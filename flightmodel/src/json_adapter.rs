//! **JSON 适配器**（`fm-json` feature）：GitHub `gszabi99/War-Thunder-Datamine` 的 JSON 版 FM
//! → 与 legacy blkx 完全相同的 `HashMap<String, BlkxValue>` —— 语义层（`flight_model.rs` 及以下）
//! 一行不用改，这里只负责把 JSON 展平成同一套键值。
//!
//! # 归一规则（逐条对应 legacy 的写法，全部由真实样本实测校准）
//!
//! | JSON | 产出键 | 产出值 | legacy 对应写法 |
//! |---|---|---|---|
//! | `{"a": {"b": 1}}` | `a.b` | `"1"` | `a { b:r = 1 }` |
//! | `{"a": 1}` / `{"a": 1.0}` | `a` | `"1"` | `a:r = 1`（整数不带 `.0`） |
//! | `{"a": true}` | `a` | `"true"` | `a:b = true` |
//! | `{"a": "x"}` | `a` | `"x"` | `a:t = "x"`（legacy 剥引号，JSON 本来就没有） |
//! | `{"a": [1, 2]}`（数字，长度 ≤6） | `a` | `"1, 2"` | `a:p2 = 1, 2`（**单行多值**） |
//! | `{"a": [1,2,3,4,5,6,7]}`（数字，长度 >6） | `a` | 最后一个 | 同一标量键写多行 → 后写覆盖 |
//! | `{"a": ["x","y"]}`（全字符串） | `a` | `"y"` | `a:t = "x"` 紧跟 `a:t = "y"` → 覆盖 |
//! | `{"a": [true,true]}`（全布尔） | `a` | `"true"` | 布尔没有多值类型 → 覆盖 |
//! | `{"a": [{"…"},{"…"}]}` | `a`, `a[1]`, … | 递归展平 | 同级重复块 → 第 2 个起 `[n]` |
//! | `{"a": ["x", {…}, {…}]}`（混对象） | `a`（标量）+ `a.*`（第 1 块）+ `a[1].*` | — | 标量行不占块号（见 [`flatten_array`]） |
//! | `{"a": [[0,390],[2700,445]]}` | `a` | `"2700, 445"` | 同一个多值键写多行 → 覆盖 |
//! | `{"a": {}}` / `{"a": []}` / `{"a": null}` | **不产生键** | — | 空块 `a { }` 不产生任何键 |
//!
//! 每类规则的实测证据与残留歧义写在 [`scalar_array_text`] 的文档里；
//! 对照结果由 `test_p2_parity.rs` 冻结。
//!
//! 类型标签：legacy 的 `BlkxValue.vtype` 全仓库只写不读，JSON 侧统一填 `Unknown`。

use crate::parser::BlkxValue;
use serde_json::Value;
use std::collections::HashMap;

/// 纯字符串数组的归一规则（实测决定，对照报告见 `test_p2_parity.rs`）。
///
/// [`StrArrayRule::LastWins`]：legacy 里"同一个键写多行"用 `HashMap::insert` 后写覆盖先写，
/// 取最后一个元素才能与 legacy 逐值相等。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StrArrayRule {
    /// 按 `", "` 拼接（**只用于回归对照**：量一下换这条规则会多出多少差异）。
    #[allow(dead_code)]
    Join,
    /// 取最后一个元素（生产路径用它，见 [`STR_ARRAY_RULE`]）。
    LastWins,
}

/// 生产路径使用的规则：字符串数组在上游 JSON 里全部来自"同一个 `t` 键写多行"，
/// legacy 只有最后一个值活着 —— 用 `Join` 会造出 legacy 永远不会有的值
/// （例如 `type` = `"typeBomber, typeTransport"`），而 `Join` 对数值数组才是必需的。
pub(crate) const STR_ARRAY_RULE: StrArrayRule = StrArrayRule::LastWins;

/// JSON FM 文本 → 同一套扁平 `HashMap`。
///
/// 解析失败返回**可读错误**（含行列号）；顶层不是对象也报错（GitHub 源的 FM 文件都是对象）。
pub fn parse_json_fm(content: &str) -> Result<HashMap<String, BlkxValue>, String> {
    parse_json_fm_with_rule(content, STR_ARRAY_RULE)
}

