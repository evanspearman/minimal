//! The picker itself: two screens in sequence — VM resources, then the
//! loadout — driven by one event loop.
//!
//! The loop is synchronous — `crossterm::event::read` blocks until the
//! user does something, and nothing else moves the screen — so unlike
//! `min dash` this needs no async runtime. [`update`] is the pure
//! transition between screens, so the flow is testable without a
//! terminal.

use anyhow::{Context as _, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Paragraph;

use crate::detail;
use crate::portrait::{Art, Loadout, Size};
use crate::resources::{Allocation, Resources, Step};
use crate::strip::{Filmstrip, Panel};

/// Blank columns kept either side of the detail copy.
const MARGIN: u16 = 2;

/// What the user came away with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub allocation: Allocation,
    pub loadout: Loadout,
}

/// What a key press asks of the picker. The same seven actions serve
/// both screens; what each one means is the screen's business.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Left,
    Right,
    Up,
    Down,
    Confirm,
    Back,
    Quit,
}

/// Which screen is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Resources,
    Loadout,
}

/// The picker's whole state.
struct Model {
    screen: Screen,
    resources: Resources,
    /// Index into [`Loadout::ALL`].
    selected: usize,
}

/// What [`update`] tells the loop to do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Done(Choice),
    Canceled,
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

