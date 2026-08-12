//! The embedded loadout portraits.
//!
//! Every loadout ships two ANSI half-block renders — a 64x32 `_sm` and
//! an 86x43 `_lg` — under `assets/`. They are `include_str!`d, so the
//! binary carries its own art and resolves no asset path at runtime.
//!
//! Two ways out of here: [`Loadout::portrait`] hands back the raw
//! escape-coded text for printing straight to a terminal, and [`Art`]
//! parses that text into styled cells the TUI blits into a ratatui
//! buffer.

use std::fmt;

use clap::ValueEnum;
use ratatui::style::{Color, Modifier, Style};

/// A loadout onboarding can offer. Variant order is the order the
/// picker lists them in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, ValueEnum)]
pub enum Loadout {
    Redteamer,
    Researcher,
    Stalwart,
    Tinkerer,
    Vibecoder,
}

impl Loadout {
    /// Every loadout, in picker order.
    pub const ALL: [Self; 5] = [
        Self::Redteamer,
        Self::Researcher,
        Self::Stalwart,
        Self::Tinkerer,
        Self::Vibecoder,
    ];

    /// Lowercase identifier: the asset file stem, the value accepted on
    /// the command line, and what a selection is reported as.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Redteamer => "redteamer",
            Self::Researcher => "researcher",
            Self::Stalwart => "stalwart",
            Self::Tinkerer => "tinkerer",
            Self::Vibecoder => "vibecoder",
        }
    }

    /// The name the picker shows, definite article and all.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Redteamer => "The Redteamer",
            Self::Researcher => "The Researcher",
            Self::Stalwart => "The Stalwart",
            Self::Tinkerer => "The Hacker-Tinkerer",
            Self::Vibecoder => "The Vibecoder",
        }
    }

    /// Who this loadout is for, in a sentence or two.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Redteamer => {
                "Thinks in exploits, reads code looking for the crack. Smells unsanitized input \
                 from across the repo."
            }
            Self::Researcher => {
                "Papers open in a dozen tabs, prefers a notebook to an IDE. Derives from first \
                 principles before reaching for a library."
            }
            Self::Stalwart => {
                "Decades of experience, conservative about tooling, values fundamentals and \
                 stability. Distrusts anything with a logo and will lecture you about the OOM \
                 killer."
            }
            Self::Tinkerer => {
                "Homelab in the closet, dotfiles repo with hundreds of stars, runs NixOS on the \
                 laptop. Yak-shaves for joy."
            }
            Self::Vibecoder => {
                "Ships on intuition, LLM-native, light on fundamentals. Builds features in an \
                 afternoon, can't always explain the diff."
            }
        }
    }

    /// The packages this loadout brings into a session.
    #[must_use]
    pub const fn packages(self) -> &'static [&'static str] {
        match self {
            Self::Redteamer => &["grype", "syft", "ast-grep", "zizmor"],
            Self::Researcher => &["python", "numpy", "typst", "lean"],
            Self::Stalwart => &["emacs", "gawk", "tmux", "make"],
            Self::Tinkerer => &["helix", "zellij", "yazi", "fish"],
            Self::Vibecoder => &["claude-code", "bun", "next", "railway"],
        }
    }

    /// This loadout's portrait at `size`: escape-coded text, one line
    /// per row, each line ending in a reset so it prints as-is.
    #[must_use]
    pub const fn portrait(self, size: Size) -> &'static str {
        match (self, size) {
            (Self::Redteamer, Size::Small) => include_str!("../assets/redteamer_sm.txt"),
            (Self::Redteamer, Size::Large) => include_str!("../assets/redteamer_lg.txt"),
            (Self::Researcher, Size::Small) => include_str!("../assets/researcher_sm.txt"),
            (Self::Researcher, Size::Large) => include_str!("../assets/researcher_lg.txt"),
            (Self::Stalwart, Size::Small) => include_str!("../assets/stalwart_sm.txt"),
            (Self::Stalwart, Size::Large) => include_str!("../assets/stalwart_lg.txt"),
            (Self::Tinkerer, Size::Small) => include_str!("../assets/tinkerer_sm.txt"),
            (Self::Tinkerer, Size::Large) => include_str!("../assets/tinkerer_lg.txt"),
            (Self::Vibecoder, Size::Small) => include_str!("../assets/vibecoder_sm.txt"),
            (Self::Vibecoder, Size::Large) => include_str!("../assets/vibecoder_lg.txt"),
        }
    }
}

/// The user-facing title; [`Loadout::slug`] is the machine-facing
/// spelling.
impl fmt::Display for Loadout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.title())
    }
}

/// Which of the two renders to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Size {
    Small,
    Large,
}

