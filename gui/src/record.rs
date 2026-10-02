//! `.wpr`（飞行记录）解析与导出：复用 `wp8f-logger`（`default-features = false` 只带容器，
//! `convert` feature 提供纯函数转换器），控制台不重复实现格式。
//!
//! 回放 JSON：坐标是世界坐标 km（向东/向北为正），换算数据全部带上，前端不推算经纬度。
//!
//! ```json
//! {"format":"wpr","planar":true,"coords":"world_km_north_up",
//!  "t0":<ms>,"t1":<ms>,"aircraft":"f_16c","groups":2,"poll_hz":10.0,
//!  "map":{"x0_km":..,"x1_km":..,"y0_km":..,"y1_km":..,          // 地图矩形（同一个世界坐标系）
//!         "width_m":..,"height_m":..,"min_x_m":..,"min_y_m":..,  // 归一化×尺寸 = 米；+min_* = 世界
//!         "grid_zero":[..],"grid_steps":[..],
//!         "img_w":..,"img_h":..,"mime":"image/jpeg","has_image":true},
//!  "objects":[{"id":"0","name":"f_16c","role":"player",
//!              "frames":[[t_ms,east_km,north_km,alt_m,roll,pitch,yaw,ias_ms,tas_ms,ny,aoa,mach], ...]}]}
//! ```
//!
//! * 每组 CSV = 一个对象，组 0 = 玩家（`role: "player"`）；
//! * 第 2/3 槽 = 世界坐标 km（`min_* + 归一化 × maxsize`，见 `MapMeta::norm_to_world_km`）；
//! * 底图不内联（base64 会膨胀 33% 且每次加入文件都要重传）：前端拿
//!   `/api/replay/map?file=<名字>` 当图片 URL（见 [`read_map_image`]）；
//! * 没带底图时 `has_image = false` → 回放不铺底面，退化成 z=0 网格线；
//! * `pitch` 统一"抬头为正"，`ias`/`tas` 统一 m/s（只服务前端渲染契约）。

use serde_json::{json, Value};
use std::path::Path;
use wp8f_logger::{convert, wpr};

/// `objects[].frames` 的列名与顺序 —— 与下面 `json!([...])` 的投影一一对应，
/// 随响应一起给前端（前端按下标建映射，列序改动只改这一处）。
pub const FRAME_COLUMNS: [&str; 12] = [
    "t_ms", "east_km", "north_km", "alt_m", "roll_deg", "pitch_deg",
    "heading_deg", "ias_ms", "tas_ms", "ny", "aoa_deg", "mach",
];

/// 读并解包一个 `.wpr`。
fn read_record(path: &Path) -> Result<wpr::Record, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
    wpr::Record::decode(&bytes)
}

