//! Иконка-светофор: собачка (фавикон dominigames.com, если он был при сборке, иначе нарисованный «D») слева снизу
//! без фона и плашка статуса справа сверху — оценка числом, «!», полоски слабого интернета; во время проверки —
//! синяя точка слева сверху.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

use crate::score::{Badge, Color};

pub const SIZE: u32 = 32;

#[cfg(has_favicon)]
const FAVICON: Option<&[u8]> = Some(include_bytes!("../assets/favicon.ico"));
#[cfg(not(has_favicon))]
const FAVICON: Option<&[u8]> = None;

type Rgba = [u8; 4];

pub fn color_rgba(c: Color) -> Rgba {
    match c {
        Color::Green => [0x1f, 0xa8, 0x4a, 0xff],
        Color::Yellow => [0xe8, 0xa2, 0x00, 0xff],
        Color::Red => [0xd9, 0x2d, 0x2d, 0xff],
        Color::Gray => GRAY,
    }
}
const GRAY: Rgba = [0x70, 0x76, 0x80, 0xff];
const WHITE: Rgba = [0xff, 0xff, 0xff, 0xff];

/// Холст 32×32 RGBA.
pub struct Canvas {
    pub px: Vec<u8>,
}

impl Canvas {
    fn new() -> Self {
        Self {
            px: vec![0; (SIZE * SIZE * 4) as usize],
        }
    }

    fn blend(&mut self, x: i32, y: i32, c: Rgba, a: f32) {
        if x < 0 || y < 0 || x >= SIZE as i32 || y >= SIZE as i32 || a <= 0.0 {
            return;
        }
        let i = ((y as u32 * SIZE + x as u32) * 4) as usize;
        let a = a.clamp(0.0, 1.0) * c[3] as f32 / 255.0;
        let da = self.px[i + 3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        for k in 0..3 {
            let v = (c[k] as f32 * a + self.px[i + k] as f32 * da * (1.0 - a)) / oa.max(1e-6);
            self.px[i + k] = v.round() as u8;
        }
        self.px[i + 3] = (oa * 255.0).round() as u8;
    }

    fn rect(&mut self, x0: i32, y0: i32, w: i32, h: i32, c: Rgba) {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                self.blend(x, y, c, 1.0);
            }
        }
    }

    /// Скруглённый прямоугольник с мягким краем.
    fn round_rect(&mut self, x0: f32, y0: f32, w: f32, h: f32, r: f32, c: Rgba) {
        for y in 0..SIZE as i32 {
            for x in 0..SIZE as i32 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let cx = px.clamp(x0 + r, x0 + w - r);
                let cy = py.clamp(y0 + r, y0 + h - r);
                let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                self.blend(x, y, c, r - d + 0.5);
            }
        }
    }

    fn ring_arc(
        &mut self,
        cx: f32,
        cy: f32,
        r: f32,
        width: f32,
        from_deg: f32,
        to_deg: f32,
        c: Rgba,
    ) {
        for y in 0..SIZE as i32 {
            for x in 0..SIZE as i32 {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let d = (dx * dx + dy * dy).sqrt();
                let ang = dy.atan2(dx).to_degrees().rem_euclid(360.0);
                if ang >= from_deg && ang <= to_deg {
                    self.blend(x, y, c, width / 2.0 - (d - r).abs() + 0.5);
                }
            }
        }
    }

    /// Цифра или «!» шрифтом 3×5, масштаб `s`.
    fn glyph(&mut self, ch: char, x0: i32, y0: i32, s: i32, c: Rgba) {
        let rows = glyph_rows(ch);
        for (ry, row) in rows.iter().enumerate() {
            for rx in 0..3 {
                if row & (0b100 >> rx) != 0 {
                    self.rect(x0 + rx * s, y0 + ry as i32 * s, s, s, c);
                }
            }
        }
    }
}

fn glyph_rows(ch: char) -> [u8; 5] {
    match ch {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '!' => [0b010, 0b010, 0b010, 0b000, 0b010],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'x' => [0b101, 0b101, 0b010, 0b101, 0b101],
        '?' => [0b111, 0b001, 0b011, 0b000, 0b010],
        '|' => [0b101, 0b101, 0b101, 0b101, 0b101],
        _ => [0; 5],
    }
}

/// Логотип в левом нижнем углу (21×21): фавикон без фона или нарисованный «D».
const LOGO: u32 = 21;

fn base() -> Canvas {
    let mut c = Canvas::new();
    if let Some(bytes) = FAVICON
        && draw_favicon(&mut c, bytes, 0, SIZE - LOGO, LOGO).is_some()
    {
        return c;
    }
    c.round_rect(0.0, 11.0, 21.0, 21.0, 5.0, [0x1d, 0x2b, 0x4f, 0xff]);
    c.glyph('D', 6, 16, 2, WHITE);
    c
}

