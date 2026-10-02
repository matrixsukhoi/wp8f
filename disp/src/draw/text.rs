pub fn draw_text_raw(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    font: &ab_glyph::FontRef,
    x: f32,
    y: f32,
    text: &str,
    size: f32,
    color: u32,
) {
    use ab_glyph::Font;
    use crate::draw::glyph_cache;

    let mut x_pos = x;

    for c in text.chars() {
        let glyph_id = font.glyph_id(c);
        let raster = glyph_cache::get_or_rasterize(font, glyph_id, size);
        glyph_cache::blit_cached_glyph(buffer, buf_width, buf_height, &raster, x_pos, y, color);
        let h_advance = font.h_advance_unscaled(glyph_id) * size / font.height_unscaled();
        x_pos += h_advance;
    }
}

pub fn measure_text_raw(font: &ab_glyph::FontRef, text: &str, size: f32) -> f32 {
    use ab_glyph::Font;
    let mut width = 0.0;
    for c in text.chars() {
        let glyph_id = font.glyph_id(c);
        width += font.h_advance_unscaled(glyph_id) * size / font.height_unscaled();
    }
    width
}
