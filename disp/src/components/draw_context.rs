use ab_glyph::{Font, FontRef};
use std::borrow::Cow;

use super::theme::RgbaColor;
use super::text_scale::TextScale;

#[derive(Clone)]
pub struct DrawItem<'a> {
    pub label: Cow<'a, str>,
    pub value: Cow<'a, str>,
    pub unit: Cow<'a, str>,
    pub color: Option<RgbaColor>,
    pub data_width: f32,
    pub label_width: f32,
}

impl<'a> DrawItem<'a> {
    pub fn new(label: impl Into<Cow<'a, str>>, value: impl Into<Cow<'a, str>>, unit: impl Into<Cow<'a, str>>, data_width: f32, label_width: f32) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            unit: unit.into(),
            color: None,
            data_width,
            label_width,
        }
    }

    pub fn with_color(mut self, color: RgbaColor) -> Self {
        self.color = Some(color);
        self
    }

    pub fn empty(data_width: f32, label_width: f32) -> Self {
        Self {
            label: Cow::Borrowed(""),
            value: Cow::Borrowed(""),
            unit: Cow::Borrowed(""),
            color: None,
            data_width,
            label_width,
        }
    }
}

impl<'a> DrawItem<'a> {
    pub fn width(&self) -> f32 {
        self.data_width + self.label_width
    }

    pub fn height(&self, scale: &TextScale) -> f32 {
        scale.label_size() + scale.unit_size()
    }
}

pub fn grid_size<'a>(items: &[DrawItem<'a>], rows: usize, cols: usize,
                     padding: f32, h_spacing: f32, v_spacing: f32, scale: &TextScale) -> (f32, f32) {
    if items.is_empty() {
        return (0.0, 0.0);
    }
    let item_w = items[0].width();
    let w = padding * 2.0 + item_w * cols as f32 + h_spacing * (cols as f32 - 1.0);
    let item_h = items[0].height(scale);
    let h = padding * 2.0 + item_h * rows as f32 + v_spacing * (rows as f32 - 1.0);
    (w, h)
}

pub fn draw_grid<'a>(items: &[DrawItem<'a>], _rows: usize, cols: usize,
                     ctx: &mut DrawContext, x: f32, y: f32,
                     padding: f32, h_spacing: f32, v_spacing: f32, scale: &TextScale) {
    let l = crate::hud_layout();
    let label_color = RgbaColor::from_argb(l.label_color);
    let unit_color = RgbaColor::from_argb(l.unit_color);
    let default_data_color = RgbaColor::from_argb(l.data_color);

    for (i, item) in items.iter().enumerate() {
        let row = i / cols;
        let col = i % cols;
        let item_x = x + padding + col as f32 * (item.width() + h_spacing);
        let item_y = y + padding + row as f32 * (item.height(scale) + v_spacing);

        let color = item.color.unwrap_or(default_data_color);

        let data_text_width = ctx.measure_text(&item.value, scale.data_size());
        let data_x = item_x + item.data_width - data_text_width;
        ctx.draw_text(data_x, item_y + scale.label_size(), &item.value, scale.data_size(), color);

        let label_text_width = ctx.measure_text(&item.label, scale.label_size());
        let label_x = item_x + item.data_width + item.label_width - label_text_width;
        ctx.draw_text(label_x, item_y, &item.label, scale.label_size(), label_color);

        let unit_text_width = ctx.measure_text(&item.unit, scale.unit_size());
        let unit_x = item_x + item.data_width + item.label_width - unit_text_width;
        ctx.draw_text(unit_x, item_y + scale.label_size(), &item.unit, scale.unit_size(), unit_color);
    }
}

pub struct DrawContext<'a> {
    pub buffer: &'a mut [u32],
    pub width: usize,
    pub height: usize,
    pub font: &'a FontRef<'a>,
}

impl<'a> DrawContext<'a> {
    pub fn new(buffer: &'a mut [u32], width: usize, height: usize, font: &'a FontRef) -> Self {
        Self {
            buffer,
            width,
            height,
            font,
        }
    }

    fn set_pixel(&mut self, x: i32, y: i32, color: u32) {
        if x >= 0 && x < self.width as i32 && y >= 0 && y < self.height as i32 {
            let idx = (y as usize) * self.width + (x as usize);
            if idx < self.buffer.len() {
                self.buffer[idx] = color;
            }
        }
    }

