//! The picker itself: a full-screen ratatui strip of portraits, arrow
//! keys to move the selection, enter to take it.
//!
//! The loop is synchronous — `crossterm::event::read` blocks until the
//! user does something, and nothing else moves the screen — so unlike
//! `min dash` this needs no async runtime.

use anyhow::{Context as _, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Paragraph;

use crate::detail;
use crate::portrait::{Art, Loadout, Size};
use crate::strip::{Filmstrip, Panel};

/// Blank columns kept either side of the detail copy.
const MARGIN: u16 = 2;

/// What a key press asks of the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Prev,
    Next,
    Choose,
    Quit,
}

/// Every portrait, parsed once up front. Both sizes are kept live
/// because the selection swaps between them on every arrow key, and
/// re-parsing 86x43 cells mid-keystroke would show.
struct Portraits {
    small: Vec<Art>,
    large: Vec<Art>,
}

impl Portraits {
    fn load() -> Self {
        let parse = |size| {
            Loadout::ALL
                .iter()
                .map(|loadout| Art::parse(loadout.portrait(size)))
                .collect()
        };
        Self {
            small: parse(Size::Small),
            large: parse(Size::Large),
        }
    }
}

/// Run the picker to a decision: `Some` when the user chose a loadout,
/// `None` when they quit without choosing.
///
/// The terminal is restored on every exit path, including panics
/// (`ratatui::init` installs the hook) and the `?`s below.
pub fn run() -> Result<Option<Loadout>> {
    let portraits = Portraits::load();
    let mut terminal = TerminalGuard::enter().context("entering the alternate screen")?;
    let mut selected = 0;

    loop {
        terminal
            .draw(|frame| view(frame, &portraits, selected))
            .context("drawing the picker")?;

        // A resize (or any event that maps to no action) falls through
        // to another draw, which is exactly the redraw it wants.
        let Some(action) = read_action()? else {
            continue;
        };
        match action {
            Action::Prev | Action::Next => selected = moved(selected, action),
            Action::Choose => return Ok(Loadout::ALL.get(selected).copied()),
            Action::Quit => return Ok(None),
        }
    }
}

/// Block for the next event and translate it, `None` for events the
/// picker does not act on.
fn read_action() -> Result<Option<Action>> {
    match event::read().context("reading terminal events")? {
        // Press only: with the kitty keyboard protocol a held key also
        // reports Repeat and Release, which would move the selection
        // two or three notches per tap.
        Event::Key(key) if key.kind == KeyEventKind::Press => Ok(action_for(key)),
        _ => Ok(None),
    }
}

/// Key bindings: arrows or vi keys to move, enter or space to choose,
/// `q`/Esc/Ctrl-C to leave empty-handed.
fn action_for(key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return matches!(key.code, KeyCode::Char('c')).then_some(Action::Quit);
    }
    match key.code {
        KeyCode::Left | KeyCode::Char('h') => Some(Action::Prev),
        KeyCode::Right | KeyCode::Char('l') => Some(Action::Next),
        KeyCode::Enter | KeyCode::Char(' ') => Some(Action::Choose),
        KeyCode::Esc | KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

/// Move the selection, wrapping at both ends: five portraits in a ring,
/// so `←` from the first lands on the last.
fn moved(selected: usize, action: Action) -> usize {
    let count = Loadout::ALL.len();
    match action {
        Action::Prev => (selected + count - 1) % count,
        Action::Next => (selected + 1) % count,
        Action::Choose | Action::Quit => selected,
    }
}

/// Header, strip, the selected loadout's copy, key hints.
fn view(frame: &mut Frame<'_>, portraits: &Portraits, selected: usize) {
    // The detail block is measured before the split, because how many
    // lines the description wraps to decides how much height is left
    // for the portraits. A short terminal squeezes the strip, not the
    // copy: a clipped portrait still reads, half a sentence doesn't.
    let detail = Loadout::ALL
        .get(selected)
        .map(|loadout| detail::lines(*loadout, frame.area().width.saturating_sub(2 * MARGIN)))
        .unwrap_or_default();
    let detail_height = u16::try_from(detail.len()).unwrap_or(u16::MAX);

    let [header, strip, copy, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(detail_height),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new("Choose a loadout")
            .style(Style::default().add_modifier(Modifier::BOLD))
            .alignment(Alignment::Center),
        header,
    );

    let labels: Vec<String> = Loadout::ALL.iter().map(ToString::to_string).collect();
    let panels: Vec<Panel<'_>> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| Panel {
            art: art_for(portraits, index, selected),
            label,
        })
        .collect();
    frame.render_widget(Filmstrip::new(&panels, selected), strip);

    frame.render_widget(Paragraph::new(detail).alignment(Alignment::Center), copy);

    frame.render_widget(
        Paragraph::new("←/→ move   enter choose   q quit")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center),
        footer,
    );
}