/// Run the picker to a decision: `Some` when the user came out the far
/// end of both screens, `None` when they quit part-way.
///
/// The terminal is restored on every exit path, including panics
/// (`ratatui::init` installs the hook) and the `?`s below.
pub fn run() -> Result<Option<Choice>> {
    let portraits = Portraits::load();
    let mut model = Model {
        screen: Screen::Resources,
        resources: Resources::probe(),
        selected: 0,
    };
    let mut terminal = TerminalGuard::enter().context("entering the alternate screen")?;

    loop {
        terminal
            .draw(|frame| view(frame, &model, &portraits))
            .context("drawing the picker")?;

        // A resize (or any event that maps to no action) falls through
        // to another draw, which is exactly the redraw it wants.
        let Some(action) = read_action()? else {
            continue;
        };
        match update(&mut model, action) {
            Flow::Continue => {}
            Flow::Done(choice) => return Ok(Some(choice)),
            Flow::Canceled => return Ok(None),
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

/// Key bindings: arrows or vi keys, enter or space to go forward, Esc to
/// go back, `q` or Ctrl-C to leave empty-handed.
fn action_for(key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return matches!(key.code, KeyCode::Char('c')).then_some(Action::Quit);
    }
    match key.code {
        KeyCode::Left | KeyCode::Char('h') => Some(Action::Left),
        KeyCode::Right | KeyCode::Char('l') => Some(Action::Right),
        KeyCode::Up | KeyCode::Char('k') => Some(Action::Up),
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => Some(Action::Down),
        KeyCode::Enter | KeyCode::Char(' ') => Some(Action::Confirm),
        KeyCode::Esc => Some(Action::Back),
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

/// Apply one action. Pure: the whole flow — both screens, back and
/// forward — is exercised through this function in the tests.
fn update(model: &mut Model, action: Action) -> Flow {
    match (model.screen, action) {
        (_, Action::Quit) => Flow::Canceled,

        // Resources: up/down pick the field, left/right step it, enter
        // moves on, Esc backs out of the picker entirely — there is no
        // screen behind this one.
        (Screen::Resources, Action::Up | Action::Down) => {
            model.resources.toggle_field();
            Flow::Continue
        }
        (Screen::Resources, Action::Left) => {
            model.resources.adjust(Step::Down);
            Flow::Continue
        }
        (Screen::Resources, Action::Right) => {
            model.resources.adjust(Step::Up);
            Flow::Continue
        }
        (Screen::Resources, Action::Confirm) => {
            model.screen = Screen::Loadout;
            Flow::Continue
        }
        (Screen::Resources, Action::Back) => Flow::Canceled,

        // Loadout: left/right ride the ring of portraits, enter takes
        // the selection, Esc returns to the resource screen with what
        // was set there intact.
        (Screen::Loadout, Action::Left | Action::Right) => {
            model.selected = moved(model.selected, action);
            Flow::Continue
        }
        (Screen::Loadout, Action::Confirm) => match Loadout::ALL.get(model.selected) {
            Some(&loadout) => Flow::Done(Choice {
                allocation: model.resources.allocation(),
                loadout,
            }),
            None => Flow::Continue,
        },
        (Screen::Loadout, Action::Back) => {
            model.screen = Screen::Resources;
            Flow::Continue
        }
        (Screen::Loadout, Action::Up | Action::Down) => Flow::Continue,
    }
}

/// Move the selection, wrapping at both ends: five portraits in a ring,
/// so `←` from the first lands on the last.
fn moved(selected: usize, action: Action) -> usize {
    let count = Loadout::ALL.len();
    match action {
        Action::Left => (selected + count - 1) % count,
        Action::Right => (selected + 1) % count,
        _ => selected,
    }
}

/// Draw whichever screen is up.
fn view(frame: &mut Frame<'_>, model: &Model, portraits: &Portraits) {
    match model.screen {
        Screen::Resources => view_resources(frame, model),
        Screen::Loadout => view_loadout(frame, model, portraits),
    }
}

/// Host facts and the two fields, as a block in the middle of the
/// screen.
fn view_resources(frame: &mut Frame<'_>, model: &Model) {
    let lines = model.resources.lines();
    let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(height),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(title("Set VM resources"), header);

    // Centered vertically in what is left, so the block sits with the
    // header rather than pinned to the top of a tall terminal.
    let top = body.y + body.height.saturating_sub(height) / 2;
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        ratatui::layout::Rect {
            y: top,
            height: height.min(body.height),
            ..body
        },
    );

    frame.render_widget(
        hints("↑/↓ field   ←/→ adjust   enter continue   q quit"),
        footer,
    );
}

/// Header, strip, the selected loadout's copy, key hints.
fn view_loadout(frame: &mut Frame<'_>, model: &Model, portraits: &Portraits) {
    // The detail block is measured before the split, because how many
    // lines the description wraps to decides how much height is left
    // for the portraits. A short terminal squeezes the strip, not the
    // copy: a clipped portrait still reads, half a sentence doesn't.
    let selected = model.selected;
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

    frame.render_widget(title("Choose a loadout"), header);

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

    frame.render_widget(hints("←/→ move   enter choose   esc back   q quit"), footer);
}

fn title(text: &'static str) -> Paragraph<'static> {
    Paragraph::new(text)
        .style(Style::default().add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
}

fn hints(text: &'static str) -> Paragraph<'static> {
    Paragraph::new(text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
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

    /// A model on a synthetic 16-core / 64 GiB host, so the resource
    /// numbers are the same wherever the tests run.
    fn model() -> Model {
        Model {
            screen: Screen::Resources,
            resources: Resources::for_host(minvmd::cmd::config::HostCapacity {
                logical_cores: 16,
                total_mib: 65_536,
            }),
            selected: 0,
        }
    }

    /// Drive a sequence of actions through `update`, returning the flow
    /// the last one produced.
    fn drive(model: &mut Model, actions: &[Action]) -> Flow {
        actions
            .iter()
            .fold(Flow::Continue, |_, &action| update(model, action))
    }

    #[test]
    fn arrows_wrap_around_the_ring() {
        let last = Loadout::ALL.len() - 1;
        assert_eq!(moved(0, Action::Right), 1);
        assert_eq!(moved(last, Action::Right), 0);
        assert_eq!(moved(0, Action::Left), last);
        assert_eq!(moved(1, Action::Left), 0);
    }

    #[test]
    fn keys_map_to_their_actions() {
        assert_eq!(action_for(key(KeyCode::Left)), Some(Action::Left));
        assert_eq!(action_for(key(KeyCode::Char('h'))), Some(Action::Left));
        assert_eq!(action_for(key(KeyCode::Right)), Some(Action::Right));
        assert_eq!(action_for(key(KeyCode::Char('l'))), Some(Action::Right));
        assert_eq!(action_for(key(KeyCode::Up)), Some(Action::Up));
        assert_eq!(action_for(key(KeyCode::Down)), Some(Action::Down));
        assert_eq!(action_for(key(KeyCode::Tab)), Some(Action::Down));
        assert_eq!(action_for(key(KeyCode::Enter)), Some(Action::Confirm));
        assert_eq!(action_for(key(KeyCode::Esc)), Some(Action::Back));
        assert_eq!(action_for(key(KeyCode::Char('q'))), Some(Action::Quit));
        assert_eq!(action_for(key(KeyCode::Char('x'))), None);
    }

    #[test]
    fn the_resource_screen_comes_first_and_enter_moves_on() {
        let mut model = model();
        assert_eq!(model.screen, Screen::Resources);

        assert_eq!(drive(&mut model, &[Action::Confirm]), Flow::Continue);
        assert_eq!(model.screen, Screen::Loadout);
    }

    #[test]
    fn the_choice_carries_both_the_allocation_and_the_loadout() {
        let mut model = model();
        let default_vcpus = model.resources.allocation().vcpus;

        // One more vcpu, then one portrait along, then take it.
        let flow = drive(
            &mut model,
            &[
                Action::Right,
                Action::Confirm,
                Action::Right,
                Action::Confirm,
            ],
        );

        let Flow::Done(choice) = flow else {
            panic!("enter on the loadout screen must finish: {flow:?}");
        };
        assert_eq!(choice.loadout, Loadout::ALL[1]);
        assert_eq!(
            choice.allocation.vcpus,
            default_vcpus + 1,
            "the resource screen's edit came through"
        );
    }

    #[test]
    fn esc_goes_back_from_the_loadouts_and_keeps_the_allocation() {
        let mut model = model();
        drive(&mut model, &[Action::Right, Action::Confirm]);
        let allocated = model.resources.allocation();

        assert_eq!(drive(&mut model, &[Action::Back]), Flow::Continue);
        assert_eq!(model.screen, Screen::Resources);
        assert_eq!(
            model.resources.allocation(),
            allocated,
            "stepping back must not reset what was set"
        );
    }

    #[test]
    fn esc_on_the_first_screen_leaves_the_picker() {
        let mut model = model();
        assert_eq!(drive(&mut model, &[Action::Back]), Flow::Canceled);
    }

    #[test]
    fn quitting_works_from_either_screen() {
        assert_eq!(drive(&mut model(), &[Action::Quit]), Flow::Canceled);
        assert_eq!(
            drive(&mut model(), &[Action::Confirm, Action::Quit]),
            Flow::Canceled
        );
    }

    #[test]
    fn up_and_down_move_the_field_on_the_resource_screen_only() {
        let mut model = model();
        // The focused row wears the marker and the arrows, so the
        // rendered block is what moving the focus changes.
        let before = render(&model, 120, 40);
        drive(&mut model, &[Action::Down]);
        assert_ne!(render(&model, 120, 40), before, "the focus moved");

        // On the loadout screen they do nothing — the strip is one row.
        drive(&mut model, &[Action::Confirm]);
        let selected = model.selected;
        drive(&mut model, &[Action::Up, Action::Down]);
        assert_eq!(model.selected, selected);
    }

    #[test]
    fn ctrl_c_quits_and_other_control_chords_do_nothing() {
        let chord = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(action_for(chord('c')), Some(Action::Quit));
        // Ctrl-L is a redraw in most TUIs, not a move; it must not be
        // read as the plain `l` binding.
        assert_eq!(action_for(chord('l')), None);
    }

    /// Render `model` and read the screen back as plain rows.
    fn render(model: &Model, width: u16, height: u16) -> String {
        let portraits = Portraits::load();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
                .expect("test backend");
        terminal
            .draw(|frame| view(frame, model, &portraits))
            .expect("draw");

        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .filter_map(|x| buffer.cell((x, y)).map(ratatui::buffer::Cell::symbol))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The loadout screen with `selected` up.
    fn loadout_screen(selected: usize, width: u16, height: u16) -> String {
        let mut model = model();
        model.screen = Screen::Loadout;
        model.selected = selected;
        render(&model, width, height)
    }

    #[test]
    fn the_resource_screen_shows_the_host_the_ceilings_and_the_fields() {
        let rendered = render(&model(), 120, 40);

        assert!(rendered.contains("Set VM resources"), "{rendered}");
        assert!(rendered.contains("16 cores · 64 GiB"), "host:\n{rendered}");
        assert!(
            rendered.contains("up to 14 cores · 48 GiB"),
            "ceilings:\n{rendered}"
        );
        assert!(rendered.contains("CPU cores"), "cpu field:\n{rendered}");
        assert!(rendered.contains("Memory"), "memory field:\n{rendered}");
    }

    #[test]
    fn adjusting_a_field_shows_up_on_the_resource_screen() {
        let mut model = model();
        let before = render(&model, 120, 40);
        drive(&mut model, &[Action::Right, Action::Right]);
        let after = render(&model, 120, 40);

        assert_ne!(before, after, "the screen must reflect the new value");
        assert!(
            after.contains(&format!("{}", model.resources.allocation().vcpus)),
            "{after}"
        );
    }

    #[test]
    fn the_frame_carries_the_selected_loadouts_title_copy_and_packages() {
        let rendered = loadout_screen(2, 120, 50);
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
        let before = loadout_screen(2, 120, 50);
        let after = loadout_screen(moved(2, Action::Right), 120, 50);

        assert!(before.contains("OOM killer"), "stalwart copy");
        assert!(!after.contains("OOM killer"), "stalwart copy did not leave");
        assert!(after.contains("Yak-shaves"), "tinkerer copy did not arrive");
    }

    #[test]
    fn a_short_terminal_keeps_the_copy_and_squeezes_the_portraits() {
        let rendered = loadout_screen(2, 120, 12);

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
