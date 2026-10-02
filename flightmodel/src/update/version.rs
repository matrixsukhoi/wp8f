//! FM 数据库版本号：**按 `.` 分段做数值比较**（D10），不假设等差。
//!
//! 上游 tag 不连续（G3 实测缺 `.39`/`.40`），所以：
//! * 只回答"新 / 旧 / 相同"，**不做**"差多少个版本 / 还差几版"这类推断；
//! * 段数与数值都比较，"缺段当 0"（`2.59.0` 与 `2.59.0.0` 视为同一个版本，
//!   而 `2.59.0` < `2.59.0.43`）；
//! * 段里出现非数字（`2.59.0-rc1`）→ 解析失败，调用方如实报"版本号格式不认识"，
//!   **绝不**退化成字符串比较（`2.9.0` < `2.10.0` 会判反）。

use std::cmp::Ordering;

/// 一个可比较的版本号（原始文本 + 数值分段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    raw: String,
    segs: Vec<u64>,
}

impl Version {
    /// 解析 `2.58.0.35` 这类版本号。空白会被去掉；空串、超长串、含非数字段 → `None`。
    pub fn parse(text: &str) -> Option<Version> {
        let raw = text.trim();
        if raw.is_empty() || raw.len() > 64 {
            return None;
        }
        let mut segs = Vec::new();
        for part in raw.split('.') {
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            // 单段过长（>9 位）多半不是版本号：按解析失败处理，免得 u64 溢出/无意义比较
            if part.len() > 9 {
                return None;
            }
            segs.push(part.parse::<u64>().ok()?);
        }
        if segs.is_empty() {
            return None;
        }
        Some(Version { raw: raw.to_string(), segs })
    }

    /// 原始文本（登录日志/界面显示用）。
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// 数值分段（`2.58.0.35` → `[2, 58, 0, 35]`）。
    pub fn segments(&self) -> &[u64] {
        &self.segs
    }

    /// 与另一个版本比较：逐段数值比较，缺段当 0。
    pub fn cmp_version(&self, other: &Version) -> Ordering {
        let n = self.segs.len().max(other.segs.len());
        for i in 0..n {
            let a = self.segs.get(i).copied().unwrap_or(0);
            let b = other.segs.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                Ordering::Equal => continue,
                ord => return ord,
            }
        }
        Ordering::Equal
    }

    /// 本地版本相对远端：`Less` = 有新版可下。
    pub fn cmp_str(&self, other: &str) -> Option<Ordering> {
        Version::parse(other).map(|o| self.cmp_version(&o))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

/// 便捷入口：比较两个版本串。任一解析失败 → `None`（调用方按"无法判断"处理）。
pub fn compare(local: &str, remote: &str) -> Option<Ordering> {
    let l = Version::parse(local)?;
    let r = Version::parse(remote)?;
    Some(l.cmp_version(&r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::{Equal, Greater, Less};

    #[test]
    fn numeric_segments_not_string_order() {
        // 字符串比较会把 "2.9.0" 判成大于 "2.10.0" —— 这正是不能用字符串比较的原因
        assert_eq!(compare("2.10.0", "2.9.0"), Some(Greater));
        assert_eq!(compare("2.9.0", "2.10.0"), Some(Less));
        assert_eq!(compare("2.58.0.35", "2.59.0.43"), Some(Less));
        assert_eq!(compare("2.59.0.43", "2.58.0.35"), Some(Greater));
    }

    #[test]
    fn missing_segments_are_zero_but_longer_wins_when_nonzero() {
        // 上游 tag 不连续（缺 .39/.40）：只比大小，不管"差几版"
        assert_eq!(compare("2.59.0", "2.59.0.0"), Some(Equal), "补零等价、不算更新");
        assert_eq!(compare("2.59.0", "2.59.0.43"), Some(Less), "带有效尾段的是新版");
        assert_eq!(compare("2.59.0.43", "2.59"), Some(Greater));
        assert_eq!(compare("2.59.0.39", "2.59.0.43"), Some(Less), "上游缺号也不影响比较");
        assert_eq!(compare("2.59.0.43", "2.59.0.43"), Some(Equal));
    }

    #[test]
    fn big_segments_do_not_overflow() {
        assert_eq!(compare("2.999999999.0", "2.1000000000.0"), None, "10 位段视为非法");
        assert_eq!(compare("2026.10.1", "2026.9.30"), Some(Greater));
    }

    #[test]
    fn junk_is_rejected_not_silently_compared() {
        for bad in ["", "  ", "master", "2.59.0-rc1", "2..0", "2.59.0.43 ", "v2.59.0", "2.59.a"] {
            // 注意：'2.59.0.43 ' 前后空白会被 trim，是合法输入
            let expect_ok = bad == "2.59.0.43 ";
            assert_eq!(Version::parse(bad).is_some(), expect_ok, "输入 {bad:?}");
        }
        assert_eq!(compare("master", "2.59.0.43"), None);
        assert_eq!(compare("2.58.0.35", "master"), None);
    }

    #[test]
    fn parse_keeps_raw_and_segments() {
        let v = Version::parse(" 2.58.0.35\n").expect("合法");
        assert_eq!(v.as_str(), "2.58.0.35");
        assert_eq!(v.segments(), &[2, 58, 0, 35]);
        assert_eq!(v.to_string(), "2.58.0.35");
    }
}
