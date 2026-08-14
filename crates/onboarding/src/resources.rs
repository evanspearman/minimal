//! The resource screen's model: what the host has, what a VM is allowed
//! to take of it, and what the user has picked so far.
//!
//! The policy is not invented here. The vcpu ceiling, the RAM floor, the
//! defaults, and the x86_64 MMIO-hole hazard all come from `minvmd`,
//! which is what actually boots the VM — a picker that offered a value
//! `minvmd` rejects (or worse, accepts into a guest that panics at boot)
//! would be a lie. Only the RAM *ceiling* is this module's own, because
//! `minvmd` merely warns about over-allocation where a picker has to
//! draw a line.

use minvmd::cmd::config::{HostCapacity, MIN_RAM_MIB};
use minvmd::cmd::{DEFAULT_VM_RAM_MIB, DEFAULT_VM_VCPUS, max_vm_vcpus};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// RAM sizes the picker offers, in MiB. A ladder rather than a free
/// slider: the sizes that matter are round ones, and stepping through
/// twelve of them beats nudging a number 512 MiB at a time.
const RAM_LADDER: [u32; 15] = [
    512, 1024, 1536, 2048, 3072, 4096, 6144, 8192, 12288, 16384, 24576, 32768, 49152, 65536,
    131_072,
];

/// The share of host RAM a VM may claim, as `numerator / DENOMINATOR`.
/// The rest stays with the host, which still has to run the VMM, the
/// user's own desktop, and the page cache the guest's disk I/O goes
/// through.
const HOST_RAM_SHARE_NUMERATOR: u32 = 3;
const HOST_RAM_SHARE_DENOMINATOR: u32 = 4;

/// Which field the arrow keys act on. Internal: the screen owns its own
/// focus, and the caller only ever needs the [`Allocation`] that comes
/// out the far end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Cpu,
    Memory,
}

/// What the VM will be booted with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allocation {
    pub vcpus: u8,
    pub ram_mib: u32,
}

/// The host as probed, the ceilings derived from it, and the current
/// allocation.
#[derive(Clone, Debug)]
pub struct Resources {
    host: HostCapacity,
    max_vcpus: u8,
    /// Offerable RAM sizes, ascending. Never empty.
    ram_choices: Vec<u32>,
    allocation: Allocation,
    field: Field,
}

impl Resources {
    /// Probe the host and start from the defaults `minvmd` would boot
    /// with, clamped into range.
    #[must_use]
    pub fn probe() -> Self {
        Self::for_host(HostCapacity::probe())
    }

    /// The same, for a host supplied by the caller — capacity is
    /// injected so the limits are testable without probing this
    /// machine.
    #[must_use]
    pub fn for_host(host: HostCapacity) -> Self {
        let max_vcpus = max_vm_vcpus(host.logical_cores);
        let ram_choices = ram_choices(host.total_mib);
        // `minvmd`'s defaults, brought into range: the ladder may not
        // reach the default RAM on a small host, and the vcpu default is
        // itself the floor of the ceiling, so it always fits.
        let ram_mib = ram_choices
            .iter()
            .rev()
            .find(|&&mib| mib <= DEFAULT_VM_RAM_MIB)
            .copied()
            .unwrap_or_else(|| ram_choices[0]);

        Self {
            host,
            max_vcpus,
            ram_choices,
            allocation: Allocation {
                vcpus: DEFAULT_VM_VCPUS.min(max_vcpus),
                ram_mib,
            },
            field: Field::Cpu,
        }
    }

    #[must_use]
    pub fn allocation(&self) -> Allocation {
        self.allocation
    }

    /// Move between the two fields. There are only two, so either
    /// direction toggles.
    pub fn toggle_field(&mut self) {
        self.field = match self.field {
            Field::Cpu => Field::Memory,
            Field::Memory => Field::Cpu,
        };
    }

    /// Step the focused field one notch. Clamps at both ends rather
    /// than wrapping: rolling from the maximum RAM back to 512 MiB on
    /// one extra key press is a trap, not a convenience.
    pub fn adjust(&mut self, step: Step) {
        match self.field {
            Field::Cpu => {
                self.allocation.vcpus = match step {
                    Step::Down => self.allocation.vcpus.saturating_sub(1).max(1),
                    Step::Up => self.allocation.vcpus.saturating_add(1).min(self.max_vcpus),
                };
            }
            Field::Memory => {
                let current = self
                    .ram_choices
                    .iter()
                    .position(|&mib| mib == self.allocation.ram_mib)
                    .unwrap_or(0);
                let next = match step {
                    Step::Down => current.saturating_sub(1),
                    Step::Up => (current + 1).min(self.ram_choices.len() - 1),
                };
                self.allocation.ram_mib = self.ram_choices[next];
            }
        }
    }

