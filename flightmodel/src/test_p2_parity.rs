//! 回归：**两条解析路径（legacy blkx ↔ GitHub JSON）必须产出同一套键值**。分三层：
//!
//! 1. `frozen_legacy_key_counts` —— 本地数据上冻结的键数清单，锁住扁平化行为
//!    （块前缀、重复块 `[n]`、丢类型）。**冻结值跟随数据版本**，换数据后要重采：
//!    把 `expect` 改成 0 跑一遍，用打印出的 `[冻结候选]` 回填。无数据时跳过。
//! 2. `vs_json::synthetic_pair_is_identical` —— 手写的最小 legacy/JSON 对照样本（入库），
//!    覆盖适配器的归一规则：类型、数组、对象数组、嵌套块、`.blk→.blkx`、大小写。
//!    需要两个 feature 同时打开（`--features fm-legacy`）。
//! 3. `vs_json::real_sample_legacy_vs_json_parity` —— 真实样本对照：`WP8F_FM_JSON_SAMPLE_DIR`
//!    指向一个数据根，逐机型与本地 legacy 数据比键和值；没设就跳过（样本不入库：上游无许可证）。

use std::path::{Path, PathBuf};

/// 本地数据下每个机型的**顶层文件键数**（冻结清单）。
///
/// 生成方式：`cargo test -p wp8f-flightmodel frozen_legacy_key_counts -- --nocapture`
/// （未冻结的项会打印 `[冻结候选] ("name", N),`）。
/// ⚠️ 值跟随数据版本：`resource/data` 一换就会漂移（2.58.0.35 → 2.59.0.43 动了 4 个机型）。
pub const FROZEN_KEY_COUNTS: &[(&str, usize)] = &[
    ("a-20g", 2428),
    ("b_52h", 4177),
    ("bf-109g-6", 1223),
    ("f-4m_fgr2", 2554),
    ("f_111f", 3178),
    ("f_15a", 3116),
    ("f_15e", 7392),
    ("f_16c_block_50", 3505),
    ("fa_18a", 3869),
    ("harrier_gr7", 3087),
    ("hornet_mk1", 1607),
    ("me-163b", 1222),
    ("mig_23mld", 2899),
    ("mirage_3s_c70_switzerland", 2008),
    ("nt_tu_95m", 3200),
    ("su-9", 1268),
    ("tornado_gr4", 4232),
    ("wyvern_s4", 2624),
    ("yak-3", 1088),
    ("yak_141", 2541),
];

/// 本地 legacy 机型目录（datamine 数据不入库 → 不存在时相关测试跳过而不是失败）。
fn local_fm_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("resource")
        .join("data")
        .join("gamedata")
        .join("flightmodels")
}

/// `resource/data/version` 的原文（一行，如 `2.59.0.43`）；读不到时给一句说明。
///
/// 冻结键数**跟着数据版本走**，所以失败信息里必须能一眼看到"这一轮冻结的是哪一版数据"。
fn local_data_version() -> String {
    let path = local_fm_dir().parent().and_then(|p| p.parent()).map(|p| p.join("version"));
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "<读不到 version 文件>".to_string())
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

#[allow(dead_code)]
fn read_local(name: &str) -> Option<(PathBuf, String)> {
    let p = local_fm_dir().join(format!("{name}.blkx"));
    read(&p).map(|c| (p, c))
}

