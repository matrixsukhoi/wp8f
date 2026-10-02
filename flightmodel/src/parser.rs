//! FM 数据文本解析：**两套实现、一个出口**（`HashMap<String, BlkxValue>`）。
//!
//! 编译期用 cargo feature 选择（D12，用户拍板：不新建 crate、不用 workspace exclude）：
//!
//! | feature | 数据来源 | 文本形态 |
//! |---|---|---|
//! | `fm-json`（**默认**） | GitHub `War-Thunder-Datamine` | 纯文本 JSON（[`crate::json_adapter::parse_json_fm`]） |
//! | `fm-legacy`（仅备份，要显式打开） | 旧游戏解包 | `key:type = value` + `Foo { … }`（[`parse_legacy_blkx_string`]） |
//!
//! 统一入口是 [`parse_fm_text`]：**按内容**分派（首个非空白字节 `{` → JSON），不按后缀
//! ——两套数据里文件都叫 `.blkx`（G5/G9）。
//!
//! 两个 feature 同时打开时（`--features fm-legacy` 叠加默认的 `fm-json`）两条路径都在，
//! 回归测试就在同一个进程里对照它们（键与值都要一致，见 `test_p2_parity.rs`）。
//!
//! 语义层（`flight_model.rs` / `aero.rs` / `engine_model.rs` / `weapon_model.rs`）一行不用改：
//! 实测两边扁平化后的键集合 100% 相同（G11），值也按 [`crate::json_adapter`] 的归一规则对齐。

use std::collections::HashMap;

/// 当前构建**是否**带 legacy blkx 解析（GUI/日志用来显示数据来源）。
pub const FM_LEGACY_ENABLED: bool = cfg!(feature = "fm-legacy");
/// 当前构建**是否**带 JSON 适配器（GitHub 源）。
pub const FM_JSON_ENABLED: bool = cfg!(feature = "fm-json");

/// 文本种类（按内容判定，不看扩展名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FmTextKind {
    /// legacy blkx 文本（`key:type = value` / `Foo { }`）。
    LegacyBlkx,
    /// JSON（GitHub 源）。
    Json,
}

impl FmTextKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FmTextKind::LegacyBlkx => "legacy-blkx",
            FmTextKind::Json => "json",
        }
    }
}

/// 按**内容**判断文本种类：首个非空白字节（跳过 UTF-8 BOM）是 `{` → JSON，否则 legacy。
///
/// 依据是内容而不是后缀：两套数据里文件都叫 `.blkx`，后缀无法区分（G5/G9）。
pub fn detect_fm_text_kind(content: &str) -> FmTextKind {
    let head = content.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with('{') {
        FmTextKind::Json
    } else {
        FmTextKind::LegacyBlkx
    }
}

/// **统一解析入口**：任何来源的 FM 文本 → 同一套 `HashMap<String, BlkxValue>`。
///
/// 选中的实现不可用时（例如 legacy-only 构建收到 JSON）返回**可读错误**而不是空表：
/// 静默给出空模型会让"数据源不对"表现成"飞机性能全是 0"，极难排查。
pub fn parse_fm_text(content: &str) -> Result<HashMap<String, BlkxValue>, String> {
    match detect_fm_text_kind(content) {
        FmTextKind::Json => {
            #[cfg(feature = "fm-json")]
            {
                crate::json_adapter::parse_json_fm(content)
            }
            #[cfg(not(feature = "fm-json"))]
            {
                let _ = content;
                Err("数据是 JSON（GitHub 源），但本构建未启用 `fm-json` feature。\
                     它是**默认** feature —— 多半是用 `--no-default-features` 构建的：\
                     去掉那个参数（或显式加 `--features fm-json`）重新构建"
                    .to_string())
            }
        }
        FmTextKind::LegacyBlkx => {
            #[cfg(feature = "fm-legacy")]
            {
                Ok(parse_legacy_blkx_string(content))
            }
            #[cfg(not(feature = "fm-legacy"))]
            {
                let _ = content;
                Err("数据是 legacy blkx 文本（旧游戏解包），但本构建未启用 `fm-legacy` feature。\
                     legacy 解析器**仅作备份**、默认不编译：确实要读旧格式数据就用 \
                     `cargo build --features fm-legacy` 重新构建"
                    .to_string())
            }
        }
    }
}

/// Try multiple keys in order, return first value found
pub fn get_f64_first<'a>(
    data: &'a HashMap<String, BlkxValue>,
    keys: &[&str],
) -> Option<f64> {
    keys.iter().find_map(|k| data.get(*k).and_then(|v| v.as_f64()))
}

/// 取第一个数值：单值字段直接解析；p2..p6 多值字段（如
/// `ElevatorsEffectiveSpeed:p2 = "950.005, 950.005"`）取第一个分量。
/// 不改全局 `as_f64` 语义，避免多值字段在其它调用点被误当单值。
pub fn get_f64_first_value(
    data: &HashMap<String, BlkxValue>,
    keys: &[&str],
) -> Option<f64> {
    keys.iter().find_map(|k| {
        data.get(*k)
            .and_then(|v| v.as_f64().or_else(|| v.as_f64_vec().first().copied()))
    })
}