    pub fn draw_text(&mut self, x: f32, y: f32, text: &str, size: f32, color: RgbaColor) {
        use ab_glyph::Font;
        use crate::draw::glyph_cache;

        let argb = color.to_argb();

        let mut x_pos = x;

        for c in text.chars() {
            let glyph_id = self.font.glyph_id(c);
            let raster = glyph_cache::get_or_rasterize(self.font, glyph_id, size);
            glyph_cache::blit_cached_glyph(self.buffer, self.width, self.height, &raster, x_pos, y, argb);
            let h_advance =
                self.font.h_advance_unscaled(glyph_id) * size / self.font.height_unscaled();
            x_pos += h_advance;
        }
    }

    pub fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, color: RgbaColor, width: f32) {
        let dx = (x2 - x1).abs();
        let dy = (y2 - y1).abs();
        let sx = if x1 < x2 { 1.0 } else { -1.0 };
        let sy = if y1 < y2 { 1.0 } else { -1.0 };
        let mut err = if dx > dy { dx } else { -dy } / 2.0;
        let mut x = x1;
        let mut y = y1;
        let half_width = (width / 2.0).ceil() as i32;

        let argb = color.to_argb();

        loop {
            for wy in -half_width..=half_width {
                for wx in -half_width..=half_width {
                    self.set_pixel(x as i32 + wx, y as i32 + wy, argb);
                }
            }
            if (x - x2).abs() < 1.0 && (y - y2).abs() < 1.0 {
                break;
            }
            let e2 = 2.0 * err;
            if e2 > -dx {
                err -= dx;
                x += sx;
            }
            if e2 < dy {
                err += dy;
                y += sy;
            }
        }
    }

    pub fn measure_text(&self, text: &str, size: f32) -> f32 {
        let mut width = 0.0;
        for c in text.chars() {
            let glyph_id = self.font.glyph_id(c);
            width += self.font.h_advance_unscaled(glyph_id) * size / self.font.height_unscaled();
        }
        width
    }

    pub fn draw_rect_fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: RgbaColor) {
        let argb = color.to_argb();
        let x0 = x as i32;
        let y0 = y as i32;
        let x1 = (x + w) as i32;
        let y1 = (y + h) as i32;

        let x0 = x0.max(0).min(self.width as i32);
        let y0 = y0.max(0).min(self.height as i32);
        let x1 = x1.max(0).min(self.width as i32);
        let y1 = y1.max(0).min(self.height as i32);

        for py in y0..y1 {
            let row_start = (py as usize) * self.width + (x0 as usize);
            let row_end = (py as usize) * self.width + (x1 as usize);
            for idx in row_start..row_end {
                self.buffer[idx] = argb;
            }
        }
    }

    pub fn draw_rect_outline(&mut self, x: f32, y: f32, w: f32, h: f32, color: RgbaColor) {
        let argb = color.to_argb();
        let x0 = x as i32;
        let y0 = y as i32;
        let x1 = (x + w) as i32;
        let y1 = (y + h) as i32;

        let x0 = x0.max(0).min(self.width as i32);
        let y0 = y0.max(0).min(self.height as i32);
        let x1 = x1.max(0).min(self.width as i32);
        let y1 = y1.max(0).min(self.height as i32);

        // Top edge
        if y0 < self.height as i32 {
            let row_start = (y0 as usize) * self.width + (x0 as usize);
            let row_end = (y0 as usize) * self.width + (x1 as usize);
            for idx in row_start..row_end { self.buffer[idx] = argb; }
        }
        // Bottom edge
        if y1 - 1 >= 0 && y1 - 1 < self.height as i32 {
            let row_start = ((y1 - 1) as usize) * self.width + (x0 as usize);
            let row_end = ((y1 - 1) as usize) * self.width + (x1 as usize);
            for idx in row_start..row_end { self.buffer[idx] = argb; }
        }
        // Left edge
        if x0 < self.width as i32 {
            for py in (y0 + 1)..(y1 - 1) {
                if py >= 0 && py < self.height as i32 {
                    let idx = (py as usize) * self.width + (x0 as usize);
                    self.buffer[idx] = argb;
                }
            }
        }
        // Right edge
        if x1 - 1 >= 0 && x1 - 1 < self.width as i32 {
            for py in (y0 + 1)..(y1 - 1) {
                if py >= 0 && py < self.height as i32 {
                    let idx = (py as usize) * self.width + (x1 as usize - 1);
                    self.buffer[idx] = argb;
                }
            }
        }
    }
}
