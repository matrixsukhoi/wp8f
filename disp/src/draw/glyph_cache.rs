use ab_glyph::{point, Font, FontRef, GlyphId, PxScale};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

pub struct CachedGlyph {
    pub width: i32,
    pub height: i32,
    pub coverage: Vec<u8>,
    pub bounds_min_y: f32,
}

thread_local! {
    static GLYPH_CACHE: RefCell<HashMap<(usize, u16, u32), Arc<CachedGlyph>>> =
        RefCell::new(HashMap::with_capacity(1024));
}

fn size_key(size: f32) -> u32 {
    (size * 2.0).round() as u32
}

pub fn get_or_rasterize(font: &FontRef, glyph_id: GlyphId, size: f32) -> Arc<CachedGlyph> {
    let font_ptr = Font::font_data(font).as_ptr() as usize;
    let key = (font_ptr, glyph_id.0, size_key(size));

    if let Some(cached) = GLYPH_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return cached;
    }

    let cached = rasterize(font, glyph_id, size);
    let arc = Arc::new(cached);

    GLYPH_CACHE.with(|c| {
        c.borrow_mut().insert(key, arc.clone());
    });

    arc
}

fn rasterize(font: &FontRef, glyph_id: GlyphId, size: f32) -> CachedGlyph {
    use ab_glyph::Glyph;

    let scale = PxScale::from(size);
    let glyph = Glyph {
        id: glyph_id,
        scale,
        position: point(0.0, 0.0),
    };

    match font.outline_glyph(glyph) {
        Some(outlined) => {
            let bounds = outlined.px_bounds();
            let w = bounds.width() as i32;
            let h = bounds.height() as i32;

            if w <= 0 || h <= 0 {
                return CachedGlyph {
                    width: 0,
                    height: 0,
                    coverage: Vec::new(),
                    bounds_min_y: 0.0,
                };
            }

            let mut coverage = vec![0u8; (w * h) as usize];
            let w_u32 = w as u32;

            outlined.draw(|px, py, cov| {
                let idx = (py * w_u32 + px) as usize;
                if idx < coverage.len() {
                    coverage[idx] = (cov * 255.0).min(255.0) as u8;
                }
            });

            CachedGlyph {
                width: w,
                height: h,
                coverage,
                bounds_min_y: bounds.min.y,
            }
        }
        None => CachedGlyph {
            width: 0,
            height: 0,
            coverage: Vec::new(),
            bounds_min_y: 0.0,
        },
    }
}