/// `.wpr` → 回放 JSON（每组 CSV = 一个对象，**组 0 = 玩家**）。
///
/// 坐标用**相对地图 0 点的 km**（向东/向北为正），换算与 HUD 地图面板同一口径：
/// `MapMeta::norm_to_km` = 归一化 × maxsize（y 竖直翻转）。前端只按 km 画，不再做经纬度推算。
pub fn parse_wpr_file(path: &Path) -> Result<Value, String> {
    let rec = read_record(path)?;
    let m = &rec.meta.map;
    let player = rec.player().ok_or_else(|| "记录里一组 CSV 都没有".to_string())?;
    if player.frames() == 0 {
        return Err("记录里没有任何帧（是空录制？）".to_string());
    }

    let mut objects: Vec<Value> = Vec::with_capacity(rec.groups.len());
    let (mut t_min, mut t_max) = (u64::MAX, 0u64);
    for (i, g) in rec.groups.iter().enumerate() {
        let rows = g.rows()?;
        if rows.is_empty() {
            continue; // 空组不算对象（记录线程也不会写没进过行的组）
        }
        let frames: Vec<Value> = rows
            .iter()
            .map(|r| {
                let v = &r.v;
                let (east_km, north_km) = m.norm_to_world_km(v[wpr::col::X], v[wpr::col::Y]);
                json!([
                    r.time_ns / 1_000_000,                       // t（毫秒）
                    east_km,                                     // 世界坐标 km（东为正）
                    north_km,                                    // 世界坐标 km（北为正）
                    v[wpr::col::ALTITUDE],
                    v[wpr::col::ROLL],
                    -v[wpr::col::PITCH],                         // 抬头为正（渲染契约）
                    v[wpr::col::HEADING],
                    v[wpr::col::IAS] / 3.6,                      // m/s（渲染契约）
                    v[wpr::col::TAS] / 3.6,
                    v[wpr::col::NY],
                    v[wpr::col::AOA],
                    v[wpr::col::MACH],
                ])
            })
            .collect();
        t_min = t_min.min(rows[0].time_ns / 1_000_000);
        t_max = t_max.max(rows[rows.len() - 1].time_ns / 1_000_000);
        let a = g.aircraft();
        let name = if a.is_empty() { rec.meta.aircraft.clone() } else { a };
        objects.push(json!({
            "id": i.to_string(),
            "name": name,
            "role": if i == 0 { "player" } else { "other" },   // 组 0 = 玩家飞机
            "frames": frames,
        }));
    }
    if objects.is_empty() {
        return Err("记录里没有任何帧（是空录制？）".to_string());
    }
    let has_map = !rec.map_img.is_empty();
    let (wx0, wx1, wy0, wy1) = m.world_rect_km();

    Ok(json!({
        "format": "wpr",
        "planar": true,
        // 坐标口径写在响应里：前端不必猜"相对谁"（不能拿玩家首帧当原点）
        "coords": "world_km_north_up",
        // `objects[].frames` 每一列的含义（顺序即下标）：前端按下标建映射，不再抄魔数
        "columns": FRAME_COLUMNS,
        "t0": t_min as i64,
        "t1": t_max as i64,
        "aircraft": rec.meta.aircraft,
        "groups": rec.groups.len(),
        "poll_hz": rec.meta.poll_hz,
        "duration_secs": rec.duration_secs(),
        "map": {
            // 地图矩形（世界坐标 km，北为正）——底图四角就铺在这里
            "x0_km": wx0,
            "x1_km": wx1,
            "y0_km": wy0,
            "y1_km": wy1,
            // mappanel 换算的原始数据：归一化 × 尺寸 = 米，再加 min_* = 世界坐标
            "width_m": m.width_m,
            "height_m": m.height_m,
            "min_x_m": m.min_x_m,
            "min_y_m": m.min_y_m,
            "grid_zero": m.grid_zero,
            "grid_steps": m.grid_steps,
            "img_w": m.img_w,
            "img_h": m.img_h,
            "mime": m.img_mime,
            // 底图字节走 /api/replay/map（只给标志，不给内容）。
            // **`has_image = false` 时前端不铺底面地图**（用户的明确口径：记录不带底图就不渲染底面，
            // 不去借别的记录的图当"假底图"）——退化成 z=0 网格线。
            "has_image": has_map,
        },
        "objects": objects,
    }))
}

/// `.wpr` 内嵌的地图底图原始字节 + MIME（`/api/replay/map` 直接当图片响应体）。
///
/// 只解**容器头 + meta**（`ContainerHead`，不碰 CSV）：几百 KB 的 CSV 对"取一张图"毫无用处。
/// 记录没带底图（`img_len = 0`）时返回错误（404）——**不去别处借图**：
/// wpr 不带底图就不渲染底面地图（借来的图会被当成本场底图）。
pub fn read_map_image(path: &Path) -> Result<(String, Vec<u8>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取失败: {e}"))?;
    let head = wpr::ContainerHead::decode(&bytes)?;
    if head.img_len() == 0 {
        return Err("这份记录里没有地图底图".to_string());
    }
    let mime = if head.meta.map.img_mime.trim().is_empty() {
        "application/octet-stream".to_string()
    } else {
        head.meta.map.img_mime.clone()
    };
    Ok((mime, head.image(&bytes).to_vec()))
}

