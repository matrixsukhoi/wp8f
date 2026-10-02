pub fn format_duration(total_secs: f64) -> String {
    if total_secs.is_infinite() || total_secs.is_nan() {
        return "-".to_string();
    }
    if total_secs < 0.0 {
        return "-".to_string();
    }
    let mins = (total_secs / 60.0).floor() as u32;
    let secs = (total_secs % 60.0) as u32;
    format!("{}'{:02.0}", mins, secs)
}
