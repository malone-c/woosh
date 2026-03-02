use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

/// Compact single-line "WOOSH" wordmark using box-drawing characters.
/// Replaces the plain " WOOSH  " span in the title bar.
pub const LOGO_LINE: &str = " ＷＯＯＳＨ ";

pub struct DitherBackground;

const DITHER_FG: Color = Color::Rgb(45, 35, 80);
const DITHER_BG: Color = Color::Rgb(8, 6, 18);

// 5 levels of density: space, light, light, medium, medium
const DITHER_SYMBOLS: [&str; 5] = [" ", "░", "░", "▒", "▒"];

impl Widget for DitherBackground {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let style = Style::default().fg(DITHER_FG).bg(DITHER_BG);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                // Deterministic diagonal wave pattern — no RNG needed
                let idx =
                    (usize::from(x).wrapping_add(usize::from(y).wrapping_mul(3))) % 5;
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(DITHER_SYMBOLS[idx]).set_style(style);
                }
            }
        }
    }
}