/// 导出：`format` = `acmi`（TacView ACMI，**多机组全带上**）/ `csv`（TacView Real-life CSV，
/// 单机格式 → 只出玩家组）/ `flat`（玩家组自带的那份 CSV）。
/// 返回（建议文件名后缀、文本内容）：ACMI/CSV 需要零点经纬度，FlatCSV 不需要。
pub fn convert_wpr_file(
    path: &Path,
    format: &str,
    lat: f64,
    lon: f64,
) -> Result<(&'static str, String), String> {
    // 先校验格式再读盘：参数写错了不该等读完几百 KB 才报错（也不该报成"读取失败"）
    if !matches!(format, "acmi" | "csv" | "flat") {
        return Err(format!("未知导出格式 {format:?}（支持 acmi / csv / flat）"));
    }
    let rec = read_record(path)?;
    let origin = convert::Origin::new(lat, lon);
    Ok(match format {
        "acmi" => (".acmi", convert::to_tacview_acmi(&rec, origin)),
        "csv" => ("_tacview.csv", convert::to_tacview_csv(&rec, origin)),
        // FlatCSV：玩家组（组 0）自带的那份 CSV，原样输出（零换算）
        _ => (".csv", convert::flat_csv(&rec)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wp8f_logger::wpr::{MapMeta, RecordMeta, RecordRow, NUM_COLS};

    #[test]
    fn unknown_convert_format_is_rejected() {
        assert!(convert_wpr_file(Path::new("不存在.wpr"), "acmi2", 0.0, 0.0).is_err());
    }

    #[test]
    fn missing_file_reports_read_error() {
        assert!(convert_wpr_file(Path::new("不存在.wpr"), "flat", 0.0, 0.0).is_err());
        assert!(parse_wpr_file(Path::new("不存在.wpr")).is_err());
    }

    /// 端到端：自己造一份 **两组** `.wpr`（走 logger 的 encode）→ 平面坐标必须与地图换算一致，
    /// 且每组各出一个对象（组 0 = 玩家）。
    #[test]
    fn frames_are_world_km_from_map_meta() {
        use wp8f_logger::wpr::CsvPool;
        let dir = std::env::temp_dir().join("wp8f_wpr_gui_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.wpr");

        let pool_of = |aircraft: &str, rows: &[(u64, f64, f64)]| {
            let mut pool = CsvPool::new(0);
            for (t, x, y) in rows {
                let mut v = [0f64; NUM_COLS];
                v[wp8f_logger::wpr::col::X] = *x;
                v[wp8f_logger::wpr::col::Y] = *y;
                v[wp8f_logger::wpr::col::ALTITUDE] = 3000.0;
                v[wp8f_logger::wpr::col::IAS] = 540.0;
                v[wp8f_logger::wpr::col::PITCH] = -2.5;
                pool.push(&RecordRow { time_ns: *t, type_str: aircraft.to_string(), v });
            }
            pool.into_group()
        };
        let rec = wpr::Record {
            meta: RecordMeta {
                poll_hz: 10.0,
                map: MapMeta {
                    width_m: 100_000.0,
                    height_m: 80_000.0,
                    // 地图矩形西南角的世界坐标：中心 = 世界 0 点（游戏常见口径）
                    min_x_m: -50_000.0,
                    min_y_m: -40_000.0,
                    grid_zero: [6_494.3, 19_547.5],
                    grid_steps: [5_500.0, 5_500.0],
                    img_w: 2,
                    img_h: 2,
                    img_mime: "image/jpeg".into(),
                },
                aircraft: "f_16c".into(),
                ..Default::default()
            },
            groups: vec![
                pool_of("f_16c", &[
                    (1_700_000_000_000_000_000, 0.5, 0.5),
                    (1_700_000_000_100_000_000, 0.6, 0.25),
                ]),
                // 友机：同一套归一化地图坐标 → 相对玩家首帧同样是东 5 km / 北 8 km
                pool_of("su_27", &[(1_700_000_000_050_000_000, 0.55, 0.4)]),
            ],
            map_img: vec![0xFF, 0xD8, 0xFF],
        };
        std::fs::write(&path, rec.encode().unwrap()).unwrap();

        let v = parse_wpr_file(&path).expect("解析 .wpr");
        assert_eq!(v["format"], "wpr");
        assert_eq!(v["planar"], true);
        assert_eq!(v["aircraft"], "f_16c");
        assert_eq!(v["groups"], 2, "两组 CSV");
        let objs = v["objects"].as_array().unwrap();
        assert_eq!(objs.len(), 2, "每组一个对象");
        assert_eq!((objs[0]["id"].as_str(), objs[0]["role"].as_str()), (Some("0"), Some("player")));
        assert_eq!((objs[1]["name"].as_str(), objs[1]["role"].as_str()), (Some("su_27"), Some("other")));
        let frames = objs[0]["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 2);
        // 坐标 = 世界坐标 km（`min_* + 归一化 × 尺寸`，mappanel 口径），不是相对玩家首帧：
        // 首帧 (0.5,0.5) 在地图中心 = 世界 0 点 → (0, 0)
        assert_eq!(v["coords"], "world_km_north_up");
        assert!((frames[0][1].as_f64().unwrap()).abs() < 1e-9, "东 = −50 + 0.5×100 = 0");
        assert!((frames[0][2].as_f64().unwrap()).abs() < 1e-9, "北 = −40 + 0.5×80 = 0");
        assert_eq!(frames[0][0].as_i64().unwrap(), 1_700_000_000_000);
        // 第二帧：x 0.6 → 东 10 km；y 0.25 → 北 (−40 + 0.75×80) = 20 km
        assert!((frames[1][1].as_f64().unwrap() - 10.0).abs() < 1e-9, "{frames:?}");
        assert!((frames[1][2].as_f64().unwrap() - 20.0).abs() < 1e-9, "{frames:?}");
        // pitch 抬头为正（记录里 -2.5 → 回放 2.5），速度 m/s
        assert!((frames[0][5].as_f64().unwrap() - 2.5).abs() < 1e-9);
        assert!((frames[0][7].as_f64().unwrap() - 150.0).abs() < 1e-9);
        // 友机帧：同一套地图坐标 → x 0.55 → 东 5 km；y 0.40 → 北 (−40 + 0.6×80) = 8 km
        let peer = objs[1]["frames"][0].as_array().unwrap();
        assert!((peer[1].as_f64().unwrap() - 5.0).abs() < 1e-9, "{peer:?}");
        assert!((peer[2].as_f64().unwrap() - 8.0).abs() < 1e-9, "{peer:?}");
        // 时间轴取全组并集（友机那帧在 1_700_000_000_050 毫秒）
        assert_eq!(v["t0"].as_i64().unwrap(), 1_700_000_000_000);
        assert_eq!(v["t1"].as_i64().unwrap(), 1_700_000_000_100);
        // 地图矩形 = 世界坐标 km（西南角 −50/−40 ⇒ 宽 100 / 高 80）
        let m = &v["map"];
        assert_eq!(m["x0_km"].as_f64().unwrap(), -50.0);
        assert_eq!(m["x1_km"].as_f64().unwrap(), 50.0);
        assert_eq!(m["y0_km"].as_f64().unwrap(), -40.0);
        assert_eq!(m["y1_km"].as_f64().unwrap(), 40.0);
        // 换算数据必须原样带给前端：尺寸 + 西南角世界坐标 + 网格
        assert_eq!(m["width_m"].as_f64().unwrap(), 100_000.0);
        assert_eq!(m["height_m"].as_f64().unwrap(), 80_000.0);
        assert_eq!(m["min_x_m"].as_f64().unwrap(), -50_000.0);
        assert_eq!(m["min_y_m"].as_f64().unwrap(), -40_000.0);
        assert_eq!(m["grid_steps"][0].as_f64().unwrap(), 5_500.0);
        // 底图不再内联：JSON 里只给标志，字节走 read_map_image（/api/replay/map 的响应体）
        assert_eq!(m["has_image"], true);
        assert!(m.get("image").is_none(), "底图不应再内联进回放 JSON");
        let (mime, bytes) = read_map_image(&path).expect("取出底图字节");
        assert_eq!(mime, "image/jpeg");
        assert_eq!(bytes, vec![0xFF, 0xD8, 0xFF]);

        // 三种导出都能跑通：FlatCSV = 玩家组的 CSV；ACMI 把两架飞机都带上
        let (ext, text) = convert_wpr_file(&path, "flat", 0.0, 0.0).unwrap();
        assert_eq!(ext, ".csv");
        assert_eq!(text, rec.player().unwrap().csv);
        assert!(!text.contains("su_27"), "FlatCSV 只有玩家组");
        let (ext2, acmi) = convert_wpr_file(&path, "acmi", 55.7558, 37.6173).unwrap();
        assert_eq!(ext2, ".acmi");
        assert!(acmi.starts_with("FileType=text/acmi/tacview"), "{acmi}");
        let (_, tv) = convert_wpr_file(&path, "csv", 55.7558, 37.6173).unwrap();
        assert!(tv.starts_with("Time,Longitude,Latitude"), "{tv}");
        std::fs::remove_file(&path).ok();
    }

    /// **记录没带底图 ⇒ 不铺底面地图**。
    ///
    /// 断言三件事：
    /// 1. `map.has_image = false`，并且 JSON 里**没有** `fallback_image` 字段 ——
    ///    "借 logs 里另一份记录的底图"那套回退逻辑已彻底删除（前端据此不铺底面，退化成网格线）；
    /// 2. `/api/replay/map` 对这份记录只能报错（404）——不伪造、不外借字节；
    /// 3. 即便同目录里就躺着一份**几何完全一致、带底图**的记录，也不会被借走
    ///    （借来的图会让人误以为是本场底图）。
    #[test]
    fn record_without_map_image_gets_no_ground_map_at_all() {
        use wp8f_logger::wpr::CsvPool;
        let dir = std::env::temp_dir().join("wp8f_wpr_nomap_test");
        std::fs::create_dir_all(&dir).unwrap();
        for p in std::fs::read_dir(&dir).unwrap().flatten() {
            let _ = std::fs::remove_file(p.path());
        }
        let mk_csv = || {
            let mut pool = CsvPool::new(0);
            let mut v = [0f64; NUM_COLS];
            v[wp8f_logger::wpr::col::ALTITUDE] = 3000.0;
            pool.push(&RecordRow { time_ns: 1_700_000_000_000_000_000, type_str: "f_16c".into(), v });
            pool.into_group()
        };
        let map = MapMeta {
            width_m: 131_072.0,
            height_m: 131_072.0,
            min_x_m: -65_536.0,
            min_y_m: -65_536.0,
            grid_zero: [6_494.3, 19_547.5],
            grid_steps: [5_500.0, 5_500.0],
            img_w: 2048,
            img_h: 2048,
            img_mime: "image/jpeg".into(),
        };
        let meta_of = |map: MapMeta| RecordMeta { poll_hz: 30.0, map, aircraft: "f_16a".into(), ..Default::default() };

        // ① 没带底图的记录（img_len = 0，用户实测那种）
        let nomap = dir.join("nomap.wpr");
        std::fs::write(&nomap, wpr::Record {
            meta: meta_of(map.clone()), groups: vec![mk_csv()], map_img: Vec::new(),
        }.encode().unwrap()).unwrap();

        // ② 同目录里再放一份**几何一致、带底图**的记录（旧回退逻辑会挑中它）
        let withmap = dir.join("withmap.wpr");
        std::fs::write(&withmap, wpr::Record {
            meta: meta_of(map), groups: vec![mk_csv()], map_img: vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3],
        }.encode().unwrap()).unwrap();

        let v = parse_wpr_file(&nomap).unwrap();
        assert_eq!(v["map"]["has_image"], false, "没带底图的记录：has_image = false");
        assert!(
            v["map"].get("fallback_image").is_none(),
            "没有「回退底图」字段：{}", v["map"]
        );
        assert!(read_map_image(&nomap).is_err(), "没带底图时取图必须失败");
        // 带底图的那份自己照样能取
        let (mime, bytes) = read_map_image(&withmap).unwrap();
        assert_eq!((mime.as_str(), bytes.len()), ("image/jpeg", 7));

        for p in std::fs::read_dir(&dir).unwrap().flatten() {
            let _ = std::fs::remove_file(p.path());
        }
    }
}