impl Size {
    /// Portrait dimensions in terminal cells, `(columns, rows)`. Every
    /// asset of a given size shares them; the tests hold the art to it.
    #[must_use]
    pub const fn dimensions(self) -> (u16, u16) {
        match self {
            Self::Small => (64, 32),
            Self::Large => (86, 43),
        }
    }

    /// The largest portrait that fits a terminal `columns` wide.
    ///
    /// Width alone decides: a portrait taller than the window still
    /// reads fine once it scrolls, whereas one wider than the window
    /// wraps and turns to noise.
    #[must_use]
    pub const fn fitting(columns: u16) -> Self {
        if columns >= Self::Large.dimensions().0 {
            Self::Large
        } else {
            Self::Small
        }
    }
}

/// One rendered cell: the glyph and the colors it carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub symbol: char,
    pub style: Style,
}

/// A portrait parsed out of its escape codes into styled cells, sized
/// and indexed for blitting.
///
/// Rows are kept as-is rather than padded to a rectangle: the renderer
/// clips per cell anyway, so a short row simply draws nothing past its
/// end.
#[derive(Clone, Debug)]
pub struct Art {
    rows: Vec<Vec<Cell>>,
}

impl Art {
    /// Parse an escape-coded portrait.
    ///
    /// Total by construction — unrecognized or truncated sequences are
    /// dropped rather than rejected — because the input is compiled
    /// into the binary and a picker that renders nothing is worse than
    /// one that renders slightly wrong.
    ///
    /// Style resets at every line because the assets end each line with
    /// a `\x1b[0m`; `portrait_lines_end_reset_so_color_does_not_leak`
    /// keeps that true.
    #[must_use]
    pub fn parse(art: &str) -> Self {
        Self {
            rows: art.lines().map(parse_line).collect(),
        }
    }

    /// Width in cells: the longest row.
    #[must_use]
    pub fn width(&self) -> u16 {
        let widest = self.rows.iter().map(Vec::len).max().unwrap_or(0);
        u16::try_from(widest).unwrap_or(u16::MAX)
    }

    /// Height in cells.
    #[must_use]
    pub fn height(&self) -> u16 {
        u16::try_from(self.rows.len()).unwrap_or(u16::MAX)
    }

    /// The rows, top to bottom.
    pub fn rows(&self) -> impl Iterator<Item = &[Cell]> {
        self.rows.iter().map(Vec::as_slice)
    }
}

/// Split one line into cells, tracking the style the escape codes set.
fn parse_line(line: &str) -> Vec<Cell> {
    let mut chars = line.chars();
    let mut style = Style::default();
    let mut cells = Vec::new();

    while let Some(c) = chars.next() {
        if c != '\x1b' {
            cells.push(Cell { symbol: c, style });
            continue;
        }
        // CSI: `ESC [ <params> <final byte>`. Anything else — a bare
        // ESC, some other introducer — is swallowed with the character
        // that follows it.
        if chars.next() != Some('[') {
            continue;
        }
        let mut params = String::new();
        let mut sgr = false;
        for c in chars.by_ref() {
            match c {
                '0'..='9' | ';' => params.push(c),
                // `m` is Select Graphic Rendition; every other final
                // byte (cursor moves, erases) is a no-op for static art.
                final_byte => {
                    sgr = final_byte == 'm';
                    break;
                }
            }
        }
        if sgr {
            style = apply_sgr(style, &params);
        }
    }
    cells
}

/// Fold one SGR parameter list into `style`. The art uses four codes:
/// `0` reset, `7` reverse video, and truecolor `38;2;r;g;b` /
/// `48;2;r;g;b`; anything else is skipped.
fn apply_sgr(style: Style, params: &str) -> Style {
    let codes: Vec<u16> = params.split(';').filter_map(|p| p.parse().ok()).collect();

    // Indexed rather than iterated: a truecolor code consumes the four
    // parameters after it, which a plain `for` over the codes can't do.
    let mut style = style;
    let mut i = 0;
    while i < codes.len() {
        let consumed = match codes[i..] {
            [0, ..] => {
                style = Style::default();
                1
            }
            [7, ..] => {
                style = style.add_modifier(Modifier::REVERSED);
                1
            }
            [38, 2, r, g, b, ..] => {
                style = style.fg(rgb(r, g, b));
                5
            }
            [48, 2, r, g, b, ..] => {
                style = style.bg(rgb(r, g, b));
                5
            }
            _ => 1,
        };
        i += consumed;
    }
    style
}

