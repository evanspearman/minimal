//! The filmstrip: every loadout's portrait in one horizontal row,
//! scrolled so the selected one sits centered and its neighbors run off
//! both edges.
//!
//! Five portraits side by side are far wider than any terminal — the
//! selection alone is 86 columns — so this widget places them in its own
//! coordinate space, shifts that space to center the selection, and
//! blits cell by cell, dropping whatever falls outside the viewport.
//! That is also why it writes into the [`Buffer`] directly instead of
//! composing `Paragraph`s: a `Rect` cannot start off-screen.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use crate::portrait::Art;

/// Blank columns between neighboring portraits.
const GAP: u16 = 4;

/// One portrait in the strip, with the label naming it.
pub struct Panel<'a> {
    pub art: &'a Art,
    pub label: &'a str,
}

/// Portraits in a row, centered on `selected`.
pub struct Filmstrip<'a> {
    panels: &'a [Panel<'a>],
    selected: usize,
}

impl<'a> Filmstrip<'a> {
    /// `selected` indexes `panels`; out of range simply centers nothing.
    #[must_use]
    pub fn new(panels: &'a [Panel<'a>], selected: usize) -> Self {
        Self { panels, selected }
    }
}

impl Widget for Filmstrip<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // The last row carries the labels; the portraits get the rest.
        let Some(portraits) = area
            .height
            .checked_sub(1)
            .map(|height| Rect { height, ..area })
        else {
            return;
        };
        let labels = Rect {
            y: area.bottom().saturating_sub(1),
            height: 1,
            ..area
        };

        let widths: Vec<u16> = self.panels.iter().map(|p| p.art.width()).collect();
        let lefts = screen_lefts(&widths, self.selected, area.width);

        for ((index, panel), left) in self.panels.iter().enumerate().zip(lefts) {
            // Each portrait is centered in the band, so the two sizes
            // share a mid-line instead of hanging off the top.
            let top = i32::from(portraits.y)
                + (i32::from(portraits.height) - i32::from(panel.art.height())) / 2;
            blit(panel.art, left + i32::from(area.x), top, portraits, buf);

            let style = if index == self.selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let label_left = left
                + i32::from(area.x)
                + (i32::from(panel.art.width()) - text_width(panel.label)) / 2;
            write_text(
                panel.label,
                label_left,
                i32::from(labels.y),
                labels,
                style,
                buf,
            );
        }
    }
}

/// Left edge of each portrait, in screen columns relative to the strip's
/// own origin, once the strip is shifted to center `selected`. Values
/// may be negative: a portrait can start off the left edge.
fn screen_lefts(widths: &[u16], selected: usize, area_width: u16) -> Vec<i32> {
    let lefts: Vec<i32> = widths
        .iter()
        .scan(0i32, |x, &width| {
            let left = *x;
            *x += i32::from(width) + i32::from(GAP);
            Some(left)
        })
        .collect();

    // No selection to center on (empty strip, or an index past the end):
    // leave the row where it starts.
    let shift = match (lefts.get(selected), widths.get(selected)) {
        (Some(left), Some(width)) => {
            let center = left + i32::from(*width) / 2;
            i32::from(area_width) / 2 - center
        }
        _ => 0,
    };

    lefts.iter().map(|left| left + shift).collect()
}

/// Draw `art` with its top-left at `(left, top)`, clipped to `area`.
fn blit(art: &Art, left: i32, top: i32, area: Rect, buf: &mut Buffer) {
    for (row, cells) in art.rows().enumerate() {
        let y = top + i32::try_from(row).unwrap_or(i32::MAX);
        for (column, cell) in cells.iter().enumerate() {
            let x = left + i32::try_from(column).unwrap_or(i32::MAX);
            if let Some(target) = cell_at(x, y, area, buf) {
                target.set_char(cell.symbol).set_style(cell.style);
            }
        }
    }
}

