//! 纯函数转换器：`.wpr` → 三种导出格式（只有控制台需要，故放在 `convert` feature 下）。
//!
//! | 函数 | 输出 | 需要零点经纬度 |
//! |---|---|---|
//! | [`to_tacview_acmi`] | TacView ACMI 2.2（`FileVersion=2.2`，纪元时间基准，**多机组按时间合流**） | 是 |
//! | [`to_tacview_csv`] | TacView Real-life CSV（18 列、绝对经纬度，**单机格式 → 只导玩家组**） | 是 |
//! | [`flat_csv`] | **记录里组 0（玩家）的那份 CSV**（wp8f 口径，原样，零换算） | 否 |
//!
//! 记录里存的是 wp8f 自己的口径（归一化地图坐标、km/h、百分比操纵面、纳秒时间、HUD 俯仰符号），
//! **只有这里**才把它们换算成目标格式要求的样子：纳秒 → 秒 / 毫秒、km/h → m/s、
//! 百分比 → ratio、`pitch` 取负（ACMI 抬头为正）、归一化坐标 → 经纬度。
//! 三个函数都不做 I/O、不碰全局状态。

use crate::wpr::{col, fin, sanitize, Record, RecordRow, NUM_COLS};

/// 地球平均半径（米）——球面近似，与 HUD 侧的地图换算同口径。
pub const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// 零点经纬度（地图中心 `(0.5, 0.5)` 对应的真实位置）。默认莫斯科市中心
/// （与旧配置 `origin_lat/origin_lon` 的默认值一致，方便直接对比老录制）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Origin {
    pub lat: f64,
    pub lon: f64,
}

impl Default for Origin {
    fn default() -> Self {
        Self { lat: 55.7558, lon: 37.6173 }
    }
}

impl Origin {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }
}

/// TacView Real-life CSV 表头（18 列）。
pub const TACVIEW_CSV_HEADER: &str = "Time,Longitude,Latitude,Altitude,Roll,Pitch,Yaw,\
                                      IAS,TAS,Mach,AOA,AOS,Ny,Throttle,AirBrakes,Flaps,\
                                      LandingGear,FuelWeight";

/// 归一化地图坐标 → 经纬度：与 HUD 地图面板同一套换算（**归一化 × maxsize**），
/// 再以零点经纬度为原点换算成经纬度（球面近似）。
///
/// 零点经纬度按约定对应**地图中心** `(0.5, 0.5)`（见 [`Origin`]），偏移量按
/// `MapMeta::norm_to_world_m` 算出的世界米再减去地图中心的世界米得到。
pub fn to_lon_lat(rec: &Record, origin: Origin, x: f64, y: f64) -> (f64, f64) {
    let m = &rec.meta.map;
    // 地图中心的世界坐标（= 零点经纬度所在处）：矩形西南角 + 半个尺寸
    let cx = m.min_x_m + m.width_m / 2.0;
    let cy = m.min_y_m + m.height_m / 2.0;
    let (wx, wy) = m.norm_to_world_m(x, y);
    let d_east = if wx.is_finite() && cx.is_finite() { wx - cx } else { 0.0 };
    let d_north = if wy.is_finite() && cy.is_finite() { wy - cy } else { 0.0 };
    let cos_lat = origin.lat.to_radians().cos();
    let cos_lat = if cos_lat.abs() < 1e-6 { 1e-6 } else { cos_lat };
    let dlat = d_north / EARTH_RADIUS_M * 180.0 / std::f64::consts::PI;
    let dlon = d_east / (EARTH_RADIUS_M * cos_lat) * 180.0 / std::f64::consts::PI;
    (origin.lon + dlon, origin.lat + dlat)
}

/// 组名：组内首行的机型；空则退回元数据里登记的机型。
fn group_name(rows: &[RecordRow], idx: usize, rec: &Record) -> String {
    rows.first()
        .map(|r| r.type_str.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| rec.meta.group_info.get(idx).map(|g| g.aircraft.clone()).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| rec.meta.aircraft.clone())
}