/// 把缓存字形按覆盖率混进缓冲区（字形渲染的最后一步）。
///
/// 按覆盖率**同时衰减 RGB 和 alpha**（不做 alpha 预乘）：
/// ```text
/// c = 覆盖率（0..=255）；bc = 255 - c
/// out_ch = (配置RGB × c + 背景ch × bc) / 255     // R/G/B 同式
/// out_a  = (配置A   × c + 背景A  × bc) / 255
/// ```
/// 于是：字心（c = 255）逐位等于配置值（"配置 RGBA == 落屏 RGBA"的契约），
/// 边缘 RGB 与 alpha 一起衰减、背景按 (1 - c) 透回，轮廓柔和。
///
/// ⚠️ **不要改回 `hud_common::blend_pixel_cov`**（那是直通 alpha：覆盖率只写进 alpha，
/// 边缘的 RGB 与字心一样实，字会变粗变硬），也不要在这里做 alpha 预乘。
///
/// ⚠️ config 里 5 个文字色是"偏暗 + alpha 偏小"的**补偿值**（`新 RGB = 旧 RGB × 旧 A / 255`、
/// `新 A = 旧 A × 旧 A / 255`）：本函数的覆盖率衰减与那组补偿值配套才等于旧观感，
/// 只改一半就会变亮/变实。别"顺手修正"配置里的色值。
///
/// 边界：`width/height <= 0` 直接返回、覆盖率为 0 的像素跳过、越界像素裁剪丢弃。
pub fn blit_cached_glyph(
    buffer: &mut [u32],
    buf_width: usize,
    buf_height: usize,
    glyph: &CachedGlyph,
    cursor_x: f32,
    baseline_y: f32,
    color: u32,
) {
    if glyph.width <= 0 || glyph.height <= 0 {
        return;
    }

    // 配置色按 straight 用：**不做** alpha 预乘（预乘会让半覆盖处更暗、alpha 再自乘一次）
    let src_a = (color >> 24) & 0xFF;
    let src_r = (color >> 16) & 0xFF;
    let src_g = (color >> 8) & 0xFF;
    let src_b = color & 0xFF;

    let y_origin = (baseline_y + glyph.bounds_min_y) as i32;
    let x_origin = cursor_x as i32;
    let w = glyph.width as usize;
    let h = glyph.height as usize;

    for py in 0..h {
        let dest_y = y_origin + py as i32;
        if dest_y < 0 || dest_y >= buf_height as i32 {
            continue;
        }
        let row_base = py * w;
        let buf_row = (dest_y as usize) * buf_width;

        for px in 0..w {
            let c = glyph.coverage[row_base + px] as u32;
            // 覆盖率为 0 的像素完全不动背景
            if c == 0 {
                continue;
            }
            let dest_x = x_origin + px as i32;
            if dest_x < 0 || dest_x >= buf_width as i32 {
                continue;
            }

            // 权重用的是**覆盖率 c**（不是 c×A）：字心 c=255 完全替换背景 → 落屏即配置值
            let bc = 255 - c;
            let idx = buf_row + dest_x as usize;
            let bg = buffer[idx];

            let out_b = (src_b * c + (bg & 0xFF) * bc) / 255;
            let out_g = (src_g * c + ((bg >> 8) & 0xFF) * bc) / 255;
            let out_r = (src_r * c + ((bg >> 16) & 0xFF) * bc) / 255;
            let out_a = (src_a * c + ((bg >> 24) & 0xFF) * bc) / 255;

            buffer[idx] = (out_b & 0xFF)
                | ((out_g & 0xFF) << 8)
                | ((out_r & 0xFF) << 16)
                | ((out_a & 0xFF) << 24);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 新 `data_color`：配置 `#3EC8659C`（R=3E G=C8 B=65 A=9C）→ 内部打包 `0x9C3EC865`。
    const DATA_COLOR: u32 = 0x9C_3E_C8_65;

    /// 5 个文字色：(迁移前配置的 ARGB 顺序值, 现在配置色的内部打包)，与 `config/*.json` 一一对应。
    const MIGRATED: [(u32, u32); 5] = [
        (0xC850_FF82, 0x9C_3E_C8_65), // data_color  #3EC8659C
        (0x6632_C85A, 0x28_14_50_24), // label_color #14502428
        (0x66A0_A0A0, 0x28_40_40_40), // unit_color  #40404028
        (0xA2FF_F03C, 0x66_A2_98_26), // hint_color  #A2982666
        (0xA2FF_3C28, 0x66_A2_26_19), // alert_color #A2261966
    ];

    /// 回归用的覆盖率档位（含两端：全透明、满覆盖）。
    const COVERAGES: [u32; 8] = [0, 1, 8, 64, 128, 200, 254, 255];

    /// 1×1 字形：`bounds_min_y = 0` → 基线即首行，光标 (0,0) 落在缓冲区左上角。
    fn one_px(cov: u8) -> CachedGlyph {
        CachedGlyph {
            width: 1,
            height: 1,
            coverage: vec![cov],
            bounds_min_y: 0.0,
        }
    }

    /// 单像素走一遍真正的字形管线（1×1 缓冲区）。
    fn blit(color: u32, cov: u8, bg: u32) -> u32 {
        let mut buf = vec![bg];
        blit_cached_glyph(&mut buf, 1, 1, &one_px(cov), 0.0, 0.0, color);
        buf[0]
    }

    /// 迁移前的字形混合（2026-10-01 前的 `blit_cached_glyph`，整数整除口径）：
    ///   fa = cov×A/255；ch = (src_ch×fa + bg_ch×(255-fa))/255；a = (A×fa + bg_a×(255-fa))/255
    fn old_pipeline(color: u32, cov: u32, bg: u32) -> u32 {
        let src_a = (color >> 24) & 0xFF;
        let fa = cov * src_a / 255;
        if fa == 0 {
            return bg;
        }
        let ba = 255 - fa;
        let ch = |src: u32, shift: u32| (src * fa + ((bg >> shift) & 0xFF) * ba) / 255;
        let a_ch = (src_a * fa + ((bg >> 24) & 0xFF) * ba) / 255;
        (ch(color & 0xFF, 0) & 0xFF)
            | ((ch((color >> 8) & 0xFF, 8) & 0xFF) << 8)
            | ((ch((color >> 16) & 0xFF, 16) & 0xFF) << 16)
            | ((a_ch & 0xFF) << 24)
    }

    /// **字心 = 配置值**：满覆盖（c=255）时权重全给字色 → 缓冲区**恰好**等于配置色，
    /// 四位逐位相等（背景是什么都不影响，包括不透明背景）。
    /// 这就是"配置里的 RGBA ↔ 屏幕上的 RGBA 一一对应"，5 个文字色逐个验。
    #[test]
    fn blit_color_corresponds_to_config() {
        let px = blit(DATA_COLOR, 255, 0x0000_0000);
        assert_eq!((px >> 16) & 0xFF, 0x3E, "R 必须等于配置里的 R");
        assert_eq!((px >> 8) & 0xFF, 0xC8, "G 必须等于配置里的 G");
        assert_eq!(px & 0xFF, 0x65, "B 必须等于配置里的 B");
        assert_eq!(px >> 24, 0x9C, "A 必须等于配置里的 A");
        assert_eq!(px, DATA_COLOR, "满覆盖 + 背景全透明 = 配置色原值");
        assert_eq!(
            blit(DATA_COLOR, 255, 0xFF11_2233),
            DATA_COLOR,
            "满覆盖时背景权重为 0，不透明背景也必须被完全替换"
        );
        for (_, new_internal) in MIGRATED {
            assert_eq!(blit(new_internal, 255, 0), new_internal, "满覆盖 = 配置值");
        }
    }

    /// **覆盖率**：RGB 与 alpha **一起**按 c/255 衰减（`A = 配置A×c/255`，RGB 同式）。
    /// data：A=0x9C=156、R=0x3E=62、G=0xC8=200、B=0x65=101，c=128 →
    ///   A=156×128/255=78=0x4E、R=62×128/255=31=0x1F、G=200×128/255=100=0x64、B=101×128/255=50=0x32。
    #[test]
    fn blit_alpha_scales_with_coverage() {
        assert_eq!(
            blit(DATA_COLOR, 128, 0x0000_0000),
            0x4E_1F_64_32,
            "c=128：A 与 RGB 都必须按覆盖率衰减"
        );
        // 覆盖率为 0：完全不动背景
        assert_eq!(blit(DATA_COLOR, 0, 0x0102_0304), 0x0102_0304, "覆盖率为 0 不应改背景");
    }

    /// **边缘必须比字心暗**（防回退护栏）：有人把这里改回 `blend_pixel_cov`（覆盖率只进 alpha、
    /// RGB 不衰减）时，抗锯齿边缘的 RGB 会与字心一样实 → 字看起来变粗变硬（用户实测反馈过的毛病）。
    #[test]
    fn blit_edge_is_darker_than_center() {
        let center = blit(DATA_COLOR, 255, 0x0000_0000);
        let edge = blit(DATA_COLOR, 128, 0x0000_0000);

        for (name, shift) in [("B", 0u32), ("G", 8), ("R", 16), ("A", 24)] {
            assert!(
                (edge >> shift) & 0xFF < (center >> shift) & 0xFF,
                "{name} 通道：c=128 的落屏值必须小于 c=255（边缘要比字心暗/淡）"
            );
        }
        assert_eq!((edge >> 16) & 0xFF, 31, "边缘 R 应为 配置R×128/255=31");
        assert_ne!(
            (edge >> 16) & 0xFF,
            0x3E,
            "边缘 R 绝不能等于配置原色 0x3E —— 那就是 RGB 不随覆盖率衰减的直通写法，边缘会变粗"
        );
    }

    /// **观感不变（关键回归，别删）**：5 个文字色 × 8 档覆盖率、背景全透明，
    /// 新管线与旧管线的落屏一致：两端 `c = 0 / 255` 逐位相同，其余档位每通道 ≤1/255
    /// （两处整除的取整次序不同）。**偏差变大 = 有人改了覆盖率口径或配置补偿值。**
    #[test]
    fn migrated_text_colors_keep_old_look() {
        // 与迁移前老管线差 1/255（二次取整）的 (MIGRATED 下标, 覆盖率) 组；其余必须逐位相同
        const INEXACT: [(usize, u32); 11] = [
            (0, 254), // data  #3EC8659C @254
            (1, 64),  // label #14502428 @64
            (1, 200), // label @200
            (1, 254), // label @254
            (2, 8),   // unit  #40404028 @8
            (2, 64),  // unit  @64
            (2, 200), // unit  @200
            (2, 254), // unit  @254
            (3, 64),  // hint  #A2982666 @64
            (3, 254), // hint  @254
            (4, 254), // alert #A2261966 @254
        ];

        let mut inexact: Vec<(usize, u32)> = Vec::new();
        for (i, (old_argb, new_internal)) in MIGRATED.iter().enumerate() {
            for c in COVERAGES {
                let want = old_pipeline(*old_argb, c, 0x0000_0000);
                let got = blit(*new_internal, c as u8, 0x0000_0000);

                for (name, shift) in [("B", 0u32), ("G", 8), ("R", 16), ("A", 24)] {
                    let dev = (((got >> shift) & 0xFF) as i32 - ((want >> shift) & 0xFF) as i32).abs();
                    assert!(
                        dev <= 1,
                        "色#{i} c={c} {name}：与迁移前老管线差 {dev}/255（应 ≤1；got={got:#010X} want={want:#010X}）"
                    );
                }
                if c == 0 || c == 255 {
                    assert_eq!(got, want, "色#{i} c={c}：两端必须逐位相同");
                }
                if got != want {
                    inexact.push((i, c));
                }
            }
        }
        assert_eq!(
            inexact,
            INEXACT.to_vec(),
            "与迁移前老管线的差异组变了 —— 覆盖率口径或配置补偿值被动过，需复核"
        );
    }
}