/// Draw `text` starting at `(left, y)`, clipped to `area`.
fn write_text(text: &str, left: i32, y: i32, area: Rect, style: Style, buf: &mut Buffer) {
    for (column, symbol) in text.chars().enumerate() {
        let x = left + i32::try_from(column).unwrap_or(i32::MAX);
        if let Some(target) = cell_at(x, y, area, buf) {
            target.set_char(symbol).set_style(style);
        }
    }
}

/// The buffer cell at `(x, y)`, or `None` when that lands outside
/// `area` — which is the whole clipping story for both drawing helpers.
fn cell_at(x: i32, y: i32, area: Rect, buf: &mut Buffer) -> Option<&mut ratatui::buffer::Cell> {
    let inside =
        |value: i32, low: u16, high: u16| value >= i32::from(low) && value < i32::from(high);
    (inside(x, area.left(), area.right()) && inside(y, area.top(), area.bottom()))
        .then(|| buf.cell_mut((x as u16, y as u16)))
        .flatten()
}

/// Label width in cells. Labels are ASCII loadout names, so characters
/// are columns.
fn text_width(text: &str) -> i32 {
    i32::try_from(text.chars().count()).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three panels 10 wide with `GAP` between them: lefts at 0, 14, 28
    /// before the strip is shifted.
    const WIDTHS: [u16; 3] = [10, 10, 10];

    #[test]
    fn the_selected_portrait_lands_centered() {
        let area_width = 40;
        for selected in 0..WIDTHS.len() {
            let lefts = screen_lefts(&WIDTHS, selected, area_width);
            let center = lefts[selected] + i32::from(WIDTHS[selected]) / 2;
            assert_eq!(center, i32::from(area_width) / 2, "selected {selected}");
        }
    }

    #[test]
    fn neighbors_keep_their_spacing_and_may_start_off_screen() {
        let lefts = screen_lefts(&WIDTHS, 2, 40);

        let gaps: Vec<i32> = lefts
            .windows(2)
            .map(|pair| pair[1] - pair[0] - i32::from(WIDTHS[0]))
            .collect();
        assert_eq!(gaps, vec![i32::from(GAP); 2]);
        assert!(lefts[0] < 0, "the first portrait scrolls off the left edge");
    }

    #[test]
    fn an_out_of_range_selection_does_not_shift_the_strip() {
        assert_eq!(screen_lefts(&WIDTHS, 9, 40), vec![0, 14, 28]);
        assert_eq!(screen_lefts(&[], 0, 40), Vec::<i32>::new());
    }

    #[test]
    fn rendering_centers_the_selection_and_clips_its_neighbors() {
        // Distinguishable one-row portraits, four cells wide.
        let arts: Vec<Art> = ["aaaa", "bbbb", "cccc"]
            .iter()
            .map(|a| Art::parse(a))
            .collect();
        let panels: Vec<Panel<'_>> = arts
            .iter()
            .zip(["one", "two", "three"])
            .map(|(art, label)| Panel { art, label })
            .collect();

        let area = Rect::new(0, 0, 14, 2);
        let mut buf = Buffer::empty(area);
        Filmstrip::new(&panels, 1).render(area, &mut buf);

        let row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).expect("in bounds").symbol())
            .collect::<Vec<_>>()
            .concat();
        // 'b' centered, its neighbors clipped to the one column each
        // that still fits inside the viewport.
        assert_eq!(row, "a    bbbb    c");

        let labels: String = (0..area.width)
            .map(|x| buf.cell((x, 1)).expect("in bounds").symbol())
            .collect::<Vec<_>>()
            .concat();
        assert!(
            labels.contains("two"),
            "selected label rendered: {labels:?}"
        );
    }

    #[test]
    fn rendering_into_a_one_row_area_draws_nothing_and_does_not_panic() {
        let art = Art::parse("xxxx");
        let panels = [Panel {
            art: &art,
            label: "one",
        }];

        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        Filmstrip::new(&panels, 0).render(area, &mut buf);

        // The single row is the label row; the portrait has nowhere to go.
        let row: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).expect("in bounds").symbol())
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(row, "   one    ");
    }
}
