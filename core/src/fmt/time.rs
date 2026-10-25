/// `mm'ss` 时长，**直接写进 `out`**（零分配：热路径每帧要写两次燃油时间）。
pub fn write_duration(out: &mut impl std::fmt::Write, total_secs: f64) -> std::fmt::Result {
    if !total_secs.is_finite() || total_secs < 0.0 {
        return out.write_str("-");
    }
    let mins = (total_secs / 60.0).floor() as u32;
    let secs = (total_secs % 60.0) as u32;
    write!(out, "{}'{:02.0}", mins, secs)
}

/// [`write_duration`] 的测试在下面；不再提供 `String` 版本（热路径统一走 `write_duration`）。
#[cfg(test)]
mod tests {
    use super::*;

    fn dur(secs: f64) -> String {
        let mut s = String::new();
        write_duration(&mut s, secs).unwrap();
        s
    }

    #[test]
    fn write_duration_outputs_expected_text() {
        assert_eq!(dur(0.0), "0'00");
        assert_eq!(dur(59.9), "0'59");
        assert_eq!(dur(600.0), "10'00");
        assert_eq!(dur(3661.0), "61'01");
        assert_eq!(dur(-1.0), "-", "负值 → 占位符");
        assert_eq!(dur(f64::INFINITY), "-");
        assert_eq!(dur(f64::NAN), "-");
    }
}