/// Try multiple keys in order, return first string value found
pub fn get_string_first<'a>(
    data: &'a HashMap<String, BlkxValue>,
    keys: &[&str],
) -> Option<String> {
    keys.iter().find_map(|k| data.get(*k).map(|v| v.as_string()))
}

/// Try keys across multiple maps (fm_data first, then data)
pub fn get_f64_multi<'a>(
    maps: &[&'a HashMap<String, BlkxValue>],
    keys: &[&str],
) -> Option<f64> {
    maps.iter()
        .flat_map(|m| keys.iter().map(move |k| (*m, *k)))
        .find_map(|(m, k)| m.get(k).and_then(|v| v.as_f64()))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlkxValueType {
    Bool,
    Text,
    Int,
    Real,
    Real2,
    Real3,
    Real4,
    Real5,
    Real6,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct BlkxValue {
    pub vtype: BlkxValueType,
    pub value: String,
}

impl BlkxValue {
    pub fn parse(v: &str, type_hint: Option<&str>) -> Self {
        let v = v.trim();
        let mut vtype = BlkxValueType::Unknown;
        let mut value = v.to_string();

        if let Some(typ) = type_hint {
            vtype = match typ {
                "b" => BlkxValueType::Bool,
                "t" => BlkxValueType::Text,
                "i" => BlkxValueType::Int,
                "r" => BlkxValueType::Real,
                "p2" => BlkxValueType::Real2,
                "p3" => BlkxValueType::Real3,
                "p4" => BlkxValueType::Real4,
                "p5" => BlkxValueType::Real5,
                "p6" => BlkxValueType::Real6,
                _ => BlkxValueType::Unknown,
            };
        }

        if vtype == BlkxValueType::Text
            && value.starts_with('"')
            && value.ends_with('"')
            && value.len() >= 2
        {
            value = value[1..value.len() - 1].to_string();
        }

        Self { vtype, value }
    }

    /// 无类型标签的原始值（JSON 适配器用）。
    ///
    /// `vtype` 全仓库只写不读（JSON 适配器不需要复刻 `b/t/i/r/p2..p6`）；
    /// JSON 侧字符串本身就不带引号，直接放进来即可，与 legacy 剥引号后的结果一致。
    pub fn from_raw(value: String) -> Self {
        Self {
            vtype: BlkxValueType::Unknown,
            value,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        self.value.parse().ok()
    }

    pub fn as_f64_vec(&self) -> Vec<f64> {
        self.value
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect()
    }



    pub fn as_string(&self) -> String {
        self.value.clone()
    }
}

/// legacy blkx 文本 → 扁平 `HashMap`（**原实现，一行未改**，只是改了名字以便和 JSON 路径并列）。
///
/// 规则（E7）：块 `Foo {` → 键前缀 `Foo.`；同级重复块 `[n]` 后缀（第 2 个起）；
/// `key:type = value` → 键 `key`（丢掉 `:type`）、值按 `t` 剥引号原样存字符串。
#[cfg(feature = "fm-legacy")]
pub fn parse_legacy_blkx_string(content: &str) -> HashMap<String, BlkxValue> {
    let mut result = HashMap::new();
    let mut current_dir: Vec<String> = Vec::new();
    let mut counters: HashMap<String, usize> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.contains('{') {
            let key = line.replace('{', "").trim().to_string();
            if !key.is_empty() {
                let parent_path = current_dir.join(".");
                let count_key = if parent_path.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", parent_path, key)
                };
                let entry = counters.entry(count_key).or_insert(0);
                let final_key = if *entry == 0 {
                    key
                } else {
                    format!("{}[{}]", key, entry)
                };
                *entry += 1;
                current_dir.push(final_key);
            }
        } else if line.contains('}') {
            if !current_dir.is_empty() {
                current_dir.pop();
            }
        } else if let Some(eq_pos) = line.find('=') {
            let key_part = line[..eq_pos].trim();
            let value_part = line[eq_pos + 1..].trim();

            let full_key = if current_dir.is_empty() {
                parse_key(key_part)
            } else {
                let prefix = current_dir.join(".");
                format!("{}.{}", prefix, parse_key(key_part))
            };

            let key_type = get_key_type(key_part);
            let blkx_value = BlkxValue::parse(value_part, key_type.as_deref());
            result.insert(full_key, blkx_value);
        }
    }

    result
}

/// `key:type` → 键名（丢掉 `:type`）。只被 legacy 解析器用到。
#[cfg(feature = "fm-legacy")]
fn parse_key(k: &str) -> String {
    let k = k.trim();
    if let Some(colon_pos) = k.find(':') {
        return k[..colon_pos].trim().to_string();
    }
    k.to_string()
}

/// `key:type` → 类型标签（`b/t/i/r/p2..p6`）。只被 legacy 解析器用到。
#[cfg(feature = "fm-legacy")]
fn get_key_type(k: &str) -> Option<String> {
    let k = k.trim();
    if let Some(colon_pos) = k.find(':') {
        if colon_pos + 1 < k.len() {
            return Some(k[colon_pos + 1..].trim().to_string());
        }
    }
    None
}
