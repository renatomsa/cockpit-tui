//! Relógio grande: fonte bitmap 5×7 desenhada com meio-blocos (▀ ▄ █).
//! Cada célula carrega 2 pixels na vertical, então o pixel sai quase quadrado.

use ratatui::{buffer::Buffer, layout::Rect, style::Style, widgets::Widget};

const H: usize = 7;

fn glyph(c: char) -> Option<&'static [&'static str; H]> {
    Some(match c {
        '0' => &["#####", "#...#", "#...#", "#...#", "#...#", "#...#", "#####"],
        '1' => &[".##..", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."],
        '2' => &["#####", "....#", "....#", "#####", "#....", "#....", "#####"],
        '3' => &["#####", "....#", "....#", ".####", "....#", "....#", "#####"],
        '4' => &["#...#", "#...#", "#...#", "#####", "....#", "....#", "....#"],
        '5' => &["#####", "#....", "#....", "#####", "....#", "....#", "#####"],
        '6' => &["#####", "#....", "#....", "#####", "#...#", "#...#", "#####"],
        '7' => &["#####", "....#", "....#", "...#.", "..#..", "..#..", "..#.."],
        '8' => &["#####", "#...#", "#...#", "#####", "#...#", "#...#", "#####"],
        '9' => &["#####", "#...#", "#...#", "#####", "....#", "....#", "#####"],
        ':' => &[".", ".", "#", ".", "#", ".", "."],
        ' ' => &[".", ".", ".", ".", ".", ".", "."],
        _ => return None,
    })
}

/// Texto renderizado numa escala inteira (1 = 5 colunas × 4 linhas por dígito).
pub struct BigText<'a> {
    pub text: &'a str,
    pub scale: u16,
    pub style: Style,
}

impl BigText<'_> {
    /// Largura e altura, em células, do texto nesta escala.
    pub fn size(text: &str, scale: u16) -> (u16, u16) {
        let s = scale as usize;
        let glyphs: Vec<_> = text.chars().filter_map(glyph).collect();
        let px_w: usize = glyphs.iter().map(|g| g[0].len()).sum::<usize>() + glyphs.len().saturating_sub(1);
        ((px_w * s) as u16, (H * s).div_ceil(2) as u16)
    }

    /// Maior escala (até `max`) que cabe na área; `None` se nem a 1× cabe.
    pub fn fit(text: &str, w: u16, h: u16, max: u16) -> Option<u16> {
        (1..=max).rev().find(|&s| {
            let (tw, th) = Self::size(text, s);
            tw <= w && th <= h
        })
    }

    fn pixels(&self) -> Vec<Vec<bool>> {
        let s = self.scale as usize;
        let mut rows = vec![Vec::new(); H * s];
        for (i, g) in self.text.chars().filter_map(glyph).enumerate() {
            for (y, line) in g.iter().enumerate() {
                for r in &mut rows[y * s..(y + 1) * s] {
                    if i > 0 {
                        r.extend(std::iter::repeat_n(false, s));
                    }
                    for ch in line.chars() {
                        r.extend(std::iter::repeat_n(ch == '#', s));
                    }
                }
            }
        }
        rows
    }
}

impl Widget for BigText<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let px = self.pixels();
        let at = |x: usize, y: usize| px.get(y).and_then(|r| r.get(x)).copied().unwrap_or(false);
        let (w, h) = Self::size(self.text, self.scale);
        for cy in 0..h.min(area.height) {
            for cx in 0..w.min(area.width) {
                let (top, bot) = (at(cx as usize, cy as usize * 2), at(cx as usize, cy as usize * 2 + 1));
                let sym = match (top, bot) {
                    (true, true) => "█",
                    (true, false) => "▀",
                    (false, true) => "▄",
                    (false, false) => continue,
                };
                buf[(area.x + cx, area.y + cy)].set_symbol(sym).set_style(self.style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        // 6 dígitos × 5 + 2 dois-pontos × 1 + 7 espaços entre glifos = 39
        assert_eq!(BigText::size("15:55:44", 1), (39, 4));
        assert_eq!(BigText::size("15:55:44", 2), (78, 7));
        assert_eq!(BigText::size("15:55", 1), (25, 4));
    }

    #[test]
    fn fit_picks_largest_scale() {
        assert_eq!(BigText::fit("15:55:44", 100, 10, 3), Some(2));
        assert_eq!(BigText::fit("15:55:44", 40, 4, 3), Some(1));
        assert_eq!(BigText::fit("15:55:44", 30, 4, 3), None);
    }

    #[test]
    fn renders_half_blocks() {
        let area = Rect::new(0, 0, 5, 4);
        let mut buf = Buffer::empty(area);
        BigText { text: "8", scale: 1, style: Style::new() }.render(area, &mut buf);
        let row = |y| (0..5).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        assert_eq!(row(0), "█▀▀▀█");
        assert_eq!(row(1), "█▄▄▄█");
        assert_eq!(row(2), "█   █");
        assert_eq!(row(3), "▀▀▀▀▀");
    }
}