/// 同上，但可指定字符串数组规则（回归测试用来对照两种规则的实际差异）。
pub(crate) fn parse_json_fm_with_rule(
    content: &str,
    rule: StrArrayRule,
) -> Result<HashMap<String, BlkxValue>, String> {
    let root: Value = serde_json::from_str(content).map_err(|e| {
        format!(
            "JSON 解析失败（第 {} 行第 {} 列）: {}",
            e.line(),
            e.column(),
            e
        )
    })?;

    if !root.is_object() {
        return Err(format!(
            "JSON 解析失败：顶层不是对象（{}），GitHub 源的 FM 文件形如 `{{ ... }}`",
            json_type_name(&root)
        ));
    }

    let mut out: HashMap<String, BlkxValue> = HashMap::new();
    flatten_into(&root, "", rule, &mut out);
    // 及时释放（交付物 4）：展平结束立刻丢掉整棵 `serde_json::Value`（中间对象的峰值只存在
    // 到这一行为止，函数返回后常驻的只有 `HashMap`）。`tests/json_release.rs` 用计数分配器
    // 断言"返回后的常驻字节 ≪ 峰值字节"。
    drop(root);
    Ok(out)
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 标量 → legacy 文本（字符串不带引号、布尔小写、数字整数不带 `.0`）。
fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_text(n),
        Value::Null => String::new(),
        // 调用点已保证只对标量调用；真到了这里给空串而不是 panic。
        Value::Array(_) | Value::Object(_) => String::new(),
    }
}

/// 数字 → legacy 文本。
///
/// legacy 侧整数写法不带小数点（`ElevatorsEffectiveSpeed:p2 = 300, 300`），而 JSON 里是
/// `300.0`；Rust 的 `f64` Display 打的是**最短可回读**表示且整数值不带 `.0`，正好对上。
/// `as_f64` 两种写法都能 parse，所以这里主要影响"逐值对比"和日志可读性。
fn number_text(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    match n.as_f64() {
        Some(f) => format!("{f}"),
        None => n.to_string(),
    }
}

/// 单个 `pN` 类型最多 6 个分量（`p2..p6`，见 `parser.rs` 的类型表）：长度 > 6 的数字数组
/// 不可能是"一行 pN"，只能是"同一个标量键写了好几行"，而重复标量键在 legacy 里是后写覆盖
/// → 取最后一个元素。
const MAX_PN_ARITY: usize = 6;

/// 全标量数组 → legacy 的单行文本（适配器里唯一有歧义的地方，规则按实测数据定）：
///
/// | 数组 | legacy 里的真实形态 | 规则 |
/// |---|---|---|
/// | 全数字，长度 2..=6 | 一行 `key:p2/p3/p4 = a, b` | **逗号拼接** |
/// | 全数字，长度 > 6 | 同一标量键写多行 | 取**最后一个** |
/// | 全字符串 | 同一个 `t` 键写多行 | 取**最后一个** |
/// | 全布尔 | 同一个 `b` 键写多行（布尔没有多值类型） | 取**最后一个** |
///
/// 残留歧义：长度 ≤ 6 的数字数组若来自"重复标量行"，与"一行 p3"结构上不可区分，按 pN 处理。
/// 代价不对称：拼错方向只会让极少数视觉参数的 `as_f64()` 退化成 `None`，
/// 反过来会把几千个语义层要读的 `p2/p3/p4` 字段（`Lim`/`Arm`/`Angle`…）打碎。
fn scalar_array_text(items: &[&Value], rule: StrArrayRule) -> String {
    let last = || items.last().copied().map(scalar_text).unwrap_or_default();

    if items.iter().all(|v| v.is_string()) {
        return if rule == StrArrayRule::LastWins {
            last()
        } else {
            join_scalars(items)
        };
    }
    if items.iter().all(|v| v.is_boolean()) {
        return last(); // 布尔只有标量类型，写多行 = 后写覆盖
    }
    if items.iter().all(|v| v.is_number()) && items.len() > MAX_PN_ARITY {
        return last(); // 一行 pN 最多 6 个分量 → 只能是重复标量行
    }
    join_scalars(items)
}

fn join_scalars(items: &[&Value]) -> String {
    items
        .iter()
        .map(|v| scalar_text(v))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 数组套数组 → 全部标量拍平成一条（legacy 的多值字段就是一行逗号分隔）。
fn collect_scalar_texts(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Array(items) => {
            for item in items {
                collect_scalar_texts(item, out);
            }
        }
        Value::Null => {}
        other => out.push(scalar_text(other)),
    }
}