/// Вписывает фавикон в квадрат `box_` с сохранением пропорций и прозрачности, усредняя пиксели.
fn draw_favicon(c: &mut Canvas, bytes: &[u8], ox0: u32, oy0: u32, box_: u32) -> Option<()> {
    let dir = ico::IconDir::read(std::io::Cursor::new(bytes)).ok()?;
    let entry = dir.entries().iter().max_by_key(|e| e.width())?;
    let img = entry.decode().ok()?;
    let (w, h, data) = (img.width(), img.height(), img.rgba_data());
    let scale = (w.max(h) as f32) / box_ as f32;
    let (ow, oh) = ((w as f32 / scale) as u32, (h as f32 / scale) as u32);
    let (ox, oy) = (ox0 + (box_ - ow) / 2, oy0 + (box_ - oh));
    for y in 0..oh {
        for x in 0..ow {
            let (sx0, sy0) = ((x as f32 * scale) as u32, (y as f32 * scale) as u32);
            let (sx1, sy1) = (
                (((x + 1) as f32 * scale) as u32).min(w).max(sx0 + 1),
                (((y + 1) as f32 * scale) as u32).min(h).max(sy0 + 1),
            );
            let mut acc = [0f32; 4];
            let mut n = 0f32;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let i = ((sy * w + sx) * 4) as usize;
                    let a = data[i + 3] as f32;
                    for k in 0..3 {
                        acc[k] += data[i + k] as f32 * a;
                    }
                    acc[3] += a;
                    n += 1.0;
                }
            }
            let (px, py) = (x + ox, y + oy);
            if px >= SIZE || py >= SIZE {
                continue;
            }
            let i = ((py * SIZE + px) * 4) as usize;
            if acc[3] > 0.0 {
                for k in 0..3 {
                    c.px[i + k] = (acc[k] / acc[3]).round() as u8;
                }
            }
            c.px[i + 3] = (acc[3] / n).round() as u8;
        }
    }
    Some(())
}

/// Готовая иконка RGBA 32×32: собачка слева снизу, статус — плашкой справа сверху; во время проверки статус не
/// меняется, а слева сверху появляется синяя точка (v0.6).
pub fn render(badge: Badge, color: Color, checking: bool) -> Vec<u8> {
    let mut c = base();
    let col = match badge {
        Badge::Unknown | Badge::Captive | Badge::Paused | Badge::NoInternet => GRAY,
        _ => color_rgba(color),
    };
    // плашка статуса: x 13..31, y 1..18
    let (bx, by, bw, bh) = (13.0, 1.0, 18.0, 17.0);
    match badge {
        Badge::Score(n) => {
            let text = format!("{:02}", n.clamp(1, 99));
            c.round_rect(bx, by, bw, bh, 4.0, col);
            let mut x = 15;
            for ch in text.chars() {
                c.glyph(ch, x, 4, 2, WHITE);
                x += 8;
            }
        }
        Badge::WeakInternet(bars) => {
            c.round_rect(bx, by, bw, bh, 4.0, col);
            let (cx, cy) = (22.0, 15.0);
            c.round_rect(cx - 1.5, cy - 2.0, 3.0, 3.0, 1.5, WHITE);
            if bars >= 2 {
                c.ring_arc(cx, cy, 5.0, 2.0, 215.0, 325.0, WHITE);
            }
            if bars >= 3 {
                c.ring_arc(cx, cy, 9.0, 2.0, 220.0, 320.0, WHITE);
            }
        }
        Badge::Unknown => {
            c.round_rect(17.0, 3.0, 12.0, 12.0, 6.0, col);
        }
        _ => {
            let ch = match badge {
                Badge::NoVpn => '!',
                Badge::Captive => '?',
                Badge::Paused => '|',
                _ => 'x',
            };
            c.round_rect(bx, by, bw, bh, 4.0, col);
            c.glyph(ch, 19, 4, 2, WHITE);
        }
    }
    if checking {
        // синяя точка «идёт проверка»
        c.round_rect(1.0, 1.0, 8.0, 8.0, 4.0, [0x2f, 0x7c, 0xf6, 0xff]);
    }
    c.px
}

/// RGBA 32×32 → `.ico` (PNG внутри) — фавикон окна в браузере, тот же, что иконка в трее (v0.6).
pub fn to_ico(px: &[u8]) -> Vec<u8> {
    let img = ico::IconImage::from_rgba_data(SIZE, SIZE, px.to_vec());
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    if let Ok(e) = ico::IconDirEntry::encode(&img) {
        dir.add_entry(e);
    }
    let mut out = Vec::new();
    let _ = dir.write(&mut out);
    out
}

/// RGBA → PNG (для предпросмотра иконок в тестах и отладке).
pub fn to_ppm(px: &[u8]) -> Vec<u8> {
    let mut out = format!("P6 {SIZE} {SIZE} 255\n").into_bytes();
    for p in px.chunks(4) {
        let a = p[3] as u32;
        for k in 0..3 {
            out.push(((p[k] as u32 * a + 255 * (255 - a)) / 255) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_badges() {
        for (b, c) in [
            (Badge::Score(7), Color::Green),
            (Badge::Score(99), Color::Yellow),
            (Badge::NoVpn, Color::Red),
            (Badge::WeakInternet(2), Color::Yellow),
            (Badge::Unknown, Color::Red),
            (Badge::NoInternet, Color::Gray),
            (Badge::Captive, Color::Gray),
            (Badge::Paused, Color::Gray),
        ] {
            let px = render(b, c, false);
            assert_eq!(px.len(), (SIZE * SIZE * 4) as usize);
            assert!(px.chunks(4).any(|p| p[3] > 0));
        }
        // предпросмотр: ICON_PREVIEW=папка cargo test renders_all_badges — PPM каждой иконки
        if let Ok(dir) = std::env::var("ICON_PREVIEW") {
            for (i, (b, c, ch)) in [
                (Badge::Score(84), Color::Green, false),
                (Badge::Score(61), Color::Yellow, true),
                (Badge::NoVpn, Color::Red, false),
                (Badge::WeakInternet(2), Color::Yellow, false),
                (Badge::NoInternet, Color::Gray, true),
                (Badge::Paused, Color::Gray, false),
            ]
            .into_iter()
            .enumerate()
            {
                let _ = std::fs::write(format!("{dir}/icon{i}.ppm"), to_ppm(&render(b, c, ch)));
            }
        }
        assert_eq!(
            render(Badge::Unknown, Color::Red, true).len(),
            (SIZE * SIZE * 4) as usize
        );
    }
}
