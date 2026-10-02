pub fn format_sig_figs(value: f64, sigfigs: usize) -> String {
    if value <= 0.0 {
        return "-".to_string();
    }
    let magnitude = value.abs().log10().floor() as i32;
    let factor = 10_f64.powi(magnitude - sigfigs as i32 + 1);
    let rounded = (value / factor).round() * factor;

    format!("{:.0}", rounded)
}

/// 小数位数随量级自适应（同一量级的数显示到差不多的精度）：`prec = (base - floor(log10|v|))`，下限 0。
///
/// `v <= 0` 或非有限值 → `"-"`：`log10(0)` = -inf、`log10(inf)` = inf，直接 `as i32` 会饱和成
/// `i32::MIN/MAX`，`base - 饱和值` 在 debug 下整数溢出 panic。
pub fn format_adaptive(v: f64, base_decimals: i32) -> String {
    if !v.is_finite() || v <= 0.0 {
        return "-".to_string();
    }
    let prec = (base_decimals - v.abs().log10().floor() as i32).max(0) as usize;
    format!("{:.prec$}", v, prec = prec)
}
