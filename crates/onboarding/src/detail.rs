//! The block under the strip: who the selected loadout is for, and the
//! packages it brings into a session.

use ratatui::style::{Color, Style};
use ratatui::text::Line;

use crate::portrait::Loadout;

/// The widest the copy is allowed to run. Prose set across a
/// 200-column terminal is a chore to read, and the block is centered,
/// so the extra width would only pull it away from the portrait above.
const MAX_WIDTH: u16 = 72;

/// What goes between package names.
const SEPARATOR: &str = " · ";

/// The lines to draw for `loadout`, given the width available to them.
///
/// Wrapped here rather than left to `Paragraph`'s own wrapping so the
/// caller can size the block from `lines.len()` before it lays the
/// frame out — the description is two lines on a wide terminal and four
/// on a narrow one, and the strip above should get whatever is left.
#[must_use]
pub fn lines(loadout: Loadout, available: u16) -> Vec<Line<'static>> {
    let width = available.min(MAX_WIDTH);
    if width == 0 {
        return Vec::new();
    }

    let description = wrap(loadout.description(), width)
        .into_iter()
        .map(|line| Line::styled(line, Style::default().fg(Color::Gray)));
    let packages = wrap(&loadout.packages().join(SEPARATOR), width)
        .into_iter()
        .map(|line| Line::styled(line, Style::default().fg(Color::White)));

    description
        .chain(std::iter::once(Line::default()))
        .chain(packages)
        .collect()
}

/// Greedy word wrap at `width` columns. A word longer than the whole
/// width gets a line to itself and overflows it; breaking mid-word
/// would read worse than a package name running past the margin.
fn wrap(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width);
    text.split_whitespace()
        .fold(Vec::new(), |mut lines: Vec<String>, word| {
            let fits = |line: &String| line.chars().count() + 1 + word.chars().count() <= width;
            match lines.last_mut() {
                Some(line) if fits(line) => {
                    line.push(' ');
                    line.push_str(word);
                }
                _ => lines.push(word.to_owned()),
            }
            lines
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn wrap_breaks_between_words_and_never_exceeds_the_width() {
        let wrapped = wrap("the quick brown fox jumps over the lazy dog", 12);

        assert!(
            wrapped.iter().all(|line| line.chars().count() <= 12),
            "{wrapped:?}"
        );
        assert_eq!(
            wrapped.concat().replace(' ', ""),
            "thequickbrownfoxjumpsoverthelazydog"
        );
    }

    #[test]
    fn wrap_gives_an_overlong_word_its_own_line() {
        assert_eq!(
            wrap("a supercalifragilistic b", 6),
            vec!["a", "supercalifragilistic", "b"]
        );
    }

    #[test]
    fn the_block_is_description_then_packages() {
        let lines = plain(&lines(Loadout::Stalwart, 72));
        let blank = lines
            .iter()
            .position(String::is_empty)
            .expect("a blank line separates the two halves");

        assert!(lines[..blank].concat().contains("OOM killer"));
        assert_eq!(lines[blank + 1..].concat(), "emacs · gawk · tmux · make");
    }

    #[test]
    fn narrow_terminals_get_more_lines_than_wide_ones() {
        let wide = lines(Loadout::Researcher, 72).len();
        let narrow = lines(Loadout::Researcher, 30).len();

        assert!(narrow > wide, "wide {wide} lines, narrow {narrow} lines");
    }

    #[test]
    fn a_zero_width_block_draws_nothing() {
        assert!(lines(Loadout::Vibecoder, 0).is_empty());
    }

    #[test]
    fn every_loadout_renders_its_whole_description_and_package_list() {
        // Wrapping rearranges the whitespace, so compare on words.
        let words = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");

        for loadout in Loadout::ALL {
            let rendered = words(&plain(&lines(loadout, 72)).join(" "));
            let expected = words(&format!(
                "{} {}",
                loadout.description(),
                loadout.packages().join(SEPARATOR)
            ));
            assert_eq!(rendered, expected, "{}", loadout.slug());
        }
    }
}
