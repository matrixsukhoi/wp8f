use crate::fm_paths::{normalize_blkx_ref, normalize_weapon_ref, weapon_ref_stem};
use crate::parser::{get_f64_first, get_string_first, parse_fm_text, BlkxValue};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct BulletInfo {
    pub bullet_type: String,
    pub mass: f64,
    pub armor_power: f64,
    pub hit_power: f64,
    pub hit_power_mult: f64,
    pub explode_hit_power: f64,
    pub explode_armor_power: f64,
    pub explode_radius: f64,
    pub fire_chance: f64,
}

#[derive(Debug, Clone)]
pub struct BeltInfo {
    pub name: String,
    pub bullets: Vec<BulletInfo>,
}

impl BeltInfo {
}

#[derive(Debug, Clone)]
pub struct Weapon {
    pub name: String,
    pub mass: f64,
    pub bullet_mass: f64,
    pub ammo_count: usize,
    pub caliber: f64,
    pub shot_freq: f64,
    pub bullets_cluster: usize,
    pub weapon_type: i64,
    pub belts: Vec<BeltInfo>,
}

impl Default for Weapon {
    fn default() -> Self {
        Self {
            name: String::new(),
            mass: 0.0,
            bullet_mass: 0.0,
            ammo_count: 0,
            caliber: 0.0,
            shot_freq: 0.0,
            bullets_cluster: 1,
            weapon_type: 0,
            belts: Vec::new(),
        }
    }
}

impl Weapon {
    pub fn load(weapon_blk_path: &str, weapons_dir: &str) -> Option<Self> {
        // 路径归一（`.blk` → `.blkx` + 小写 + 去 `gameData/Weapons/` 前缀）走 `crate::fm_paths`
        // 的唯一实现：旧代码在这里手写了一遍同样的规则，与 `parse_aircraft` 的 `fmFile` 分支
        // 各写各的 —— 两处"恰好都对"是巧合，现在由同一个函数 + 回归测试锁住（D14）。
        let relative_path = weapon_ref_stem(weapon_blk_path);
        let path = Path::new(weapons_dir).join(normalize_weapon_ref(weapon_blk_path));
        if path.exists() {
            let content = std::fs::read_to_string(&path).ok()?;
            let data = parse_fm_text(&content).ok()?;
            return Some(Weapon::parse_from_data(&data, &relative_path, ""));
        }
        None
    }

    fn pick_bullet_mass(data: &HashMap<String, BlkxValue>, preset: &str) -> f64 {
        if preset.is_empty() {
            return get_f64_first(data, &["BulletMass", "bulletMass", "bullet.mass"]).unwrap_or(0.0);
        }
        let pattern = format!("_{}.bullet", preset);
        let mut total = 0.0f64;
        let mut count = 0u32;
        for key in data.keys() {
            if key.ends_with(".mass") && key.contains(&pattern) {
                if let Some(v) = get_f64_first(data, &[key]) {
                    total += v;
                    count += 1;
                }
            }
        }
        if count > 0 {
            total / count as f64
        } else {
            get_f64_first(data, &["BulletMass", "bulletMass", "bullet.mass"]).unwrap_or(0.0)
        }
    }
    pub fn parse_from_data(data: &HashMap<String, BlkxValue>, name: &str, preset: &str) -> Self {
        let bullet_mass = Self::pick_bullet_mass(data, preset);
        let belts = Self::parse_belts(data);
        Self {
            name: name.to_string(),
            mass: get_f64_first(data, &["Mass", "mass", "rocket.mass"]).unwrap_or(0.0),
            bullet_mass,
            caliber: get_f64_first(data, &["Calibre", "calibre", "Caliber", "bullet.caliber", "rocket.caliber"]).unwrap_or(0.0),
            shot_freq: get_f64_first(data, &["ShotFreq", "shotFreq"]).unwrap_or(0.0),
            bullets_cluster: get_f64_first(data, &["BulletsCluster", "bulletsCluster"]).unwrap_or(1.0) as usize,
            ammo_count: get_f64_first(data, &["bullets", "Bullets"]).unwrap_or(0.0) as usize,
            weapon_type: get_string_first(data, &["Type", "type"])
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            belts,
        }
    }

