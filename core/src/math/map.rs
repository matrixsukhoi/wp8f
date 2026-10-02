//! 地图坐标与距离、机场/点位取点（由原 `map.rs` + `map_utils.rs` 合并迁入）。
//! **只搬家不改逻辑**：函数体、签名、可见性、常量值全部保持原样。

use crate::parser::{MapInfo, MapObjItem};

pub fn normalize_to_map_coords(x: f64, y: f64, map_info: &MapInfo) -> (f64, f64) {
    (x * map_info.maxsize[0], y * map_info.maxsize[1])
}

pub fn calc_distance_to_player(
    player_x: f64,
    player_y: f64,
    target_x: f64,
    target_y: f64,
    map_info: &MapInfo,
) -> f64 {
    let (player_map_x, player_map_y) = normalize_to_map_coords(player_x, player_y, map_info);
    let (target_map_x, target_map_y) = normalize_to_map_coords(target_x, target_y, map_info);

    let dx = target_map_x - player_map_x;
    let dy = target_map_y - player_map_y;

    (dx * dx + dy * dy).sqrt()
}

pub fn calc_weighted_distance(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    scale_x: f64,
    scale_y: f64,
) -> f64 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    ((dx * dx) * scale_x * scale_x + (dy * dy) * scale_y * scale_y).sqrt()
}

pub fn calc_map_zone(player_x: f64, player_y: f64, map_info: &MapInfo) -> String {
    // 算法单一实现于 disp（友军面板区域推导共用，避免两边漂移）
    wp8f_disp::map_zone_label(
        player_x,
        player_y,
        map_info.maxsize,
        map_info.map_min,
        map_info.grid_zero,
        map_info.grid_steps,
    )
}

#[derive(Debug, Clone)]
pub struct ObjectCoords {
    pub x: f64,
    pub y: f64,
    pub dx: f64,
    pub dy: f64,
}

impl ObjectCoords {
    pub fn new(x: f64, y: f64, dx: f64, dy: f64) -> Self {
        Self { x, y, dx, dy }
    }
}

pub fn extract_airfield_origin_coords(item: &MapObjItem) -> Option<ObjectCoords> {
    if item.obj_type.as_str() == "airfield" {
        if let (Some(sx), Some(sy)) = (item.sx, item.sy) {
            return Some(ObjectCoords::new(sx, sy, 0.0, 0.0));
        } else if let (Some(x), Some(y)) = (item.x, item.y) {
            return Some(ObjectCoords::new(
                x,
                y,
                item.dx.unwrap_or(0.0),
                item.dy.unwrap_or(0.0),
            ));
        }
    }
    None
}

pub fn extract_point_coords(item: &MapObjItem) -> Option<ObjectCoords> {
    if let (Some(x), Some(y)) = (item.x, item.y) {
        Some(ObjectCoords::new(
            x,
            y,
            item.dx.unwrap_or(0.0),
            item.dy.unwrap_or(0.0),
        ))
    } else {
        None
    }
}

pub fn extract_map_object_coords(item: &MapObjItem) -> Option<ObjectCoords> {
    if item.obj_type.as_str() == "airfield" {
        extract_airfield_origin_coords(item)
    } else {
        extract_point_coords(item)
    }
}