/// The selection is drawn large, everything else small — the size *is*
/// the selection cue.
fn art_for(portraits: &Portraits, index: usize, selected: usize) -> &Art {
    let bank = if index == selected {
        &portraits.large
    } else {
        &portraits.small
    };
    // Both banks are built from `Loadout::ALL`, so the index is in range.
    &bank[index]
}

/// Enters the alternate screen on construction and restores the
/// terminal on drop, so every exit path leaves the terminal usable.
struct TerminalGuard(ratatui::DefaultTerminal);

impl TerminalGuard {
    fn enter() -> std::io::Result<Self> {
        let mut terminal = ratatui::init();
        terminal.clear()?;
        Ok(Self(terminal))
    }

    fn draw(
        &mut self,
        f: impl FnOnce(&mut Frame<'_>),
    ) -> std::io::Result<ratatui::CompletedFrame<'_>> {
        self.0.draw(f)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn arrows_wrap_around_the_ring() {
        let last = Loadout::ALL.len() - 1;
        assert_eq!(moved(0, Action::Next), 1);
        assert_eq!(moved(last, Action::Next), 0);
        assert_eq!(moved(0, Action::Prev), last);
        assert_eq!(moved(1, Action::Prev), 0);
    }

    #[test]
    fn choosing_and_quitting_leave_the_selection_alone() {
        assert_eq!(moved(2, Action::Choose), 2);
        assert_eq!(moved(2, Action::Quit), 2);
    }

    #[test]
    fn keys_map_to_their_actions() {
        assert_eq!(action_for(key(KeyCode::Left)), Some(Action::Prev));
        assert_eq!(action_for(key(KeyCode::Char('h'))), Some(Action::Prev));
        assert_eq!(action_for(key(KeyCode::Right)), Some(Action::Next));
        assert_eq!(action_for(key(KeyCode::Char('l'))), Some(Action::Next));
        assert_eq!(action_for(key(KeyCode::Enter)), Some(Action::Choose));
        assert_eq!(action_for(key(KeyCode::Esc)), Some(Action::Quit));
        assert_eq!(action_for(key(KeyCode::Char('q'))), Some(Action::Quit));
        assert_eq!(action_for(key(KeyCode::Tab)), None);
    }

    #[test]
    fn ctrl_c_quits_and_other_control_chords_do_nothing() {
        let chord = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(action_for(chord('c')), Some(Action::Quit));
        // Ctrl-L is a redraw in most TUIs, not a move; it must not be
        // read as the plain `l` binding.
        assert_eq!(action_for(chord('l')), None);
    }

    /// Render a frame and read the screen back as plain rows.
    fn screen(selected: usize, width: u16, height: u16) -> Vec<String> {
        let portraits = Portraits::load();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
                .expect("test backend");
        terminal
            .draw(|frame| view(frame, &portraits, selected))
            .expect("draw");

        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .filter_map(|x| buffer.cell((x, y)).map(ratatui::buffer::Cell::symbol))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_frame_carries_the_selected_loadouts_title_copy_and_packages() {
        let rendered = screen(2, 120, 50).join("\n");
        let stalwart = Loadout::Stalwart;

        assert!(rendered.contains(stalwart.title()), "title missing");
        assert!(
            rendered.contains("OOM killer"),
            "description missing:\n{rendered}"
        );
        for package in stalwart.packages() {
            assert!(rendered.contains(package), "package {package} missing");
        }
    }

    #[test]
    fn moving_the_selection_swaps_the_copy() {
        let before = screen(2, 120, 50).join("\n");
        let after = screen(moved(2, Action::Next), 120, 50).join("\n");

        assert!(before.contains("OOM killer"), "stalwart copy");
        assert!(!after.contains("OOM killer"), "stalwart copy did not leave");
        assert!(after.contains("Yak-shaves"), "tinkerer copy did not arrive");
    }

    #[test]
    fn a_short_terminal_keeps_the_copy_and_squeezes_the_portraits() {
        let rendered = screen(2, 120, 12).join("\n");

        assert!(
            rendered.contains("OOM killer"),
            "copy survives:\n{rendered}"
        );
        assert!(rendered.contains("emacs"), "packages survive:\n{rendered}");
    }

    #[test]
    fn only_the_selection_is_drawn_large() {
        let portraits = Portraits::load();
        let (large_width, _) = Size::Large.dimensions();
        let (small_width, _) = Size::Small.dimensions();

        assert_eq!(art_for(&portraits, 1, 1).width(), large_width);
        assert_eq!(art_for(&portraits, 0, 1).width(), small_width);
    }
}