#[test]
fn frozen_legacy_key_counts() {
    let dir = local_fm_dir();
    if !dir.is_dir() {
        eprintln!(
            "跳过 frozen_legacy_key_counts：缺少本地数据目录 {}（datamine 数据不入库）",
            dir.display()
        );
        return;
    }

    // 数据版本 + 实际解析路径都打出来：冻结值跟着数据版本走，对账时先看这两行。
    // （解析路径 = `parse_fm_text` 按内容分派的结果，与构建带哪个 feature 有关。）
    let version = local_data_version();
    let probe = read_local("a-20g").map(|(_, c)| c);
    let kind = probe
        .as_deref()
        .map(|c| crate::parser::detect_fm_text_kind(c).as_str())
        .unwrap_or("<读不到 a-20g.blkx>");
    println!("[FM 数据版本] {version}（resource/data/version）；解析路径 = {kind}");

    let mut mismatches = Vec::new();
    for (name, expect) in FROZEN_KEY_COUNTS {
        let Some((path, content)) = read_local(name) else {
            eprintln!("跳过 {name}：读不到 {}", dir.join(format!("{name}.blkx")).display());
            continue;
        };
        let map = crate::parser::parse_fm_text(&content)
            .unwrap_or_else(|e| panic!("本地数据应当能解析（{name}，数据版本 {version}）：{e}"));
        if *expect == 0 {
            // 未冻结（第一轮）→ 打印实测值，便于回填
            eprintln!("[冻结候选] (\"{name}\", {}),", map.len());
        } else if map.len() != *expect {
            mismatches.push(format!("{name}: 期望 {expect} 键，实测 {}（{}）", map.len(), path.display()));
        }
    }
    assert!(
        mismatches.is_empty(),
        "扁平化键数变了（数据版本 {version}；解析器被改过，或换了 FM 数据但没重新冻结？）：\n  {}\n  \
         → 换了数据就按用例文档重新采集 FROZEN_KEY_COUNTS（把 expect 改成 0 跑一遍，\
         用打印出来的 [冻结候选] 回填）",
        mismatches.join("\n  ")
    );
}

#[cfg(all(feature = "fm-legacy", feature = "fm-json"))]
mod vs_json {
    use super::*;
    use crate::parser::BlkxValue;
    use std::collections::{BTreeSet, HashMap};

    /// 抽样机型清单（**冻结**）：本地数据与 GitHub 源的交集，覆盖活塞/喷气/轰炸机/攻击机/
    /// 水上机/无平尾等形态。改这里要同步回填 [`FROZEN_KEY_COUNTS`]。
    pub const SAMPLE_AIRCRAFT: &[&str] = &[
        "a-20g",
        "b_52h",
        "bf-109g-6",
        "f-4m_fgr2",
        "f_111f",
        "f_15a",
        "f_15e",
        "f_16c_block_50",
        "fa_18a",
        "harrier_gr7",
        "hornet_mk1",
        "me-163b",
        "mig_23mld",
        "mirage_3s_c70_switzerland",
        "nt_tu_95m",
        "su-9",
        "tornado_gr4",
        "wyvern_s4",
        "yak-3",
        "yak_141",
    ];

    /// 真实 GitHub JSON 样本的数据根（`WP8F_FM_JSON_SAMPLE_DIR`）。
    fn json_sample_root() -> Option<PathBuf> {
        let p = PathBuf::from(std::env::var_os("WP8F_FM_JSON_SAMPLE_DIR")?);
        if p.join("gamedata").join("flightmodels").is_dir() {
            Some(p)
        } else {
            eprintln!(
                "跳过：WP8F_FM_JSON_SAMPLE_DIR={} 下没有 gamedata/flightmodels",
                p.display()
            );
            None
        }
    }

    #[derive(Debug, Default)]
    struct MapDiff {
        only_legacy: Vec<String>,
        only_json: Vec<String>,
        /// (键, legacy 值, json 值)
        value_diff: Vec<(String, String, String)>,
    }

    impl MapDiff {
        fn total(&self) -> usize {
            self.only_legacy.len() + self.only_json.len() + self.value_diff.len()
        }

        fn is_empty(&self) -> bool {
            self.total() == 0
        }

        fn summary(&self) -> String {
            format!(
                "onlyLegacy={} onlyJson={} valueDiff={}",
                self.only_legacy.len(),
                self.only_json.len(),
                self.value_diff.len()
            )
        }

