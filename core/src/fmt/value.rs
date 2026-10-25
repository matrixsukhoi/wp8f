/// 有效数字格式化，**直接写进 `out`**（零分配：`FixedBytes` 就是 `fmt::Write`）。
///
/// 热路径（每帧的 HUD 文本）用这个；需要 `String` 的冷路径用 [`format_sig_figs`]。
pub fn write_sig_figs(
    out: &mut impl std::fmt::Write,
    value: f64,
    sigfigs: usize,
) -> std::fmt::Result {
    if value <= 0.0 {
        return out.write_str("-");
    }
    let magnitude = value.abs().log10().floor() as i32;
    let factor = 10_f64.powi(magnitude - sigfigs as i32 + 1);
    write!(out, "{:.0}", (value / factor).round() * factor)
}

/// 小数位数随量级自适应（同一量级的数显示到差不多的精度）：`prec = (base - floor(log10|v|))`，下限 0。
///
/// `v <= 0` 或非有限值 → `"-"`：`log10(0)` = -inf、`log10(inf)` = inf，直接 `as i32` 会饱和成
/// `i32::MIN/MAX`，`base - 饱和值` 在 debug 下整数溢出 panic。
pub fn write_adaptive(
    out: &mut impl std::fmt::Write,
    v: f64,
    base_decimals: i32,
) -> std::fmt::Result {
    if !v.is_finite() || v <= 0.0 {
        return out.write_str("-");
    }
    let prec = (base_decimals - v.abs().log10().floor() as i32).max(0) as usize;
    write!(out, "{:.prec$}", v, prec = prec)
}

/// [`write_adaptive`] 的 `String` 版本（冷路径：启动报告；热路径请用 `write_adaptive`）。
pub fn format_adaptive(v: f64, base_decimals: i32) -> String {
    let mut out = String::new();
    let _ = write_adaptive(&mut out, v, base_decimals);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(v: f64, n: usize) -> String {
        let mut s = String::new();
        write_sig_figs(&mut s, v, n).unwrap();
        s
    }

    fn adapt(v: f64, base: i32) -> String {
        let mut s = String::new();
        write_adaptive(&mut s, v, base).unwrap();
        s
    }

    /// 钉住输出；顺带证明 `String` 包装与 `write_*` 逐字节一致（冷/热路径不许分叉）。
    #[test]
    fn write_variants_output_expected_text() {
        assert_eq!(sig(1234.5, 3), "1230", "3 位有效数字");
        assert_eq!(sig(12345.6, 2), "12000");
        assert_eq!(sig(0.0, 3), "-", "非正值 → 占位符");
        assert_eq!(sig(-5.0, 2), "-");
        assert_eq!(adapt(1.234, 3), "1.234");
        assert_eq!(adapt(1234.0, 3), "1234", "量级到万位就没有小数了");
        assert_eq!(adapt(0.0, 3), "-");
        assert_eq!(adapt(f64::INFINITY, 3), "-", "非有限值不能进 log10");
        assert_eq!(format_adaptive(1.234, 3), adapt(1.234, 3));
    }
}