    /// The screen: what the host has, what may be allocated, and the
    /// two fields.
    #[must_use]
    pub fn lines(&self) -> Vec<Line<'static>> {
        let max_ram = *self.ram_choices.last().unwrap_or(&MIN_RAM_MIB);
        vec![
            fact(
                "This machine",
                &format!(
                    "{} cores · {}",
                    self.host.logical_cores,
                    format_mib(self.host.total_mib)
                ),
            ),
            fact(
                "Allocatable",
                &format!("up to {} cores · {}", self.max_vcpus, format_mib(max_ram)),
            ),
            Line::default(),
            self.field_line(
                Field::Cpu,
                "CPU cores",
                &self.allocation.vcpus.to_string(),
                &format!("max {}", self.max_vcpus),
            ),
            self.field_line(
                Field::Memory,
                "Memory",
                &format_mib(self.allocation.ram_mib),
                &format!("max {}", format_mib(max_ram)),
            ),
        ]
    }

    /// One adjustable row: `▸ label  ◂ value ▸  max …`, lit when it has
    /// the focus.
    fn field_line(&self, field: Field, label: &str, value: &str, max: &str) -> Line<'static> {
        let focused = self.field == field;
        let (marker, label_style, value_style) = if focused {
            (
                "▸ ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (
                "  ",
                Style::default().fg(Color::Gray),
                Style::default().fg(Color::Gray),
            )
        };
        // Arrows only on the focused row: they are an affordance for the
        // keys that would act now, not decoration for every row.
        let (left, right) = if focused {
            ("◂ ", " ▸")
        } else {
            ("  ", "  ")
        };

        Line::from(vec![
            Span::styled(pad(&format!("{marker}{label}"), LABEL_WIDTH), label_style),
            Span::styled(
                pad(
                    &format!("{left}{}{right}", center(value, VALUE_WIDTH)),
                    ARROWS_WIDTH,
                ),
                value_style,
            ),
            Span::styled(pad(max, MAX_WIDTH), Style::default().fg(Color::DarkGray)),
        ])
    }
}

/// Which way a step moves the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Down,
    Up,
}

// Column widths. Every line the screen produces is `LABEL_WIDTH +
// ARROWS_WIDTH + MAX_WIDTH` cells wide, so centering the block as a
// whole leaves the columns lined up with each other.
const LABEL_WIDTH: usize = 16;
/// Wide enough for the longest value the ladder produces (`128 GiB`).
const VALUE_WIDTH: usize = 7;
/// The value, an arrow either side, and a gutter before the max column.
const ARROWS_WIDTH: usize = VALUE_WIDTH + 6;
const MAX_WIDTH: usize = 14;

/// A `label   value` line, in the same columns as the adjustable rows.
fn fact(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            pad(&format!("  {label}"), LABEL_WIDTH),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            pad(value, ARROWS_WIDTH + MAX_WIDTH),
            Style::default().fg(Color::Gray),
        ),
    ])
}

/// The RAM sizes offerable on a host with `total_mib` of memory.
///
/// Two filters: the host's share, and — on x86_64 — the 32-bit MMIO
/// hole at ~3–4 GiB, which mis-places the initramfs and panics the guest
/// kernel at boot (`minvmd::cmd::DEFAULT_VM_RAM_MIB`). Sizes inside the
/// hole are not offered at all, since there is nothing the user could do
/// about the resulting panic.
fn ram_choices(total_mib: u32) -> Vec<u32> {
    let ceiling = max_ram_mib(total_mib);
    let choices: Vec<u32> = RAM_LADDER
        .into_iter()
        .filter(|&mib| mib >= MIN_RAM_MIB && mib <= ceiling && !straddles_mmio_hole(mib))
        .collect();
    if choices.is_empty() {
        vec![MIN_RAM_MIB]
    } else {
        choices
    }
}

/// The most RAM a VM may claim on this host: the host's share, floored
/// at the amount a guest needs to reach userspace.
fn max_ram_mib(total_mib: u32) -> u32 {
    (total_mib / HOST_RAM_SHARE_DENOMINATOR * HOST_RAM_SHARE_NUMERATOR).max(MIN_RAM_MIB)
}

/// Whether `mib` lands in the x86_64 32-bit MMIO/PCI hole. libkrun boots
/// a same-arch guest, so this binary's target arch is the guest's.
#[cfg(target_arch = "x86_64")]
fn straddles_mmio_hole(mib: u32) -> bool {
    (3073..=6143).contains(&mib)
}

/// aarch64 and friends have no low MMIO hole.
#[cfg(not(target_arch = "x86_64"))]
fn straddles_mmio_hole(_mib: u32) -> bool {
    false
}

/// MiB as the user thinks of it: whole GiB where it divides evenly, one
/// decimal where it doesn't, MiB below a gigabyte.
fn format_mib(mib: u32) -> String {
    match mib {
        mib if mib < 1024 => format!("{mib} MiB"),
        mib if mib % 1024 == 0 => format!("{} GiB", mib / 1024),
        mib => format!("{:.1} GiB", f64::from(mib) / 1024.0),
    }
}

/// Pad to `width` display cells (every glyph here is single-width).
fn pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    format!("{text}{}", " ".repeat(width.saturating_sub(len)))
}