        /// 逐条明细（最多 `limit` 条，便于失败信息里直接看到样例）。
        fn details(&self, limit: usize) -> String {
            let mut s = String::new();
            for k in self.only_legacy.iter().take(limit) {
                s.push_str(&format!("\n  仅 legacy: {k}"));
            }
            for k in self.only_json.iter().take(limit) {
                s.push_str(&format!("\n  仅 json  : {k}"));
            }
            for (k, a, b) in self.value_diff.iter().take(limit) {
                s.push_str(&format!(
                    "\n  值不同   : {k}\n    legacy = {a:?}\n    json   = {b:?}"
                ));
            }
            s
        }
    }

    /// 值等价判定：整串相等即等价；否则按 `,` 逐分量比（能 parse 成 `f64` 的按数值比）。
    ///
    /// 必须逐分量：JSON 侧数字是最短可回读表示（极小值走科学计数法），legacy 侧是定点写法 ——
    /// `as_f64` 看不出差别，逐字节比会把几百个格式化差异误报成语义差异。
    fn values_equal(a: &str, b: &str) -> bool {
        if a == b {
            return true;
        }
        if let (Ok(x), Ok(y)) = (a.parse::<f64>(), b.parse::<f64>()) {
            return x == y;
        }
        let av = split_parts(a);
        let bv = split_parts(b);
        av.len() == bv.len()
            && av
                .iter()
                .zip(bv.iter())
                .all(|(x, y)| x == y || matches!((x.parse::<f64>(), y.parse::<f64>()), (Ok(p), Ok(q)) if p == q))
    }

    fn split_parts(s: &str) -> Vec<&str> {
        s.split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect()
    }

    fn diff_maps(legacy: &HashMap<String, BlkxValue>, json: &HashMap<String, BlkxValue>) -> MapDiff {
        let mut d = MapDiff::default();
        let keys: BTreeSet<&String> = legacy.keys().chain(json.keys()).collect();

        for k in keys {
            match (legacy.get(k), json.get(k)) {
                (Some(_), None) => d.only_legacy.push(k.clone()),
                (None, Some(_)) => d.only_json.push(k.clone()),
                (Some(a), Some(b)) => {
                    if !values_equal(&a.as_string(), &b.as_string()) {
                        d.value_diff.push((k.clone(), a.as_string(), b.as_string()));
                    }
                }
                (None, None) => {}
            }
        }
        d
    }

    /// 手写的**最小 legacy/JSON 对照样本**：一份内容两种写法，必须产出同一张表。
    ///
    /// 覆盖：单值（整数/浮点/负号/布尔/字符串）、`p2`/`p4` 多值、嵌套块、同级重复块 → `[n]`、
    /// 三级嵌套块、字符串数组（legacy 写两行、后者覆盖）、注释行。
    #[test]
    fn synthetic_pair_is_identical() {
        let legacy = r#"
# 合成样本：legacy blkx 文本
model:t = "a_20"
fmFile:t = "fm/A-20G.blk"
noEffectsDist:r = 8
gyroSight:b = false
WingSpringDampJointMult:p2 = 0.5, 0.005
CockpitDoorSpeedOpen:p4 = 150, 300, 0.5, 0.02
type:t = "typeBomber"
type:t = "typeTransport"
Mass {
    Empty:r = 1000.5
    GearDestructionIndSpeed:r = 500
}
Wing {
    Span:r = 10
}
Wing {
    Span:r = 20
}
DamageParts {
    armor10 {
        gun1_dm {
            hp:r = 100
        }
    }
}
"#;

        let json = r#"{
  "model": "a_20",
  "fmFile": "fm/A-20G.blk",
  "noEffectsDist": 8.0,
  "gyroSight": false,
  "WingSpringDampJointMult": [0.5, 0.005],
  "CockpitDoorSpeedOpen": [150, 300, 0.5, 0.02],
  "type": ["typeBomber", "typeTransport"],
  "Mass": { "Empty": 1000.5, "GearDestructionIndSpeed": 500 },
  "Wing": [ { "Span": 10.0 }, { "Span": 20.0 } ],
  "DamageParts": { "armor10": { "gun1_dm": { "hp": 100 } } }
}"#;