/// 递归展平：`key` 是当前键前缀（顶层为空串）。
fn flatten_into(
    value: &Value,
    key: &str,
    rule: StrArrayRule,
    out: &mut HashMap<String, BlkxValue>,
) {
    match value {
        Value::Object(map) => {
            // legacy 的空块（`Foo { }`）不产生任何键 → 空对象也不产生。
            for (k, child) in map {
                let child_key = join_key(key, k);
                flatten_into(child, &child_key, rule, out);
            }
        }
        Value::Array(items) => flatten_array(items, key, rule, out),
        // legacy 没有 null 的对应写法：不产生键（空值而不是伪造一个 "null" 字符串）。
        Value::Null => {}
        scalar => {
            out.insert(key.to_string(), BlkxValue::from_raw(scalar_text(scalar)));
        }
    }
}

fn flatten_array(
    items: &[Value],
    key: &str,
    rule: StrArrayRule,
    out: &mut HashMap<String, BlkxValue>,
) {
    // legacy 里没有 null 的写法（空值不产生键）→ 先剔掉，免得落入拼接规则变成空串分量。
    let items: Vec<&Value> = items.iter().filter(|v| !v.is_null()).collect();
    if items.is_empty() {
        return; // `"a": []` ↔ 空块，不产生键
    }

    if items.iter().any(|v| v.is_object()) {
        // **块数组** = legacy 的"同级重复块"，两条规则要与 legacy 逐字对齐：
        //   * 对象元素按**块计数**加后缀（第 1 个不加、第 2 个 `[1]`…）—— 只数块，
        //     标量元素不占号（legacy 的 `counters` 只在 `Foo {` 时自增）；
        //   * 标量/数组元素 = 同一个键写了多行 → 后写覆盖（写回**基键**）。
        let mut block = 0usize;
        for item in items.iter().copied() {
            match item {
                Value::Object(_) => {
                    let child_key = if block == 0 {
                        key.to_string()
                    } else {
                        format!("{}[{}]", key, block)
                    };
                    block += 1;
                    flatten_into(item, &child_key, rule, out);
                }
                Value::Array(_) => {
                    let mut flat = Vec::new();
                    collect_scalar_texts(item, &mut flat);
                    if !flat.is_empty() {
                        out.insert(key.to_string(), BlkxValue::from_raw(flat.join(", ")));
                    }
                }
                scalar => {
                    out.insert(key.to_string(), BlkxValue::from_raw(scalar_text(scalar)));
                }
            }
        }
        return;
    }

    if items.iter().any(|v| v.is_array()) {
        // **数组套数组** = legacy 里"同一个多值键写了多行"→ 后写覆盖。
        // 实测 `Passport.Alt.maxSpeedNom`：legacy 两行 `maxSpeedNom:p2 = 0, 390` /
        // `= 2700, 445`（后者覆盖），JSON 是 `[[0,390],[2700,445]]`。
        // 取**最后一个非空分组**（这才是 `as_f64_vec` 在 legacy 下会看到的值）。
        let last = items.iter().rev().copied().find_map(|v| {
            let mut flat = Vec::new();
            collect_scalar_texts(v, &mut flat);
            (!flat.is_empty()).then(|| flat.join(", "))
        });
        if let Some(v) = last {
            out.insert(key.to_string(), BlkxValue::from_raw(v));
        }
        return;
    }

    if items.iter().all(|v| !v.is_object() && !v.is_array()) {
        out.insert(
            key.to_string(),
            BlkxValue::from_raw(scalar_array_text(&items, rule)),
        );
        return;
    }

    // 混合数组（标量 + 数组/对象）在真实数据里没出现；按"每个元素一个键"处理，
    // 位置 0 用原名、其余加 `[i]`，至少不丢数据。
    for (i, item) in items.iter().copied().enumerate() {
        let child_key = if i == 0 {
            key.to_string()
        } else {
            format!("{}[{}]", key, i)
        };
        flatten_into(item, &child_key, rule, out);
    }
}