/// `.wpr` → **组 0（玩家）自带的那份 CSV**（wp8f 口径，原样输出，不做任何换算）。
///
/// 多机记录里其他组不在这里展开：一个 CSV 文件装不下多张表（要看别的组用 ACMI 导出）。
pub fn flat_csv(rec: &Record) -> String {
    rec.player().map(|g| g.csv.clone()).unwrap_or_default()
}

/// `.wpr` → TacView ACMI 2.2 文本（多机组按时间合流：`#时间` 单调不减，每个时刻后面跟该时刻的机体行）。
///
/// 对象 id：组 0 = `-1`，组 1 = `-2`……（沿用旧单机导出的 `-1`，多出来的组依次递减）。
pub fn to_tacview_acmi(rec: &Record, origin: Origin) -> String {
    // 每组先解析成行（组 0 = 玩家）；解析失败就当作空组，不让一个坏组毁掉整次导出
    let mut groups: Vec<Vec<RecordRow>> = Vec::with_capacity(rec.groups.len());
    let mut names: Vec<String> = Vec::with_capacity(rec.groups.len());
    for (i, g) in rec.groups.iter().enumerate() {
        let rows = g.rows().unwrap_or_default();
        names.push(group_name(&rows, i, rec));
        groups.push(rows);
    }
    let total: usize = groups.iter().map(|g| g.len()).sum();
    let mut s = String::with_capacity(320 + total * (NUM_COLS * 9));
    s.push_str("FileType=text/acmi/tacview\n");
    s.push_str("FileVersion=2.2\n");
    s.push_str("0,ReferenceTime=1970-01-01T00:00:00Z\n");
    s.push_str(&format!("0,ReferenceLongitude={:.7}\n", fin(origin.lon)));
    s.push_str(&format!("0,ReferenceLatitude={:.7}\n", fin(origin.lat)));
    s.push_str("0,Title=wp8f recording\n");
    s.push_str("0,DataRecorder=wp8f\n");
    s.push_str("0,DataSource=War Thunder 8111\n");

    // 多路归并：每次取"当前时刻最小"的组写一行，时间戳变化时才写 `#时间`
    let mut idx = vec![0usize; groups.len()];
    let mut declared = vec![false; groups.len()];
    let mut last_ns: Option<u64> = None;
    loop {
        let mut pick: Option<usize> = None;
        for (g, rows) in groups.iter().enumerate() {
            let Some(r) = rows.get(idx[g]) else { continue };
            match pick {
                Some(p) if groups[p][idx[p]].time_ns <= r.time_ns => {}
                _ => pick = Some(g),
            }
        }
        let Some(g) = pick else { break };
        let r = &groups[g][idx[g]];
        idx[g] += 1;
        let v = &r.v;
        let (lon, lat) = to_lon_lat(rec, origin, v[col::X], v[col::Y]);
        if last_ns != Some(r.time_ns) {
            // ACMI 的时间是"秒"（3 位小数）：纳秒 → 秒
            s.push_str(&format!("#{:.3}\n", r.time_ns as f64 / 1e9));
            last_ns = Some(r.time_ns);
        }
        let mut line = String::with_capacity(240);
        line.push_str(&format!("-{},", g + 1));
        if !declared[g] {
            line.push_str("Type=Air+FixedWing,");
            line.push_str(&format!("Name={},", sanitize(&names[g])));
            line.push_str(&format!("CallSign=wp8f{},", if g == 0 { String::new() } else { format!("-{g}") }));
            declared[g] = true;
        }
        line.push_str(&format!(
            "T={:.7}|{:.7}|{:.2}|{:.1}|{:.1}|{:.1}",
            fin(lon - origin.lon),
            fin(lat - origin.lat),
            fin(v[col::ALTITUDE]),
            fin(v[col::ROLL]),
            fin(-v[col::PITCH]), // ACMI 抬头为正，记录是 HUD 口径
            fin(v[col::HEADING])
        ));
        line.push_str(&format!(",TAS={:.2}", fin(v[col::TAS] / 3.6)));
        line.push_str(&format!(",IAS={:.2}", fin(v[col::IAS] / 3.6)));
        line.push_str(&format!(",Mach={:.3}", fin(v[col::MACH])));
        line.push_str(&format!(",AOA={:.2}", fin(v[col::AOA])));
        line.push_str(&format!(",AOS={:.2}", fin(v[col::AOS])));
        line.push_str(&format!(",VerticalGForce={:.3}", fin(v[col::NY])));
        line.push_str(&format!(",Throttle={:.3}", fin(v[col::THROTTLE])));
        line.push_str(&format!(",AirBrakes={:.2}", fin(v[col::AIRBRAKE] / 100.0)));
        line.push_str(&format!(",Flaps={:.2}", fin(v[col::FLAPS] / 100.0)));
        line.push_str(&format!(",LandingGear={:.2}", fin(v[col::GEAR] / 100.0)));
        line.push_str(&format!(",FuelWeight={:.1}", fin(v[col::FUEL_KG])));
        s.push_str(&line);
        s.push('\n');
    }
    s
}

