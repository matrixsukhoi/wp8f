//! HUD 显示模式（配置键 `hud_type`）：`0` 关闭 / `1` MiniHUD / `2` 圆环。
//!
//! 数字与旧字符串（`off`/`mini`/`circle`）两种写法都接受，保存回文件时统一写数字。
//!
//! 从 `lib.rs` 拆出的第二块；对外路径不变（`lib.rs` 有 `pub use hud_type::{HudType, default_hud_type};`）。

use serde::{Deserialize, Serialize};

/// HUD 显示模式（配置键 `hud_type`）：
///
/// | 值 | 含义 |
/// |---|---|
/// | `0` | 关闭 HUD（只画飞行/发动机/地图面板，不画圆环也不画 miniHUD） |
/// | `1` | MiniHUD（紧凑面板） |
/// | `2` | CircleHUD（圆环） |
///
/// 配置里**写数字**（缺键时按 [`default_hud_type`]）；为兼容旧配置也接受
/// `"off"` / `"mini"` / `"circle"` 三种字符串写法，保存回文件时统一写成数字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudType {
    Off,
    Mini,
    Circle,
}

impl HudType {
    pub const fn as_u8(self) -> u8 {
        match self {
            HudType::Off => 0,
            HudType::Mini => 1,
            HudType::Circle => 2,
        }
    }

    /// 数字 → 模式：`0/1` 之外的一切都当圆环（越界值不该让 HUD 整屏空白）。
    pub const fn from_u8(v: u8) -> Self {
        match v {
            0 => HudType::Off,
            1 => HudType::Mini,
            _ => HudType::Circle,
        }
    }
}

impl std::str::FromStr for HudType {
    type Err = String;

    /// 字符串口径：数字 `0|1|2` 与旧配置里的 `off|mini|circle`（大小写不敏感）。
    /// 只服务于 `Deserialize` —— **没有对应的命令行参数**（HUD 类型只从配置读）。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "none" => Ok(HudType::Off),
            "1" | "mini" | "minihud" => Ok(HudType::Mini),
            "2" | "circle" | "circlehud" => Ok(HudType::Circle),
            other => Err(format!("HUD 类型 '{other}' 不认识：用 0=关闭 / 1=MiniHUD / 2=CircleHUD")),
        }
    }
}

impl Serialize for HudType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u8(self.as_u8())
    }
}

impl<'de> Deserialize<'de> for HudType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct NumOrName;
        impl serde::de::Visitor<'_> for NumOrName {
            type Value = HudType;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("HUD 类型：0/1/2（或 \"off\"/\"mini\"/\"circle\"）")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<HudType, E> {
                Ok(HudType::from_u8(v.min(u8::MAX as u64) as u8))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<HudType, E> {
                Ok(HudType::from_u8(v.clamp(0, u8::MAX as i64) as u8))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<HudType, E> {
                v.parse().map_err(E::custom)
            }
        }
        d.deserialize_any(NumOrName)
    }
}

pub fn default_hud_type() -> HudType {
    HudType::Mini
}