/// Center within `width` cells, so a value stays put between its arrows
/// as it grows from `2` to `12`.
fn center(text: &str, width: usize) -> String {
    let len = text.chars().count();
    let left = width.saturating_sub(len) / 2;
    let right = width.saturating_sub(len + left);
    format!("{}{text}{}", " ".repeat(left), " ".repeat(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(logical_cores: u32, total_mib: u32) -> HostCapacity {
        HostCapacity {
            logical_cores,
            total_mib,
        }
    }

    /// 64 GiB, 16 cores: a roomy host, so the ladder is not the binding
    /// constraint anywhere.
    fn workstation() -> Resources {
        Resources::for_host(host(16, 65_536))
    }

    #[test]
    fn the_vcpu_ceiling_leaves_the_host_its_reserve() {
        assert_eq!(workstation().max_vcpus, 14);
        // Small hosts keep minvmd's baseline rather than dropping to zero.
        assert_eq!(Resources::for_host(host(2, 8192)).max_vcpus, 2);
        assert_eq!(Resources::for_host(host(1, 8192)).max_vcpus, 2);
    }

    #[test]
    fn ram_choices_stay_within_the_hosts_share() {
        let choices = ram_choices(16_384);
        assert!(
            choices.iter().all(|&mib| mib <= 12_288),
            "16 GiB host offered {choices:?}"
        );
        assert!(choices.contains(&12_288), "the ceiling itself is offerable");
    }

    #[test]
    fn a_tiny_host_still_gets_the_floor() {
        // Below the floor after the host's share, the guest still needs
        // enough RAM to reach userspace.
        assert_eq!(ram_choices(256), vec![MIN_RAM_MIB]);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn no_offered_size_straddles_the_x86_64_mmio_hole() {
        let choices = ram_choices(1_048_576);
        assert!(
            choices.iter().all(|&mib| !(3073..=6143).contains(&mib)),
            "{choices:?}"
        );
        assert!(choices.contains(&3072) && choices.contains(&6144));
    }

    #[test]
    fn the_starting_allocation_is_minvmds_default_brought_into_range() {
        let allocation = workstation().allocation();
        assert_eq!(allocation.vcpus, DEFAULT_VM_VCPUS);
        assert_eq!(allocation.ram_mib, DEFAULT_VM_RAM_MIB);

        // A host too small for the default RAM falls back to the largest
        // size it can actually offer.
        let small = Resources::for_host(host(4, 1024));
        assert!(small.allocation().ram_mib <= max_ram_mib(1024));
    }

    #[test]
    fn adjusting_cpu_clamps_at_one_and_at_the_ceiling() {
        let mut resources = workstation();
        for _ in 0..40 {
            resources.adjust(Step::Up);
        }
        assert_eq!(resources.allocation().vcpus, resources.max_vcpus);

        for _ in 0..40 {
            resources.adjust(Step::Down);
        }
        assert_eq!(resources.allocation().vcpus, 1);
    }

    #[test]
    fn adjusting_memory_walks_the_ladder_and_clamps() {
        let mut resources = workstation();
        resources.toggle_field();
        assert_eq!(resources.field, Field::Memory);

        let start = resources.allocation().ram_mib;
        resources.adjust(Step::Up);
        assert!(
            resources.allocation().ram_mib > start,
            "up moves to the next rung"
        );

        for _ in 0..40 {
            resources.adjust(Step::Up);
        }
        assert_eq!(
            resources.allocation().ram_mib,
            *resources.ram_choices.last().expect("choices are non-empty")
        );

        for _ in 0..40 {
            resources.adjust(Step::Down);
        }
        assert_eq!(resources.allocation().ram_mib, resources.ram_choices[0]);
    }

    #[test]
    fn adjusting_one_field_leaves_the_other_alone() {
        let mut resources = workstation();
        let ram = resources.allocation().ram_mib;
        resources.adjust(Step::Up);
        assert_eq!(resources.allocation().ram_mib, ram);

        resources.toggle_field();
        let vcpus = resources.allocation().vcpus;
        resources.adjust(Step::Up);
        assert_eq!(resources.allocation().vcpus, vcpus);
    }

    #[test]
    fn the_screen_states_the_host_and_the_ceilings() {
        let rendered = workstation()
            .lines()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("16 cores"), "host cores:\n{rendered}");
        assert!(rendered.contains("64 GiB"), "host RAM:\n{rendered}");
        assert!(rendered.contains("max 14"), "vcpu ceiling:\n{rendered}");
        assert!(rendered.contains("up to 14 cores"), "ceilings:\n{rendered}");
    }

    #[test]
    fn the_focused_row_is_the_one_wearing_the_arrows() {
        let mut resources = workstation();
        let cpu_focused = resources.lines()[3].to_string();
        assert!(cpu_focused.contains('◂'), "{cpu_focused:?}");

        resources.toggle_field();
        let memory_focused = resources.lines()[4].to_string();
        assert!(memory_focused.contains('◂'), "{memory_focused:?}");
        assert!(!resources.lines()[3].to_string().contains('◂'));
    }

    #[test]
    fn sizes_read_as_gib_where_they_divide_evenly() {
        assert_eq!(format_mib(512), "512 MiB");
        assert_eq!(format_mib(2048), "2 GiB");
        assert_eq!(format_mib(1536), "1.5 GiB");
        assert_eq!(format_mib(65_536), "64 GiB");
    }
}