/// `.wpr` → TacView Real-life CSV（18 列、绝对经纬度）。**单机格式**：只导玩家组（组 0）。
pub fn to_tacview_csv(rec: &Record, origin: Origin) -> String {
    let rows = rec.player().and_then(|g| g.rows().ok()).unwrap_or_default();
    let mut s = String::with_capacity(120 + rows.len() * 140);
    s.push_str(TACVIEW_CSV_HEADER);
    s.push('\n');
    for r in &rows {
        let v = &r.v;
        let (lon, lat) = to_lon_lat(rec, origin, v[col::X], v[col::Y]);
        s.push_str(&format!(
            "{},{:.7},{:.7},{:.2},{:.1},{:.1},{:.1},{:.2},{:.2},{:.3},{:.2},{:.2},{:.3},{:.2},{:.2},{:.2},{:.2},{:.1}\n",
            // Real-life CSV 的 Time 是毫秒整数：纳秒 → 毫秒
            r.time_ns / 1_000_000,
            fin(lon),
            fin(lat),
            fin(v[col::ALTITUDE]),
            fin(v[col::ROLL]),
            fin(-v[col::PITCH]), // 抬头为正
            fin(v[col::HEADING]),
            fin(v[col::IAS] / 3.6),
            fin(v[col::TAS] / 3.6),
            fin(v[col::MACH]),
            fin(v[col::AOA]),
            fin(v[col::AOS]),
            fin(v[col::NY]),
            fin(v[col::THROTTLE]),
            fin(v[col::AIRBRAKE] / 100.0),
            fin(v[col::FLAPS] / 100.0),
            fin(v[col::GEAR] / 100.0),
            fin(v[col::FUEL_KG]),
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wpr::{col, CsvPool, MapMeta, RecordGroup, RecordMeta, RecordRow, NUM_COLS};

    /// 造一行：按 `col::*` 常量填值（不写魔数，列序改了这里会跟着对）。
    fn row(t_ns: u64, x: f64, y: f64) -> RecordRow {
        let mut v = [0f64; NUM_COLS];
        v[col::X] = x;
        v[col::Y] = y;
        v[col::ALTITUDE] = 3200.0;
        v[col::IAS] = 540.0;
        v[col::TAS] = 612.0;
        v[col::MACH] = 0.87;
        v[col::AOA] = 5.2;
        v[col::AOS] = -1.1;
        v[col::NY] = 3.2;
        v[col::HEADING] = 271.5;
        v[col::ROLL] = 10.3;
        v[col::PITCH] = -2.5;
        v[col::FLAPS] = 50.0;
        v[col::AIRBRAKE] = 0.0;
        v[col::THROTTLE] = 1.05;
        v[col::FUEL_KG] = 3200.5;
        RecordRow { time_ns: t_ns, type_str: "f_16c".to_string(), v }
    }

    fn group(aircraft: &str, rows: &[RecordRow]) -> RecordGroup {
        let mut pool = CsvPool::new(0);
        for r in rows {
            let mut r = r.clone();
            r.type_str = aircraft.to_string();
            pool.push(&r);
        }
        pool.into_group()
    }

    fn rec() -> Record {
        Record {
            meta: RecordMeta {
                poll_hz: 10.0,
                map: MapMeta {
                    width_m: 100_000.0,
                    height_m: 80_000.0,
                    min_x_m: -50_000.0,
                    min_y_m: -40_000.0,
                    img_w: 1024,
                    img_h: 819,
                    img_mime: "image/jpeg".into(),
                    ..MapMeta::default()
                },
                generator: "wp8f".into(),
                ..Default::default()
            },
            groups: vec![group(
                "f_16c",
                &[row(1_700_000_000_000_000_000, 0.5, 0.5), row(1_700_000_000_100_000_000, 0.75, 0.25)],
            )],
            map_img: vec![],
        }
    }

    /// FlatCSV = 组 0（玩家）自带的那份 CSV：表头一致、行数 = 帧数、逐行与 `to_csv_line()` 相同。
    #[test]
    fn flat_csv_is_the_player_group_csv_verbatim() {
        let r = rec();
        let out = flat_csv(&r);
        let mut lines = out.lines();
        assert_eq!(lines.next().unwrap(), crate::wpr::CSV_HEADER, "表头就是记录的表头");
        let body: Vec<&str> = lines.collect();
        let rows = r.player().unwrap().rows().unwrap();
        assert_eq!(body.len(), rows.len(), "行数 = 帧数");
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(body[i], row.to_csv_line(), "第 {i} 行必须原样");
        }
        assert_eq!(out, r.player().unwrap().csv, "与容器里组 0 的 CSV 逐字节一致");
    }

    /// 多机组：FlatCSV / TacView CSV 只出玩家组，ACMI 把每组都带上（id 依次 -1、-2）。
    #[test]
    fn multi_group_exports_cover_every_aircraft() {
        let mut r = rec();
        r.groups.push(group("su_27", &[row(1_700_000_000_050_000_000, 0.4, 0.6)]));

        let flat = flat_csv(&r);
        assert!(!flat.contains("su_27"), "FlatCSV 只有玩家组");
        assert_eq!(flat.lines().count(), 3);

        let o = Origin::new(55.7558, 37.6173);
        let csv = to_tacview_csv(&r, o);
        assert_eq!(csv.lines().count(), 3, "单机格式：表头 + 玩家 2 帧");

        let acmi = to_tacview_acmi(&r, o);
        assert!(acmi.contains("-1,Type=Air+FixedWing,Name=f_16c,"), "{acmi}");
        assert!(acmi.contains("-2,Type=Air+FixedWing,Name=su_27,"), "第二架飞机也要声明：{acmi}");
        assert!(acmi.contains(",CallSign=wp8f-1,"), "非玩家组的呼号带组号");
        assert_eq!(acmi.matches("Type=Air+FixedWing").count(), 2, "每组只声明一次");
        // 8 行头 + 3 帧数据 + 3 个时间行（两组的时刻互不相同 → 3 个不同的 #时间）
        assert_eq!(acmi.lines().count(), 8 + 3 + 3, "{acmi}");
        // 时间必须单调不减（合流后不能倒着写）
        let times: Vec<f64> = acmi
            .lines()
            .filter_map(|l| l.strip_prefix('#'))
            .map(|t| t.parse::<f64>().unwrap())
            .collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]), "时间戳必须单调不减：{times:?}");
        assert_eq!(times.len(), 3, "三个不同时刻只写三行 #时间：{times:?}");
    }

    #[test]
    fn map_center_is_origin_and_axes_follow_the_game() {
        let r = rec();
        let o = Origin::new(55.7558, 37.6173);
        let (lon, lat) = to_lon_lat(&r, o, 0.5, 0.5);
        assert!((lon - o.lon).abs() < 1e-12 && (lat - o.lat).abs() < 1e-12, "中心点 = 零点");
        let (lon_e, _) = to_lon_lat(&r, o, 0.75, 0.5);
        let (_, lat_n) = to_lon_lat(&r, o, 0.5, 0.25);
        assert!(lon_e > o.lon, "向东经度增大");
        assert!(lat_n > o.lat, "y 向南 ⇒ 0.25 是北");
        let exp = 20_000.0 / EARTH_RADIUS_M * 180.0 / std::f64::consts::PI;
        assert!((lat_n - o.lat - exp).abs() < 1e-9, "纬度增量按球面公式");
    }

    /// 零点不在地图中心时（`min_*` ≠ −尺寸/2）换算跟着 min 走，而不是硬编码 0.5。
    #[test]
    fn origin_offset_follows_map_min() {
        let mut r = rec();
        // 地图矩形改成 0..W / 0..H（西南角在世界 0 点）：此时地图中心 = 世界 (50 km, 40 km)，
        // 而零点经纬度按约定对应**地图中心** → 到西北角的偏移只剩一半尺寸
        r.meta.map.min_x_m = 0.0;
        r.meta.map.min_y_m = 0.0;
        let o = Origin::new(0.0, 0.0);
        let (_, lat) = to_lon_lat(&r, o, 0.0, 0.0);
        let exp = 40_000.0 / EARTH_RADIUS_M * 180.0 / std::f64::consts::PI;
        assert!((lat - exp).abs() < 1e-9, "西北角在零点北侧 40 km：{lat} vs {exp}");
        let (lon, _) = to_lon_lat(&r, o, 1.0, 1.0);
        let exp_lon = 50_000.0 / EARTH_RADIUS_M * 180.0 / std::f64::consts::PI;
        assert!((lon - exp_lon).abs() < 1e-9, "东南角在零点东侧 50 km：{lon} vs {exp_lon}");
        // 地图 0 点（归一化 0,0 = 西北角）在西侧 50 km、北侧 40 km 处 —— 与 min_* 完全一致
        let (wx, wy) = r.meta.map.norm_to_world_m(0.0, 0.0);
        assert!((wx - 0.0).abs() < 1e-6 && (wy - 80_000.0).abs() < 1e-6, "{wx},{wy}");
    }

    #[test]
    fn acmi_header_declaration_and_conversions() {
        let s = to_tacview_acmi(&rec(), Origin::new(55.7558, 37.6173));
        assert!(s.starts_with("FileType=text/acmi/tacview\nFileVersion=2.2\n"), "{s}");
        assert!(s.contains("0,ReferenceLongitude=37.6173000"));
        assert!(s.contains("#1700000000.000\n-1,Type=Air+FixedWing,Name=f_16c,CallSign=wp8f,T=0.0000000|0.0000000|3200.00|10.3|2.5|271.5"),
                "俯仰 -2.5 → ACMI 2.5：{s}");
        assert!(s.contains(",TAS=170.00") && s.contains(",IAS=150.00"), "km/h → m/s：{s}");
        assert!(s.contains(",AirBrakes=0.00,Flaps=0.50,LandingGear=0.00"), "百分比 → ratio：{s}");
        assert_eq!(s.matches("Type=Air+FixedWing").count(), 1, "对象声明只出现一次");
        assert_eq!(s.lines().count(), 8 + 2 + 2, "8 行头 + 每帧（时间行 + 数据行）");
    }

    #[test]
    fn tacview_csv_has_18_columns_and_millisecond_time() {
        let s = to_tacview_csv(&rec(), Origin::new(55.7558, 37.6173));
        let mut it = s.lines();
        assert_eq!(it.next().unwrap(), TACVIEW_CSV_HEADER);
        let f: Vec<&str> = it.next().unwrap().split(',').collect();
        assert_eq!(f.len(), 18);
        assert_eq!(f[0], "1700000000000", "Time = 毫秒整数（记录里是纳秒）");
        assert_eq!(f[1], "37.6173000");
        assert_eq!(f[2], "55.7558000");
        assert_eq!(f[5], "2.5", "Pitch 抬头为正");
        assert_eq!(f[7], "150.00", "IAS 在前且为 m/s");
        assert_eq!(f[15], "0.50", "Flaps ratio");
    }

    #[test]
    fn non_finite_values_never_leak_into_text() {
        let mut r = rec();
        let mut pool = CsvPool::new(0);
        let mut bad = row(1_700_000_000_000_000_000, 0.5, 0.5);
        bad.v[3] = f64::NAN;
        bad.v[1] = f64::INFINITY;
        pool.push(&bad);
        r.groups[0] = pool.into_group();
        let acmi = to_tacview_acmi(&r, Origin::default());
        let csv = to_tacview_csv(&r, Origin::default());
        for text in [&acmi, &csv] {
            assert!(!text.contains("NaN") && !text.contains("inf"), "不能出现 NaN/inf");
        }
    }
}
