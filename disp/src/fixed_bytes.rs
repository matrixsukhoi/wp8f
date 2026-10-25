//! 定长字节文本（`FixedBytes<N>`）：HUD 文本与记录里的"字符串"一律用它 ——
//! **定长、无堆**（环里要求 POD），`as_str()` / `assign()` / `write_fmt_str()` 都是零分配。
//!
//! 从 `lib.rs` 拆出来的第一块（lib.rs 原本 2500+ 行）。对外路径不变：
//! 外部仍然 `use wp8f_disp::FixedBytes;`（lib.rs 里有 `pub use fixed_bytes::FixedBytes;`）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct FixedBytes<const N: usize> {
    buf: [u8; N],
    len: u8,
}

impl<const N: usize> FixedBytes<N> {
    pub const fn new() -> Self {
        Self { buf: [0u8; N], len: 0 }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len as usize]
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(self.as_bytes()).unwrap_or("")
    }

    /// 拷贝成**定长数组**（不足补 0，超出按 `M` 截断）：给"字段必须是 `[u8; M]`"的 POD 结构用
    /// （`MapObjectDisplay` 的 `obj_type`/`icon` 就是这种）。
    ///
    /// 以前调用点写 `std::array::from_fn(|i| fb.as_bytes().get(i).copied().unwrap_or(0))` ——
    /// 每个元素一次边界检查 + 一次 `unwrap_or`，还看不出在干什么。
    pub fn to_array<const M: usize>(&self) -> [u8; M] {
        let mut out = [0u8; M];
        let n = self.as_bytes().len().min(M);
        out[..n].copy_from_slice(&self.as_bytes()[..n]);
        out
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn assign(&mut self, src: &[u8]) {
        let n = src.len().min(N - 1);
        self.buf[..n].copy_from_slice(&src[..n]);
        self.len = n as u8;
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 清空后写入格式化文本，并借出 `&str` —— **画 HUD 的热路径零分配**。
    ///
    /// 以前各处是 `&format!("…")`：每帧每个数值语法/分配/释放一次 `String`，只为拷几个字节。
    /// 现在：`buf.write_fmt_str(format_args!("H {:>5.0}", alt))`（返回的引用在下次调用前有效）。
    pub fn write_fmt_str(&mut self, args: std::fmt::Arguments<'_>) -> &str {
        self.clear();
        let _ = std::fmt::Write::write_fmt(self, args);
        self.as_str()
    }
}

impl<const N: usize> Default for FixedBytes<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> std::fmt::Write for FixedBytes<N> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let bytes = s.as_bytes();
        let remaining = (N - 1).saturating_sub(self.len as usize);
        let n = bytes.len().min(remaining);
        self.buf[self.len as usize..self.len as usize + n].copy_from_slice(&bytes[..n]);
        self.len += n as u8;
        Ok(())
    }
}

impl<const N: usize> Serialize for FixedBytes<N> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.as_bytes())
    }
}

impl<'de, const N: usize> Deserialize<'de> for FixedBytes<N> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FbVisitor<const M: usize>;
        impl<'de, const M: usize> serde::de::Visitor<'de> for FbVisitor<M> {
            type Value = FixedBytes<M>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "a byte array up to {} bytes", M)
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<FixedBytes<M>, E> {
                let mut fb = FixedBytes::new();
                fb.assign(v);
                Ok(fb)
            }
        }
        deserializer.deserialize_bytes(FbVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `FixedBytes::to_array`：不足补 0、超出按目标宽度截断（给 POD 结构体的定长字段用）。
    #[test]
    fn fixed_bytes_to_array_pads_and_truncates() {
        let mut short: FixedBytes<8> = FixedBytes::new();
        short.assign(b"ab");
        assert_eq!(short.to_array::<4>(), [b'a', b'b', 0, 0], "不足补 0");

        let mut long: FixedBytes<8> = FixedBytes::new();
        long.assign(b"abcdef");
        assert_eq!(long.to_array::<4>(), *b"abcd", "超出按目标宽度截断");
        assert_eq!(long.to_array::<8>(), [b'a', b'b', b'c', b'd', b'e', b'f', 0, 0]);
        assert_eq!(FixedBytes::<8>::new().to_array::<4>(), [0u8; 4]);
    }
}
