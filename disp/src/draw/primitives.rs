use crate::hud_common::blend_pixel_cov;

pub fn clear_rect(buffer: &mut [u32], buf_width: usize, buf_height: usize, x: f32, y: f32, w: f32, h: f32) {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = (x + w).ceil() as i32;
    let y1 = (y + h).ceil() as i32;

    let x0 = x0.max(0).min(buf_width as i32);
    let y0 = y0.max(0).min(buf_height as i32);
    let x1 = x1.max(0).min(buf_width as i32);
    let y1 = y1.max(0).min(buf_height as i32);

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    for py in y0..y1 {
        let row_start = (py as usize) * buf_width + (x0 as usize);
        let row_end = (py as usize) * buf_width + (x1 as usize);
        buffer[row_start..row_end].fill(0x00000000);
    }
}

#[inline]
pub fn set_pixel(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    x: i32,
    y: i32,
    color: u32,
) {
    if x >= 0 && y >= 0 && x < buf_width as i32 && y < buf_height as i32 {
        let idx = y as usize * buf_width + x as usize;
        if idx < buffer.len() {
            buffer[idx] = color;
        }
    }
}





/// Bresenham 走线：`plot` 决定每个像素怎么落（直接写 / 按覆盖率混合）。
fn walk_line(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    mut plot: impl FnMut(&mut [u32], usize, usize, i32, i32),
) {
    let dx = (x2 - x1).abs();
    let dy = (y2 - y1).abs();
    let sx = if x1 < x2 { 1 } else { -1 };
    let sy = if y1 < y2 { 1 } else { -1 };
    let mut err = if dx > dy { dx } else { -dy } / 2;
    let mut x = x1;
    let mut y = y1;

    loop {
        plot(buffer, buf_width, buf_height, x, y);
        if x == x2 && y == y2 {
            break;
        }
        let e2 = err;
        if e2 > -dx {
            err -= dy;
            x += sx;
        }
        if e2 < dy {
            err += dx;
            y += sy;
        }
    }
}

pub fn draw_line(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    color: u32,
) {
    walk_line(buffer, buf_width, buf_height, x1, y1, x2, y2, |b, w, h, x, y| {
        set_pixel(b, w, h, x, y, color)
    });
}

/// 半透明线：与 [`draw_line`] 同一条走线，但每像素按覆盖率混合（颜色 alpha < 255 时才有差别）。
pub fn draw_line_semi(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    color: u32,
) {
    walk_line(buffer, buf_width, buf_height, x1, y1, x2, y2, |b, w, h, x, y| {
        blend_pixel_cov(b, w, h, x, y, color, 1.0)
    });
}



pub fn draw_x_marker(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    cx: i32,
    cy: i32,
    size: i32,
    color: u32,
) {
    for i in -size..=size {
        set_pixel(buffer, buf_width, buf_height, cx + i, cy + i, color);
        set_pixel(buffer, buf_width, buf_height, cx + i, cy - i, color);
    }
}

pub fn draw_rectangle_semi(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    cx: i32,
    cy: i32,
    w: i32,
    h: i32,
    color: u32,
) {
    for dy in -h..=h {
        for dx in -w..=w {
            blend_pixel_cov(buffer, buf_width, buf_height, cx + dx, cy + dy, color, 1.0);
        }
    }
}

pub fn draw_runway_semi(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    half_w: i32,
    color: u32,
) {
    let dx = (x2 - x1) as f32;
    let dy = (y2 - y1) as f32;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1.0 {
        return;
    }
    let len = len_sq.sqrt();
    let ux = dx / len;
    let uy = dy / len;

    let min_x = (x1.min(x2) - half_w).max(0);
    let max_x = (x1.max(x2) + half_w).min(buf_width as i32 - 1);
    let min_y = (y1.min(y2) - half_w).max(0);
    let max_y = (y1.max(y2) + half_w).min(buf_height as i32 - 1);

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = (x - x1) as f32;
            let py = (y - y1) as f32;
            let t = px * ux + py * uy;
            if t < 0.0 || t > len {
                continue;
            }
            let proj_x = t * ux;
            let proj_y = t * uy;
            let perp_dist = ((px - proj_x).powi(2) + (py - proj_y).powi(2)).sqrt();
            if perp_dist <= half_w as f32 {
                blend_pixel_cov(buffer, buf_width, buf_height, x, y, color, 1.0);
            }
        }
    }
}

pub fn draw_rotated_square_with_cross_semi(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    cx: i32,
    cy: i32,
    size: i32,
    heading_deg: f32,
    color: u32,
) {
    let angle = -heading_deg * std::f32::consts::PI / 180.0;
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    for dy in -size..=size {
        for dx in -size..=size {
            if dx == -size || dx == size || dy == -size || dy == size {
                let rx = dx as f32 * cos_a - dy as f32 * sin_a;
                let ry = dx as f32 * sin_a + dy as f32 * cos_a;
                blend_pixel_cov(
                    buffer,
                    buf_width,
                    buf_height,
                    cx + rx as i32,
                    cy + ry as i32,
                    color,
                    1.0,
                );
            }
        }
    }

    let line_size = size * 3 / 2;
    let (x1, y1) = (-line_size as f32, 0.0);
    let (x2, y2) = (line_size as f32, 0.0);

    let rx1 = x1 * cos_a - y1 * sin_a;
    let ry1 = x1 * sin_a + y1 * cos_a;
    let rx2 = x2 * cos_a - y2 * sin_a;
    let ry2 = x2 * sin_a + y2 * cos_a;

    draw_line_semi(
        buffer,
        buf_width,
        buf_height,
        cx + rx1 as i32,
        cy + ry1 as i32,
        cx + rx2 as i32,
        cy + ry2 as i32,
        color,
    );
}