fn join_key(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{}.{}", prefix, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(json: &str) -> HashMap<String, BlkxValue> {
        parse_json_fm(json).expect("json parse")
    }

    fn val(json: &str, key: &str) -> String {
        let m = flat(json);
        m.get(key)
            .unwrap_or_else(|| panic!("缺少键 {key}（实际键：{:?}）", {
                let mut ks: Vec<_> = m.keys().cloned().collect();
                ks.sort();
                ks
            }))
            .as_string()
    }

    #[test]
    fn scalars_follow_legacy_text_form() {
        let j = r#"{"a": 1, "b": 1.0, "c": 2.5, "d": true, "e": false, "f": "x y", "g": -3}"#;
        assert_eq!(val(j, "a"), "1");
        // `1.0` 在 legacy 侧写作 `1`（整数不带小数点）
        assert_eq!(val(j, "b"), "1");
        assert_eq!(val(j, "c"), "2.5");
        assert_eq!(val(j, "d"), "true");
        assert_eq!(val(j, "e"), "false");
        assert_eq!(val(j, "f"), "x y");
        assert_eq!(val(j, "g"), "-3");
    }

    #[test]
    fn nested_object_becomes_dotted_key() {
        let j = r#"{"Mass": {"Empty": 1000.0, "GearDestructionIndSpeed": 500}}"#;
        assert_eq!(val(j, "Mass.Empty"), "1000");
        assert_eq!(val(j, "Mass.GearDestructionIndSpeed"), "500");
    }

    #[test]
    fn numeric_array_joins_like_p2_line() {
        // legacy: `ElevatorsEffectiveSpeed:p2 = 300, 300`
        let j = r#"{"ElevatorsEffectiveSpeed": [300.0, 300.0], "p4": [150, 280, 0.15, 0.18]}"#;
        assert_eq!(val(j, "ElevatorsEffectiveSpeed"), "300, 300");
        assert_eq!(val(j, "p4"), "150, 280, 0.15, 0.18");
        // 值必须能被 as_f64_vec 切成同样的向量（语义层就是这么消费的）
        let m = flat(j);
        assert_eq!(m["ElevatorsEffectiveSpeed"].as_f64_vec(), vec![300.0, 300.0]);
        assert_eq!(m["p4"].as_f64_vec(), vec![150.0, 280.0, 0.15, 0.18]);
    }

    #[test]
    fn string_array_keeps_last_like_repeated_keys() {
        // legacy: `type:t = "typeBomber"` 紧跟 `type:t = "typeTransport"` → 后者覆盖前者
        let j = r#"{"type": ["typeBomber", "typeTransport"]}"#;
        assert_eq!(val(j, "type"), "typeTransport");
        assert_eq!(
            flat(j).len(),
            1,
            "重复键只留一个（与 legacy 的 HashMap::insert 语义一致）"
        );
    }

    #[test]
    fn bool_array_keeps_last_like_repeated_keys() {
        // 布尔没有多值类型（没有 `pb2`）→ 多行 `hideScale:b = true` 也只能是覆盖
        let j = r#"{"hideScale": [true, true], "x": [false, true]}"#;
        assert_eq!(val(j, "hideScale"), "true");
        assert_eq!(val(j, "x"), "true");
    }

    #[test]
    fn long_numeric_array_keeps_last_like_repeated_scalar_lines() {
        // 实测 f_15e：legacy `rampTextureOffset:i = 23` … `= 36`（8 行）→ 36；
        // 一行 pN 最多 6 个分量，所以 8 个元素不可能是单行多值。
        let j = r#"{"rampTextureOffset": [23, 23, 23, 23, 30, 30, 35, 36]}"#;
        assert_eq!(val(j, "rampTextureOffset"), "36");
    }

    #[test]
    fn short_numeric_array_joins_as_pn_line() {
        // ≤6 个数字按"一行 p2/p3/p4"处理（实测 40 万个 pN 行里只有 p2/p3/p4）
        let j = r#"{"counterIndex": [1, 2, 3], "p4": [1, 2, 3, 4]}"#;
        assert_eq!(val(j, "counterIndex"), "1, 2, 3");
        assert_eq!(val(j, "p4"), "1, 2, 3, 4");
    }

    #[test]
    fn mixed_array_counts_only_blocks() {
        // 实测 b_52h `bailout.seat`：4 个字符串 + 2 个块。
        // legacy 里 `seat` 是最后一个标量、"第 1 个块"不带后缀、"第 2 个块"是 `seat[1]`
        //（块计数只数块，标量行不占号）。
        let j = r#"{"seat": ["seat_01", "seat_02", "seat_03", "seat_04",
                             {"part": "seat_05", "seatVel": [-10, -20, 0]},
                             {"part": "seat_06", "seatVel": [-10, -20, 0]}]}"#;
        let m = flat(j);
        assert_eq!(val(j, "seat"), "seat_04");
        assert_eq!(val(j, "seat.part"), "seat_05");
        assert_eq!(val(j, "seat.seatVel"), "-10, -20, 0");
        assert_eq!(val(j, "seat[1].part"), "seat_06");
        assert!(!m.contains_key("seat[4].part"), "标量不占块号：{:?}", {
            let mut k: Vec<_> = m.keys().cloned().collect();
            k.sort();
            k
        });
    }

    #[test]
    fn array_of_arrays_keeps_last_group() {
        // 实测 Passport.Alt.maxSpeedNom：legacy 两行 `maxSpeedNom:p2 = 0, 390` /
        // `= 2700, 445`（后者覆盖）→ 值就是 "2700, 445"
        let j = r#"{"maxSpeedNom": [[0, 390], [2700, 445]]}"#;
        assert_eq!(val(j, "maxSpeedNom"), "2700, 445");
        assert_eq!(flat(j)["maxSpeedNom"].as_f64_vec(), vec![2700.0, 445.0]);
        // 只有一组时就是那一组
        assert_eq!(val(r#"{"a": [[1, 2]]}"#, "a"), "1, 2");
    }

    #[test]
    fn object_array_indexes_from_one() {
        // legacy: 两个同级 `Wing { }` 块 → `Wing.…` 与 `Wing[1].…`
        let j = r#"{"Wing": [{"Span": 10.0}, {"Span": 20.0}, {"Span": 30.0}]}"#;
        assert_eq!(val(j, "Wing.Span"), "10");
        assert_eq!(val(j, "Wing[1].Span"), "20");
        assert_eq!(val(j, "Wing[2].Span"), "30");
    }

    #[test]
    fn single_object_array_has_no_suffix() {
        let j = r#"{"Wing": [{"Span": 10.0}]}"#;
        let m = flat(j);
        assert_eq!(m.len(), 1);
        assert_eq!(val(j, "Wing.Span"), "10");
    }

    #[test]
    fn nested_array_keeps_last_group() {
        // 数组套数组 = "同一个多值键写了多行" → legacy 只有最后一组活着（见上方归一表）
        let j = r#"{"FlapsDestructionIndSpeedP": [[0.2, 1018.0], [1.0, 481.0]]}"#;
        let m = flat(j);
        assert_eq!(
            m["FlapsDestructionIndSpeedP"].as_f64_vec(),
            vec![1.0, 481.0]
        );
    }

    #[test]
    fn empty_object_array_and_null_produce_no_key() {
        let j = r#"{"a": {}, "b": [], "c": null, "d": 1}"#;
        let m = flat(j);
        let mut ks: Vec<_> = m.keys().cloned().collect();
        ks.sort();
        assert_eq!(ks, vec!["d".to_string()]);
    }

    #[test]
    fn deeply_nested_blocks_keep_legacy_prefix_chain() {
        let j = r#"{"EngineType0": {"Main": {"Type": "jet", "Thrust": 8000.0}}}"#;
        assert_eq!(val(j, "EngineType0.Main.Type"), "jet");
        assert_eq!(val(j, "EngineType0.Main.Thrust"), "8000");
    }

    #[test]
    fn top_level_must_be_object() {
        assert!(parse_json_fm("[1, 2, 3]").is_err());
        assert!(parse_json_fm("not json at all").is_err());
        assert!(parse_json_fm("{ oops }").is_err());
    }

    #[test]
    fn join_rule_differs_only_for_string_arrays() {
        let j = r#"{"type": ["a", "b"], "p2": [1, 2]}"#;
        let last = parse_json_fm_with_rule(j, StrArrayRule::LastWins).unwrap();
        let join = parse_json_fm_with_rule(j, StrArrayRule::Join).unwrap();
        assert_eq!(last["type"].as_string(), "b");
        assert_eq!(join["type"].as_string(), "a, b");
        assert_eq!(last["p2"].as_string(), join["p2"].as_string());
    }
}
