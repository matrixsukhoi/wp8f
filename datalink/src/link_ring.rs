//! 数据链的两个进程级槽：core 每帧发布本机状态，datalink 线程按上报节拍取最新一版；
//! datalink 线程把服务端应答（全量友军快照）发布出来，HUD 绘制线程每帧取最新消费。
//!
//! 槽就是 `wp8f_ring::Ring`（与 HUD/logger 的帧缓冲区同一个原语）：无锁、无 `Arc`、
//! 无逐帧分配，三端各跑各的线程、谁也不等谁。载荷必须是 POD —— 环形槽直读的前提
//! （本机状态用定长机型字段、友军快照用定长 `Snap`，都不带堆指针）。
//!
//! 槽是 `OnceLock` 常驻：重连时 `reset_tracks()` 推进 generation 并发布一份空快照，
//! 消费端据此识别"断流 / 换局"，不会把上一局的友军留在屏幕上。

use std::sync::OnceLock;

use wp8f_ring::{Poll, Ring};

pub use wp8f_ring::Cursor;

use crate::{write_type, StatePacket, MAX_TRACKS, TYPE_LEN};

/// 本机状态快照：datalink 线程据此构造 `StatePacket` 定期上报。
/// 字段与 `DisplayData` / `MapDisplay` 同名同型（零换算搬进来）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OwnState {
    /// 机型（定长零填充，用 `set_type()` 写入）
    pub aircraft_type: [u8; TYPE_LEN],
    pub ias: f64,
    pub tas: f64,
    pub altitude: f64,
    pub mach: f64,
    pub heading: f64,
    pub vy: f64,
    /// 归一化地图坐标（= `MapDisplay.player_map_x/y`）
    pub x: f64,
    pub y: f64,
    pub poi_enabled: bool,
    pub poi_x: f64,
    pub poi_y: f64,
}

impl OwnState {
    /// 写入机型（零填充、超长截断）。
    pub fn set_type(&mut self, name: &str) {
        write_type(&mut self.aircraft_type, name);
    }
}

/// 友军快照：服务端应答里的 `StatePacket` 全量列表，定长 POD（环形槽里不能放 `Vec`）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Snap {
    packets: [StatePacket; MAX_TRACKS],
    len: usize,
}

impl Snap {
    /// 超出 `MAX_TRACKS` 的部分截断（服务端本来也不会超发，见 `MAX_TRACKS` 的说明）。
    fn from_slice(list: &[StatePacket]) -> Self {
        let n = list.len().min(MAX_TRACKS);
        let mut packets = [StatePacket::default(); MAX_TRACKS];
        packets[..n].copy_from_slice(&list[..n]);
        Self { packets, len: n }
    }

    fn as_slice(&self) -> &[StatePacket] {
        &self.packets[..self.len]
    }
}

static OWN: OnceLock<Ring<OwnState>> = OnceLock::new();
static TRACKS: OnceLock<Ring<Snap>> = OnceLock::new();

/// datalink 线程建立时调用：建好两个槽（进程内只生效一次；重连复用同一对槽）。
/// 槽一旦存在，`is_active()` 即为 `true`。
pub fn init_slots() {
    OWN.get_or_init(Ring::new);
    TRACKS.get_or_init(Ring::new);
}

/// **core 主循环**每帧调用：发布本机最新状态。数据链没启用（线程没起来）时是空操作 ——
/// 调用点先用 `is_active()` 挡掉。
pub fn publish_own(state: OwnState) {
    if let Some(ring) = OWN.get() {
        ring.publish(state);
    }
}

/// **datalink 线程**每拍调用：取本机状态槽的最新一版（`skipped` = 这拍之前被跳过的版本数）。
pub fn poll_own(cursor: &mut Cursor) -> Poll<'static, OwnState> {
    match OWN.get() {
        Some(ring) => ring.poll(cursor),
        None => Poll { value: None, skipped: 0, reset: false },
    }
}