    fn get_p2_first(data: &HashMap<String, BlkxValue>, keys: &[&str]) -> f64 {
        for key in keys {
            if let Some(v) = data.get(*key) {
                let vals = v.as_f64_vec();
                if !vals.is_empty() {
                    return vals[0];
                }
            }
        }
        0.0
    }

    pub fn parse_belts(data: &HashMap<String, BlkxValue>) -> Vec<BeltInfo> {
        let mut prefix_set: HashSet<String> = HashSet::new();
        for key in data.keys() {
            if key.ends_with(".mass") && key.contains(".bullet") {
                if let Some(pos) = key.find(".bullet") {
                    prefix_set.insert(key[..pos].to_string());
                }
            }
        }
        let mut prefixes: Vec<String> = prefix_set.into_iter().collect();
        prefixes.sort_by(|a, b| {
            if a.is_empty() { std::cmp::Ordering::Less }
            else if b.is_empty() { std::cmp::Ordering::Greater }
            else { a.cmp(b) }
        });

        let mut belts = Vec::new();
        for prefix in &prefixes {
            let belt_name = if prefix.is_empty() { "default".to_string() } else { prefix.clone() };
            let dot = if prefix.is_empty() { String::new() } else { format!("{}.", prefix) };

            let mut bullet_indices: Vec<usize> = Vec::new();
            for key in data.keys() {
                if let Some(rest) = key.strip_prefix(&format!("{}bullet", dot)) {
                    if rest.ends_with(".mass") {
                        let idx = if rest.starts_with('[') {
                            rest.trim_start_matches('[')
                                .split(']')
                                .next()
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0)
                        } else if rest == ".mass" {
                            0
                        } else {
                            continue;
                        };
                        if !bullet_indices.contains(&idx) {
                            bullet_indices.push(idx);
                        }
                    }
                }
            }
            bullet_indices.sort();

            let mut bullets = Vec::new();
            for &idx in &bullet_indices {
                let base = if idx == 0 {
                    format!("{}bullet", dot)
                } else {
                    format!("{}bullet[{}]", dot, idx)
                };

                let bullet_type = get_string_first(data, &[&format!("{}.bulletType", base)]).unwrap_or_default();
                let mass = get_f64_first(data, &[&format!("{}.mass", base)]).unwrap_or(0.0);
                let armor_power = Self::get_p2_first(data, &[
                    &format!("{}.armorpower.ArmorPower0m", base),
                    &format!("{}.armorpower.ArmorPower10m", base),
                    &format!("{}.armorPower", base),
                ]);
                let hit_power = Self::get_p2_first(data, &[
                    &format!("{}.hitpower.HitPower10m", base),
                    &format!("{}.hitpower.HitPower100m", base),
                    &format!("{}.hitpower.HitPower0m", base),
                    &format!("{}.nearHitPower", base),
                ]);
                let hit_power_mult = get_f64_first(data, &[&format!("{}.hitPowerMult", base)]).unwrap_or(1.0);
                let explode_hit_power = get_f64_first(data, &[&format!("{}.explodeHitPower", base)]).unwrap_or(0.0);
                let explode_armor_power = get_f64_first(data, &[&format!("{}.explodeArmorPower", base)]).unwrap_or(0.0);
                let explode_radius = Self::get_p2_first(data, &[&format!("{}.explodeRadius", base)]);
                let fire_chance = get_f64_first(data, &[&format!("{}.onHitChanceMultFire", base)]).unwrap_or(0.0);

                bullets.push(BulletInfo {
                    bullet_type,
                    mass,
                    armor_power,
                    hit_power,
                    hit_power_mult,
                    explode_hit_power,
                    explode_armor_power,
                    explode_radius,
                    fire_chance,
                });
            }

            belts.push(BeltInfo {
                name: belt_name,
                bullets,
            });
        }
        belts
    }
}