        let l = crate::parser::parse_legacy_blkx_string(legacy);
        let j = crate::json_adapter::parse_json_fm(json).expect("json");
        let d = diff_maps(&l, &j);
        assert!(
            d.is_empty(),
            "合成样本两条路径必须完全一致（{}）{}",
            d.summary(),
            d.details(20)
        );

        // 顺带把几条关键规则钉死（"错得一样"也要挡住）
        assert_eq!(j["Mass.Empty"].as_string(), "1000.5");
        assert_eq!(j["noEffectsDist"].as_string(), "8");
        assert_eq!(j["Wing[1].Span"].as_string(), "20");
        assert_eq!(j["type"].as_string(), "typeTransport");
        assert_eq!(
            j["CockpitDoorSpeedOpen"].as_f64_vec(),
            vec![150.0, 300.0, 0.5, 0.02]
        );
        assert_eq!(j["fmFile"].as_string(), "fm/A-20G.blk");
    }

    /// `.blk` → `.blkx` + 小写：`parse_aircraft` 走的就是这条归一（D14 回归），
    /// 引用取的是**真实数据里出现过的写法**。
    #[test]
    fn real_data_references_are_normalized() {
        use crate::fm_paths::{normalize_blkx_ref, normalize_weapon_ref};
        // 本地 legacy 与上游 JSON 的 fmFile 都是 `fm/a-20g.blk`（带 x 才存在）
        assert_eq!(normalize_blkx_ref("fm/a-20g.blk"), "fm/a-20g.blkx");
        assert_eq!(normalize_blkx_ref("/fm/a-20g.blk"), "fm/a-20g.blkx");
        // 上游引用大写 + 缺 x（Windows 掩盖、Linux 404）
        assert_eq!(
            normalize_weapon_ref("gameData/FlightModels/weaponPresets/A-20G_default.blk"),
            "a-20g_default.blkx"
        );
        assert_eq!(
            normalize_weapon_ref("gameData/Weapons/gunBrowning50.blk"),
            "gunbrowning50.blkx"
        );
    }

    /// **实测冻结的残留差异**（放行清单）：适配器唯一有歧义的一类（见
    /// `json_adapter::scalar_array_text`）—— "长度 ≤ 6 的数字数组"分不清"一行 pN"与
    /// "重复标量行"，适配器按前者处理，代价就是这些键在 JSON 侧是拼接值、legacy 侧只有最后一个。
    ///
    /// 断言要求差异集合与本清单**完全相等**：新增或消失都失败（清单过期也要显式面对）。
    /// 实测（tag 2.58.0.35，20 机型 × 2 文件，86,535 键）：13 处 0.015%，且这些键在语义层
    /// grep 不到 → 对性能数值零影响。
    pub const FROZEN_DIVERGENCES: &[&str] = &[
        // `tank10_priority:i = 0` 两行 → legacy "0"，JSON [0, 0]
        "b_52h fm/nt_b_52h.blkx Mass.Parts.tank10_priority",
        // `maskTextureOffset:i` / `rampTextureOffset:i` 各写 6 行 → legacy 最后一个
        "f_111f (top) AfterBurner0.volumetricAfterBurner.NozzleFlames.FlameMaskTextureOffset.maskTextureOffset",
        "f_111f (top) AfterBurner0.volumetricAfterBurner.NozzleFlames.FlameRampTextureOffset.rampTextureOffset",
        "f_16c_block_50 (top) AfterBurner0.volumetricAfterBurner.NozzleFlames.FlameMaskTextureOffset.maskTextureOffset",
        "f_16c_block_50 (top) AfterBurner0.volumetricAfterBurner.NozzleFlames.FlameRampTextureOffset.rampTextureOffset",
        "fa_18a (top) AfterBurner0.volumetricAfterBurner.NozzleFlames.FlameMaskTextureOffset.maskTextureOffset",
        "fa_18a (top) AfterBurner0.volumetricAfterBurner.NozzleFlames.FlameRampTextureOffset.rampTextureOffset",
        // `sightInFov:r` / `zoomInFov:r` / `zoomOutFov:r` 各两行（座舱视角，语义层不读）
        "f_111f (top) cockpit.sightInFov",
        "f_111f (top) cockpit.zoomInFov",
        "f_111f (top) cockpit.zoomOutFov",
        // `counterIndex:i = 1/2/3` 三行 → legacy "3"，JSON [1, 2, 3]
        "f_15a (top) WeaponSlots.WeaponSlot.WeaponPreset.Weapon[1].counterIndex",
        "f_15e (top) WeaponSlots.WeaponSlot.WeaponPreset.Weapon[1].counterIndex",
        "fa_18a (top) WeaponSlots.WeaponSlot.WeaponPreset.Weapon[1].counterIndex",
    ];

    fn divergence_id(name: &str, label: &str, key: &str) -> String {
        format!("{name} {label} {key}")
    }

    /// **真实样本**对照：legacy 数据根 vs GitHub JSON（`WP8F_FM_JSON_SAMPLE_DIR`）。
    /// 逐机型比"顶层文件 + `fmFile` 指向的 fm 文件"的键集合与值，只放行 [`FROZEN_DIVERGENCES`]。
    ///
    /// legacy 侧数据根用 `WP8F_FM_LEGACY_SAMPLE_DIR`，没设就用本地 `resource/data`。
    /// ⚠️ 本地数据自 2.59.0.43 起是 JSON 版，那时两条路径同源、这条对照没有可比性 → 跳过。
    #[test]
    fn real_sample_legacy_vs_json_parity() {
        let Some(json_root) = json_sample_root() else {
            eprintln!(
                "跳过 real_sample_legacy_vs_json_parity：未设置 WP8F_FM_JSON_SAMPLE_DIR。\n  \
                 取样本：从 GitHub 源（`github.com/gszabi99/War-Thunder-Datamine/raw/<tag>`，\
                 或 `api.github.com` 的 blobs 接口；P9 起不再走第三方 CDN）\
                 按 `aces.vromfs.bin_u/gamedata/flightmodels/**` 下载到临时目录；样本不入库（上游无许可证）。"
            );
            return;
        };
        let legacy_dir = match std::env::var_os("WP8F_FM_LEGACY_SAMPLE_DIR") {
            Some(p) => PathBuf::from(p),
            None => local_fm_dir(),
        };
        if !legacy_dir.is_dir() {
            eprintln!("跳过：本地没有 {}（datamine 数据不入库）", legacy_dir.display());
            return;
        }
        // 本地数据根已经是 JSON 版 → 没有 legacy 侧可比（见上面的用例文档）
        if let Some(probe) = read(&legacy_dir.join("a-20g.blkx")) {
            if crate::parser::detect_fm_text_kind(&probe) != crate::parser::FmTextKind::LegacyBlkx {
                eprintln!(
                    "跳过 real_sample_legacy_vs_json_parity：{} 里是 JSON 版数据（2.59.0.43 起），\
                     没有 legacy 侧输入 → 无从对照。\n  要跑它：用 WP8F_FM_LEGACY_SAMPLE_DIR \
                     指向一份旧版 legacy 数据根的 `gamedata/flightmodels`（其父目录还应含 gamedata/）。",
                    legacy_dir.display()
                );
                return;
            }
        }
        let json_fm_dir = json_root.join("gamedata").join("flightmodels");

        let mut report = String::new();
        let mut observed: Vec<(String, String)> = Vec::new(); // (id, 明细)
        let (mut n_pairs, mut n_keys, mut n_known, mut n_unexpected) = (0usize, 0usize, 0usize, 0usize);
        let mut n_join_rule_diffs = 0usize;

        for name in SAMPLE_AIRCRAFT {
            let (Some((_, l_text)), Some(j_text)) = (
                read_local(name),
                read(&json_fm_dir.join(format!("{name}.blkx"))),
            ) else {
                report.push_str(&format!("  {name:<32} 样本缺失，跳过\n"));
                continue;
            };

            // 机型文件 + 它引用的 fmFile（同样先归一 `.blk`→`.blkx`）
            let mut pairs: Vec<(String, String, String)> =
                vec![("(top)".to_string(), l_text.clone(), j_text.clone())];
            if let Some(fm_ref) = crate::parser::parse_fm_text(&l_text)
                .ok()
                .and_then(|m| m.get("fmFile").map(|v| v.as_string()))
            {
                let rel = crate::fm_paths::normalize_blkx_ref(&fm_ref);
                if let (Some(lf), Some(jf)) =
                    (read(&legacy_dir.join(&rel)), read(&json_fm_dir.join(&rel)))
                {
                    pairs.push((rel, lf, jf));
                }
            }

            for (label, l_text, j_text) in pairs {
                let l = crate::parser::parse_legacy_blkx_string(&l_text);
                let j = crate::json_adapter::parse_json_fm(&j_text).expect("JSON 样本应当能解析");
                let d = diff_maps(&l, &j);
                n_pairs += 1;
                n_keys += l.len();

                let keys: Vec<(&str, &String)> = d
                    .only_legacy
                    .iter()
                    .map(|k| ("仅 legacy", k))
                    .chain(d.only_json.iter().map(|k| ("仅 json", k)))
                    .chain(d.value_diff.iter().map(|(k, _, _)| ("值不同", k)))
                    .collect();
                let mut file_known = 0usize;
                for (kind, key) in &keys {
                    let id = divergence_id(name, &label, key);
                    if FROZEN_DIVERGENCES.contains(&id.as_str()) {
                        file_known += 1;
                        n_known += 1;
                    } else {
                        n_unexpected += 1;
                        observed.push((id.clone(), format!("{kind}: {id}{}", d.details(3))));
                    }
                }

                report.push_str(&format!(
                    "  {name:<32} {label:<28} legacy={:<5} json={:<5} {}\n",
                    l.len(),
                    j.len(),
                    if keys.is_empty() {
                        "一致".to_string()
                    } else {
                        format!("差异 {}（已知 {file_known}）", keys.len())
                    }
                ));

                // 证据：字符串数组改用"拼接"规则会多出多少差异（当前规则取 last-wins）
                let j_join = crate::json_adapter::parse_json_fm_with_rule(
                    &j_text,
                    crate::json_adapter::StrArrayRule::Join,
                )
                .expect("json");
                n_join_rule_diffs += diff_maps(&l, &j_join).total();
            }
        }

        println!(
            "\n=== legacy vs JSON 真实样本对照（{n_pairs} 对文件 / {n_keys} 个 legacy 键）===\n{report}  \
             [已知差异] 命中冻结清单 {n_known} 处（共 {} 条）\n  \
             [规则对照] 字符串数组改用 Join 拼接后的差异总数 = {n_join_rule_diffs}\n",
            FROZEN_DIVERGENCES.len()
        );
        assert!(
            n_pairs >= SAMPLE_AIRCRAFT.len(),
            "样本对太少（{n_pairs}），检查 WP8F_FM_JSON_SAMPLE_DIR 指向的目录"
        );
        assert!(
            n_unexpected == 0,
            "出现**未冻结**的差异（数据变了？规则改错了？）：\n{}",
            observed
                .iter()
                .map(|(_, d)| d.clone())
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert_eq!(
            n_known,
            FROZEN_DIVERGENCES.len(),
            "冻结清单与实际差异条数不符（有清单项已经不再差异 → 请更新清单）"
        );
    }
}