/// **datalink 线程**调用：发布一份新的友军快照（全量覆盖）。
pub fn publish_tracks(list: &[StatePacket]) {
    if let Some(ring) = TRACKS.get() {
        ring.publish(Snap::from_slice(list));
    }
}

/// 友军槽的重置代数：线程据此统计换局次数（`reset_tracks()` 每次 +1）。
pub fn tracks_generation() -> u64 {
    TRACKS.get().map_or(0, Ring::generation)
}

/// **HUD 绘制线程**调用：取最新友军快照。永远返回、不做任何等待
/// （没数据就是空切片 → 友军面板/地图标记自然不画）。
pub fn latest_tracks() -> &'static [StatePacket] {
    TRACKS.get().map_or(&[], |ring| ring.latest().as_slice())
}

/// 数据链线程是否已建立（调用点用它挡掉"数据链没开"时的每帧发布；
/// 与配置里的 `enabled` 无关：配置开了但建 socket 失败同样是 `false`）。
pub fn is_active() -> bool {
    TRACKS.get().is_some()
}

/// 断线重连：推进代数并清空友军列表（新一局不该留着上一局的友军标记）。
/// 线程没起来时是空操作。
pub fn reset_tracks() {
    if let Some(ring) = TRACKS.get() {
        ring.reset();
        ring.publish(Snap::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_state_is_pod_and_roundtrips_through_the_ring() {
        let ring = Ring::<OwnState>::new();
        let mut st = OwnState {
            ias: 512.0,
            tas: 590.0,
            altitude: 3200.0,
            mach: 0.78,
            heading: 271.0,
            vy: -12.5,
            x: 0.75,
            y: 0.25,
            poi_enabled: true,
            poi_x: 0.8,
            poi_y: 0.2,
            ..OwnState::default()
        };
        st.set_type("f_16c");
        ring.publish(st);
        assert_eq!(*ring.latest(), st, "整份快照可按值搬进环形槽");
        assert_eq!(&ring.latest().aircraft_type[..5], b"f_16c", "机型是定长字节，没有堆指针");
    }

    #[test]
    fn type_field_pads_and_truncates() {
        let mut st = OwnState::default();
        st.set_type("f_16c");
        assert_eq!(&st.aircraft_type[..5], b"f_16c");
        assert_eq!(st.aircraft_type[5..], [0u8; TYPE_LEN - 5], "零填充");
        assert_eq!(OwnState::default().aircraft_type, [0u8; TYPE_LEN], "缺省机型全零");

        st.set_type("0123456789abcdef-and-more");
        assert_eq!(&st.aircraft_type, b"0123456789abcdef", "超长截断到 TYPE_LEN");
    }

    #[test]
    fn snap_truncates_to_max_tracks() {
        let list: Vec<StatePacket> =
            (0..MAX_TRACKS as u64 + 3).map(StatePacket::new).collect();
        let snap = Snap::from_slice(&list);
        assert_eq!(snap.as_slice().len(), MAX_TRACKS, "快照定长，超出部分截断");
        assert_eq!(snap.as_slice()[0].client_id, 0);
        assert!(Snap::default().as_slice().is_empty(), "缺省快照 = 没有友军");
    }

    /// 进程级槽（唯一碰全局的用例：其余用例用本地 `Ring`，避免并行跑时互相干扰）。
    #[test]
    fn global_slots_publish_poll_and_reset() {
        init_slots();
        let mut cursor = Cursor::default();

        let mut st = OwnState { ias: 300.0, ..OwnState::default() };
        st.set_type("f_16c");
        publish_own(st);
        assert_eq!(poll_own(&mut cursor).value.expect("有新版本").ias, 300.0);

        publish_tracks(&[StatePacket::new(7)]);
        let tracks = latest_tracks();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].client_id, 7);
        assert!(is_active());

        let gen = tracks_generation();
        reset_tracks();
        assert!(latest_tracks().is_empty(), "重连后旧友军必须清空");
        assert_eq!(tracks_generation(), gen + 1, "重置推进代数");
    }
}