/// Truecolor channels are 0-255; anything wider is a malformed sequence,
/// clamped rather than dropped.
fn rgb(r: u16, g: u16, b: u16) -> Color {
    let channel = |v: u16| u8::try_from(v).unwrap_or(u8::MAX);
    Color::Rgb(channel(r), channel(g), channel(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cell width of one rendered line: the characters left after the
    /// CSI SGR sequences (`ESC [ ... m`) come out. Every glyph in the
    /// art is single-width, so characters are cells.
    fn cell_width(line: &str) -> usize {
        let mut chars = line.chars();
        let mut width = 0;
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // Skip through the sequence's final byte.
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                width += 1;
            }
        }
        width
    }

    #[test]
    fn portraits_match_their_declared_dimensions() {
        for loadout in Loadout::ALL {
            for size in [Size::Small, Size::Large] {
                let (columns, rows) = size.dimensions();
                let art = loadout.portrait(size);
                let lines: Vec<&str> = art.lines().collect();

                assert_eq!(
                    lines.len(),
                    usize::from(rows),
                    "{} {size:?} portrait row count",
                    loadout.slug()
                );
                for (row, line) in lines.iter().enumerate() {
                    assert_eq!(
                        cell_width(line),
                        usize::from(columns),
                        "{} {size:?} portrait width at row {row}",
                        loadout.slug()
                    );
                }
            }
        }
    }

    #[test]
    fn portrait_lines_end_reset_so_color_does_not_leak() {
        for loadout in Loadout::ALL {
            for size in [Size::Small, Size::Large] {
                for line in loadout.portrait(size).lines() {
                    assert!(
                        line.ends_with("\x1b[0m"),
                        "{} {size:?} portrait line without a trailing reset",
                        loadout.slug()
                    );
                }
            }
        }
    }

    #[test]
    fn slugs_are_unique_and_name_their_assets() {
        let slugs: Vec<&str> = Loadout::ALL.iter().map(|l| l.slug()).collect();
        let mut sorted = slugs.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), slugs.len(), "duplicate loadout slug");

        // The slug is also the asset stem, so the two portraits of a
        // loadout must differ from every other loadout's.
        for loadout in Loadout::ALL {
            assert_ne!(loadout.portrait(Size::Small), loadout.portrait(Size::Large));
        }
    }

    #[test]
    fn size_fitting_falls_back_to_small_on_narrow_terminals() {
        let (large, _) = Size::Large.dimensions();
        assert_eq!(Size::fitting(large), Size::Large);
        assert_eq!(Size::fitting(large + 40), Size::Large);
        assert_eq!(Size::fitting(large - 1), Size::Small);
        assert_eq!(Size::fitting(0), Size::Small);
    }

    #[test]
    fn parsed_art_keeps_the_declared_dimensions() {
        for loadout in Loadout::ALL {
            for size in [Size::Small, Size::Large] {
                let (columns, rows) = size.dimensions();
                let art = Art::parse(loadout.portrait(size));
                assert_eq!(art.width(), columns, "{} {size:?} width", loadout.slug());
                assert_eq!(art.height(), rows, "{} {size:?} height", loadout.slug());
                assert!(
                    art.rows().all(|row| row.len() == usize::from(columns)),
                    "{} {size:?} has a ragged row",
                    loadout.slug()
                );
            }
        }
    }

    #[test]
    fn parse_reads_truecolor_foreground_and_background() {
        let art = Art::parse("\x1b[38;2;1;2;3;48;2;4;5;6m▄\x1b[0m ");
        let row: Vec<Cell> = art.rows().next().expect("one row").to_vec();

        assert_eq!(row[0].symbol, '▄');
        assert_eq!(row[0].style.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(row[0].style.bg, Some(Color::Rgb(4, 5, 6)));
        // The reset lands before the trailing space, which is plain.
        assert_eq!(
            row[1],
            Cell {
                symbol: ' ',
                style: Style::default()
            }
        );
    }

    #[test]
    fn parse_reads_reverse_video() {
        let art = Art::parse("\x1b[7m\x1b[38;2;9;9;9m▄");
        let cell = art.rows().next().expect("one row")[0];

        assert!(cell.style.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(cell.style.fg, Some(Color::Rgb(9, 9, 9)));
    }

    #[test]
    fn parse_drops_sequences_it_does_not_understand() {
        // A cursor move (final byte `H`) and an unknown SGR code carry
        // no cells and leave the style alone.
        let art = Art::parse("\x1b[2;3Ha\x1b[53mb");
        let row = art.rows().next().expect("one row");

        assert_eq!(row.len(), 2);
        assert_eq!(row[0].symbol, 'a');
        assert_eq!(row[1].symbol, 'b');
        assert_eq!(row[1].style, Style::default());
    }
}