pub fn load_weapon(weapon_name: &str, weapons_dir: &str) -> Option<Weapon> {
    let name = weapon_name.to_lowercase();
    let path = Path::new(weapons_dir).join(normalize_blkx_ref(&name));
    if path.exists() {
        let content = std::fs::read_to_string(&path).ok()?;
        let data = parse_fm_text(&content).ok()?;
        return Some(Weapon::parse_from_data(&data, &name, ""));
    }
    None
}

pub fn load_weapon_with_preset(weapon_name: &str, preset: &str, weapons_dir: &str) -> Option<Weapon> {
    let name = weapon_name.to_lowercase();
    let path = Path::new(weapons_dir).join(normalize_blkx_ref(&name));
    if path.exists() {
        let content = std::fs::read_to_string(&path).ok()?;
        let data = parse_fm_text(&content).ok()?;
        return Some(Weapon::parse_from_data(&data, &name, preset));
    }
    None
}


pub fn load_weapons_from_fm(
    data: &HashMap<String, BlkxValue>,
    weapons_dir: &str,
) -> Vec<Weapon> {
    let mut weapons = Vec::new();

    for (key, value) in data {
        if key.contains("blk") || key.contains("Weapon") {
            if let Some(weapon_path) = extract_weapon_path(&value.as_string()) {
                if let Some(weapon) = load_weapon(&weapon_path, weapons_dir) {
                    weapons.push(weapon);
                }
            }
        }
    }

    weapons
}

fn extract_weapon_path(blk_path: &str) -> Option<String> {
    // 去前缀（`gameData/Weapons/`、`gameData/FlightModels/weaponPresets/`，大小写不敏感）
    // + 小写 + 去扩展名：唯一实现在 `crate::fm_paths`，两条数据源共用。
    Some(weapon_ref_stem(blk_path))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_gsh23l() {
        // 数据目录来自 datamine，不入库（.gitignore: resource/data/*）：
        // 用 CARGO_MANIFEST_DIR 定位（不写死本机绝对路径），
        // 外部贡献者没有数据时跳过而不是失败。数据根在 crate 的上一级 → resource/data。
        let weapons_dir =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../resource/data/gamedata/weapons");
        if !std::path::Path::new(weapons_dir).is_dir() {
            eprintln!("跳过 test_load_gsh23l：缺少数据目录 {weapons_dir}（datamine 数据不入库）");
            return;
        }
        let weapon = load_weapon("cannongsh_23l", weapons_dir);
        assert!(weapon.is_some());
        let weapon = weapon.unwrap();
        assert!(weapon.mass > 0.0);
        assert_eq!(weapon.ammo_count, 450);
    }

    #[test]
    fn test_extract_weapon_path() {
        // 目录部分也一并小写（磁盘上的 basename 全小写；保留 `rocketGuns` 原样会在 Linux 上 404 ——
        // 靠 `load_weapon` 再转一次小写兜住 —— Windows 大小写不敏感掩盖了它，Linux 上要靠
        // 归一才对得上）。返回值只用于拼路径，两条写法指向同一个文件。
        assert_eq!(
            extract_weapon_path("gameData/Weapons/cannonGSh_23L.blk"),
            Some("cannongsh_23l".to_string())
        );
        assert_eq!(
            extract_weapon_path("gameData/Weapons/cannonGSh_23L.blkx"),
            Some("cannongsh_23l".to_string())
        );
        assert_eq!(
            extract_weapon_path("gameData/Weapons/rocketGuns/su_9m14m_default.blk"),
            Some("rocketguns/su_9m14m_default".to_string())
        );
        assert_eq!(
            extract_weapon_path("gameData/FlightModels/weaponPresets/A-20G_default.blk"),
            Some("a-20g_default".to_string())
        );
    }
}
