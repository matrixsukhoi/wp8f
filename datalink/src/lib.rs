//! Link-8 数据链协议：无状态、抗丢包（包自包含 → 丢包 = 下周期再发，离场 = 超时释放）。
//! 客户端按 `hz` 上报自身状态，服务端收到后立即应答全量追踪对象（UDP 请求/响应）。
//!
//! 追踪池按 `client_id`（启用时刻 Unix ms）建；`session_id`（u8 对局号）分池，同局才互相共享；
//! 超过 `timeout`（默认 5s）没来包就释放。
//!
//! # StatePacket（小端、`#[repr(C)]`、线上 4 字节对齐，字段与 DisplayData 同名同型）
//!
//! 共 116 字节：
//!
//! | 偏移 | 类型 | 字段 | 说明 |
//! |------|------|------|------|
//! | 0  | u64 | `client_id`     | 客户端唯一 ID（启用时刻 Unix ms） |
//! | 8  | u32 | `magic`         | `DATALINK_MAGIC`，解密后校验 |
//! | 12 | u8  | `version`       | `DATALINK_VERSION` |
//! | 13 | u8  | `session_id`    | 对局编号（同对局才共享） |
//! | 14 | u8  | `flags`         | bit0 = 服务端回填的"发件人本人"标记 |
//! | 15 | u8  | `seq`           | 客户端包计数（丢包观测） |
//! | 16 | [u8; 16] | `aircraft_type` | 机型，定长零填充 |
//! | 32 | f64 | `ias`           | 表速 km/h（= DisplayData.ias） |
//! | 40 | f64 | `tas`           | 真速 km/h（= DisplayData.tas） |
//! | 48 | f64 | `altitude`      | 高度 m（= DisplayData.altitude） |
//! | 56 | f64 | `mach`          | 马赫数（= DisplayData.mach） |
//! | 64 | f64 | `heading`       | 水平航向 deg（= DisplayData.heading） |
//! | 72 | f64 | `vy`            | 垂直速度 m/s（= DisplayData.vy） |
//! | 80 | f64 | `x`             | 归一化地图坐标（= MapDisplay.player_map_x） |
//! | 88 | f64 | `y`             | 归一化地图坐标（= MapDisplay.player_map_y） |
//! | 96 | f64 | `poi_x`         | 本机 POI 归一化地图坐标 |
//! | 104 | f64 | `poi_y`        | 本机 POI 归一化地图坐标 |
//! | 112 | u8  | `poi_enabled`  | 是否启用 POI 同步（0/1） |
//! | 113–115 | — | 保留（3 字节，恒为 0） | |
//!
//! `Reply` = 8 字节头（`magic, version, session_id, count, server_seq`）+ N×`StatePacket`。
//!
//! # 混淆
//! `xor_key = sum(密钥字节) as u32`，包内每个 4 字节字 `^= xor_key`（尾字节用密钥低位字节）；
//! 解密后魔数不符的包静默丢弃。只防串台，不是密码学加密。
//!
//! # 线程
//! `thread::spawn_link()` 起独立线程收发（主循环不碰 socket）：按 `send_hz` 从 `link_ring` 取本机
//! 状态上报、排空应答、把最新全量快照 publish 回 `link_ring`；HUD 用
//! `link_ring::latest_tracks()` 消费。core 主循环只每帧 `publish_own(...)` 一次（无 I/O）。

pub mod client;
pub mod link_ring;
pub mod server;
pub mod thread;
pub mod track;

use std::time::{SystemTime, UNIX_EPOCH};

pub const DATALINK_MAGIC: u32 = 0x4C38_4B38; // "L8K8"
pub const DATALINK_VERSION: u8 = 1;
pub const DEFAULT_PORT: u16 = 2887;
pub const DEFAULT_HZ: f64 = 4.0;
pub const DEFAULT_TIMEOUT_SECS: u64 = 5;
pub const DEFAULT_SESSION_ID: u8 = 1;
/// 客户端友军快照的默认超时（秒）：这么久没收到服务端应答就清空友军标记
/// （与服务端自己的追踪超时 `DEFAULT_TIMEOUT_SECS` 同口径；配置写 `0` = 关闭清空）。
pub const PEER_TIMEOUT_SECS: u32 = 5;
/// 单个应答里最多携带的追踪对象数（8 + 116×12 = 1400 字节，留在 UDP 安全负载内）。
pub const MAX_TRACKS: usize = 12;
pub const TYPE_LEN: usize = 16;
pub const STATE_LEN: usize = 116;
pub const REPLY_HEADER_LEN: usize = 8;

/// `flags` 位：该条目是应答接收者本人（服务端回填）。
pub const FLAG_SENDER: u8 = 0x01;

/// 一条飞行状态。字段与 `wp8f_disp::DisplayData` / `MapDisplay` 同名同型，
/// 复制进环形缓冲区时无需任何换算。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StatePacket {
    pub client_id: u64,
    pub magic: u32,
    pub version: u8,
    pub session_id: u8,
    pub flags: u8,
    pub seq: u8,
    pub aircraft_type: [u8; TYPE_LEN],
    pub ias: f64,
    pub tas: f64,
    pub altitude: f64,
    pub mach: f64,
    pub heading: f64,
    pub vy: f64,
    pub x: f64,
    pub y: f64,
    /// 本机 POI 归一化地图坐标（`poi_enabled` 为 0 时忽略）
    pub poi_x: f64,
    pub poi_y: f64,
    /// 是否启用 POI 同步（0 = 无 POI/不共享，1 = 启用）
    pub poi_enabled: u8,
}

/// 写入定长机型字段：零填充、超长截断（`StatePacket` 与 `link_ring::OwnState` 同一表示）。
pub fn write_type(dst: &mut [u8; TYPE_LEN], name: &str) {
    *dst = [0; TYPE_LEN];
    let bytes = name.as_bytes();
    let n = bytes.len().min(TYPE_LEN);
    dst[..n].copy_from_slice(&bytes[..n]);
}

impl StatePacket {
    /// 新建一个带魔数/版本/ID 的空状态包。
    pub fn new(client_id: u64) -> Self {
        Self {
            client_id,
            magic: DATALINK_MAGIC,
            version: DATALINK_VERSION,
            ..Self::default()
        }
    }

    /// 写入机型（定长零填充，超长截断）。
    pub fn set_type(&mut self, s: &str) {
        write_type(&mut self.aircraft_type, s);
    }

    /// 读取机型（去掉尾部零填充）。
    pub fn type_str(&self) -> &str {
        let end = self
            .aircraft_type
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(TYPE_LEN);
        std::str::from_utf8(&self.aircraft_type[..end]).unwrap_or("")
    }

    /// 魔数 + 版本校验（解密后调用）。
    pub fn validate(&self) -> bool {
        self.magic == DATALINK_MAGIC && self.version == DATALINK_VERSION
    }

    /// 序列化为定长字节（小端，字段布局见模块文档）。
    pub fn to_bytes(&self) -> [u8; STATE_LEN] {
        let mut b = [0u8; STATE_LEN];
        b[0..8].copy_from_slice(&self.client_id.to_le_bytes());
        b[8..12].copy_from_slice(&self.magic.to_le_bytes());
        b[12] = self.version;
        b[13] = self.session_id;
        b[14] = self.flags;
        b[15] = self.seq;
        b[16..32].copy_from_slice(&self.aircraft_type);
        b[32..40].copy_from_slice(&self.ias.to_le_bytes());
        b[40..48].copy_from_slice(&self.tas.to_le_bytes());
        b[48..56].copy_from_slice(&self.altitude.to_le_bytes());
        b[56..64].copy_from_slice(&self.mach.to_le_bytes());
        b[64..72].copy_from_slice(&self.heading.to_le_bytes());
        b[72..80].copy_from_slice(&self.vy.to_le_bytes());
        b[80..88].copy_from_slice(&self.x.to_le_bytes());
        b[88..96].copy_from_slice(&self.y.to_le_bytes());
        b[96..104].copy_from_slice(&self.poi_x.to_le_bytes());
        b[104..112].copy_from_slice(&self.poi_y.to_le_bytes());
        b[112] = self.poi_enabled;
        b
    }

    /// 反序列化并校验魔数/版本；不合法返回 `None`（单条坏数据不影响整包）。
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < STATE_LEN {
            return None;
        }
        let f64_at = |o: usize| -> Option<f64> {
            Some(f64::from_le_bytes(b[o..o + 8].try_into().ok()?))
        };
        let p = Self {
            client_id: u64::from_le_bytes(b[0..8].try_into().ok()?),
            magic: u32::from_le_bytes(b[8..12].try_into().ok()?),
            version: b[12],
            session_id: b[13],
            flags: b[14],
            seq: b[15],
            aircraft_type: b[16..32].try_into().ok()?,
            ias: f64_at(32)?,
            tas: f64_at(40)?,
            altitude: f64_at(48)?,
            mach: f64_at(56)?,
            heading: f64_at(64)?,
            vy: f64_at(72)?,
            x: f64_at(80)?,
            y: f64_at(88)?,
            poi_x: f64_at(96)?,
            poi_y: f64_at(104)?,
            poi_enabled: b[112],
        };
        p.validate().then_some(p)
    }
}

/// 服务端应答：请求者所在对局的全量追踪对象。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reply {
    pub session_id: u8,
    pub server_seq: u8,
    pub tracks: Vec<StatePacket>,
}

impl Reply {
    pub fn to_bytes(&self) -> Vec<u8> {
        let count = self.tracks.len().min(MAX_TRACKS);
        let mut b = Vec::with_capacity(REPLY_HEADER_LEN + count * STATE_LEN);
        b.extend_from_slice(&DATALINK_MAGIC.to_le_bytes());
        b.push(DATALINK_VERSION);
        b.push(self.session_id);
        b.push(count as u8);
        b.push(self.server_seq);
        for t in &self.tracks[..count] {
            b.extend_from_slice(&t.to_bytes());
        }
        b
    }

    /// 反序列化并校验包头魔数；逐条校验，坏条目跳过。
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < REPLY_HEADER_LEN {
            return None;
        }
        if u32::from_le_bytes(b[0..4].try_into().ok()?) != DATALINK_MAGIC {
            return None;
        }
        if b[4] != DATALINK_VERSION {
            return None;
        }
        let session_id = b[5];
        let count = b[6] as usize;
        let server_seq = b[7];
        let mut tracks = Vec::with_capacity(count.min(MAX_TRACKS));
        for i in 0..count.min(MAX_TRACKS) {
            let off = REPLY_HEADER_LEN + i * STATE_LEN;
            if let Some(p) = StatePacket::from_bytes(b.get(off..off + STATE_LEN)?) {
                tracks.push(p);
            }
        }
        Some(Self { session_id, server_seq, tracks })
    }
}

/// 密钥 → 异或值：密钥字节求和（u32 环绕）。
pub fn xor_key(key: &str) -> u32 {
    key.bytes().fold(0u32, |acc, b| acc.wrapping_add(b as u32))
}

/// 就地逐 4 字节异或（加解密同一函数）；非 4 字节尾巴用密钥低位字节。
pub fn xor_crypt(buf: &mut [u8], key: u32) {
    let n = buf.len();
    let words = n / 4;
    for i in 0..words {
        let o = i * 4;
        let w = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) ^ key;
        buf[o..o + 4].copy_from_slice(&w.to_le_bytes());
    }
    let kb = key.to_le_bytes();
    for (i, b) in buf[words * 4..].iter_mut().enumerate() {
        *b ^= kb[i];
    }
}

/// 生成客户端唯一 ID：启用时刻的 Unix 毫秒时间戳。
pub fn generate_client_id() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(1)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StatePacket {
        let mut p = StatePacket::new(1_700_000_000_000);
        p.session_id = 7;
        p.set_type("f_16c");
        p.seq = 42;
        p.ias = 523.4;
        p.tas = 601.2;
        p.altitude = 3129.0;
        p.mach = 0.87;
        p.heading = 271.5;
        p.vy = -3.2;
        p.x = 0.4123;
        p.y = 0.8811;
        p.poi_x = 0.6001;
        p.poi_y = 0.3999;
        p.poi_enabled = 1;
        p
    }

    #[test]
    fn state_roundtrip() {
        let p = sample();
        let bytes = p.to_bytes();
        assert_eq!(bytes.len(), STATE_LEN);
        assert_eq!(STATE_LEN % 4, 0, "包长必须 4 字节对齐");
        assert_eq!(StatePacket::from_bytes(&bytes), Some(p));
        assert_eq!(p.type_str(), "f_16c");
        assert_eq!(p.session_id, 7);
    }

    #[test]
    fn xor_roundtrip_and_wrong_key_rejected() {
        let p = sample();
        let mut wire = p.to_bytes();
        let key = xor_key("datalink-secret");
        xor_crypt(&mut wire, key);
        assert_ne!(&wire[8..12], &DATALINK_MAGIC.to_le_bytes(), "线上包魔数应被混淆");

        // 解密还原
        let mut good = wire;
        xor_crypt(&mut good, key);
        assert_eq!(StatePacket::from_bytes(&good), Some(p));

        // 错误密钥解出来的魔数校验必失败
        let mut bad = wire;
        xor_crypt(&mut bad, xor_key("other-key"));
        assert_eq!(StatePacket::from_bytes(&bad), None);
    }

    #[test]
    fn garbage_and_short_input_rejected() {
        assert_eq!(StatePacket::from_bytes(&[0u8; 3]), None);
        assert_eq!(StatePacket::from_bytes(&[0u8; STATE_LEN]), None);
        let mut b = sample().to_bytes();
        b[8] ^= 0xFF; // 破坏魔数
        assert_eq!(StatePacket::from_bytes(&b), None);
    }

    #[test]
    fn reply_roundtrip_skips_bad_entries() {
        let mut reply = Reply { session_id: 7, server_seq: 9, tracks: vec![sample(), sample()] };
        reply.tracks[1].client_id = 2;
        let bytes = reply.to_bytes();
        assert_eq!(bytes.len(), REPLY_HEADER_LEN + 2 * STATE_LEN);

        let mut wire = bytes.clone();
        xor_crypt(&mut wire, xor_key("k"));
        xor_crypt(&mut wire, xor_key("k"));
        let mut parsed = Reply::from_bytes(&wire).unwrap();
        assert_eq!(parsed, reply);
        assert_eq!(parsed.session_id, 7);

        // 中间插入坏条目 → 该条被跳过，其余保留
        let mut dirty = bytes.clone();
        dirty[REPLY_HEADER_LEN + STATE_LEN + 8] ^= 0xFF;
        parsed = Reply::from_bytes(&dirty).unwrap();
        assert_eq!(parsed.tracks.len(), 1);
        assert_eq!(parsed.server_seq, 9);
    }

    // ---- 字节布局 / 截断 / 版本 ----

    #[test]
    fn state_byte_layout_offsets() {
        let mut p = sample();
        p.flags = 0x05;
        p.poi_enabled = 1;
        let b = p.to_bytes();

        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(&b[0..8], &1_700_000_000_000u64.to_le_bytes(), "client_id @0");
        assert_eq!(&b[8..12], &DATALINK_MAGIC.to_le_bytes(), "magic @8");
        assert_eq!(b[12], DATALINK_VERSION, "version @12");
        assert_eq!(b[13], 7, "session_id @13");
        assert_eq!(b[14], 0x05, "flags @14");
        assert_eq!(b[15], 42, "seq @15");
        assert_eq!(&b[16..32], &p.aircraft_type[..], "机型 @16");
        let f64_at = |o: usize| f64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        assert_eq!(f64_at(32), p.ias, "ias @32");
        assert_eq!(f64_at(40), p.tas, "tas @40");
        assert_eq!(f64_at(48), p.altitude, "altitude @48");
        assert_eq!(f64_at(56), p.mach, "mach @56");
        assert_eq!(f64_at(64), p.heading, "heading @64");
        assert_eq!(f64_at(72), p.vy, "vy @72");
        assert_eq!(f64_at(80), p.x, "x @80");
        assert_eq!(f64_at(88), p.y, "y @88");
        assert_eq!(f64_at(96), p.poi_x, "poi_x @96");
        assert_eq!(f64_at(104), p.poi_y, "poi_y @104");
        assert_eq!(b[112], 1, "poi_enabled @112");
        assert_eq!(&b[113..116], &[0u8, 0, 0], "保留字节 113..116 必须恒为 0");
    }

    #[test]
    fn state_truncation_rejected_but_trailing_tolerated() {
        let bytes = sample().to_bytes();
        for len in 0..STATE_LEN {
            assert_eq!(
                StatePacket::from_bytes(&bytes[..len]),
                None,
                "截断包（len={len}）应被拒绝"
            );
        }
        assert_eq!(StatePacket::from_bytes(&bytes), Some(sample()));
        // 超长输入只读取前 116 字节（容忍尾部多余数据）
        let mut long = bytes.to_vec();
        long.extend_from_slice(&[0xAA; 50]);
        assert_eq!(
            StatePacket::from_bytes(&long),
            Some(sample()),
            "尾部多余字节应被忽略"
        );
    }

    #[test]
    fn state_rejects_bad_version() {
        let mut b = sample().to_bytes();
        for bad in [0u8, 2, 255] {
            b[12] = bad;
            assert_eq!(StatePacket::from_bytes(&b), None, "版本 {bad} 应被拒绝");
        }
        b[12] = DATALINK_VERSION;
        assert!(StatePacket::from_bytes(&b).is_some(), "恢复合法版本应通过");
    }

    // ---- 机型字段 ----

    #[test]
    fn type_field_padding_and_truncation() {
        let mut p = StatePacket::new(1);

        // 不足 16 字节 → 尾部零填充
        p.set_type("f_16c");
        let b = p.to_bytes();
        assert_eq!(&b[16..21], b"f_16c");
        assert!(b[21..32].iter().all(|&x| x == 0), "不足 16 字节应零填充");
        assert_eq!(p.type_str(), "f_16c");

        // 恰好 16 字节 → 占满、type_str 原样返回
        p.set_type("0123456789abcdef");
        assert_eq!(p.aircraft_type, *b"0123456789abcdef");
        assert_eq!(p.type_str(), "0123456789abcdef");

        // 超过 16 字节 → 按字节截断
        p.set_type("0123456789abcdefZZZ");
        assert_eq!(p.aircraft_type, *b"0123456789abcdef", "超长机型名应截断到 16 字节");
        assert_eq!(p.type_str(), "0123456789abcdef");

        // 空名 → 全零
        p.set_type("");
        assert_eq!(p.aircraft_type, [0u8; TYPE_LEN]);
        assert_eq!(p.type_str(), "");

        // 多字节 UTF-8 被字节级截断成半个字符 → type_str 容错返回空串（当前行为）
        let mut q = StatePacket::new(2);
        q.set_type("战斗机战斗机战斗机x"); // 28 字节 > 16
        assert_eq!(q.type_str(), "");
    }

    // ---- XOR 混淆 ----

    #[test]
    fn xor_key_is_sum_of_key_bytes() {
        assert_eq!(xor_key(""), 0, "空密钥异或值为 0");
        assert_eq!(xor_key("a"), 97);
        assert_eq!(xor_key("ab"), 97 + 98);
        assert_eq!(xor_key("中"), 0xE4 + 0xB8 + 0xAD, "多字节 UTF-8 按字节求和");
    }

    #[test]
    fn xor_crypt_roundtrip_with_unaligned_tail() {
        let key = xor_key("tail-key");
        assert_ne!(key, 0);
        for len in 0..=17usize {
            let plain: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            let mut wire = plain.clone();
            xor_crypt(&mut wire, key);
            if len >= 4 {
                assert_ne!(&wire[..4], &plain[..4], "len={len}：整字应被异或");
            }
            xor_crypt(&mut wire, key);
            assert_eq!(wire, plain, "len={len}：异或应可往返");
        }
    }

    #[test]
    fn xor_wire_leaks_no_plaintext() {
        let p = sample();
        let plain = p.to_bytes();
        let key = xor_key("datalink-secret");
        assert_ne!(key, 0);
        let mut wire = plain;
        xor_crypt(&mut wire, key);

        let magic = DATALINK_MAGIC.to_le_bytes();
        assert!(
            wire.windows(4).all(|w| w != &magic),
            "线上任意位置不应出现明文魔数"
        );
        assert!(wire.windows(5).all(|w| w != b"f_16c"), "线上不应出现明文机型");
        assert_ne!(&wire[16..32], &plain[16..32], "机型区必须被打乱");
        for i in 0..STATE_LEN / 4 {
            let o = i * 4;
            assert_ne!(&wire[o..o + 4], &plain[o..o + 4], "第 {i} 个字必须整体异或");
        }
    }

    #[test]
    fn empty_key_sends_plaintext() {
        // 空密钥 → xor_key = 0 → 不做任何混淆（配置空密钥等于明文传输）——行为固化
        let p = sample();
        let plain = p.to_bytes();
        let mut wire = plain;
        xor_crypt(&mut wire, xor_key(""));
        assert_eq!(wire, plain, "空密钥不做混淆");
        assert_eq!(&wire[8..12], &DATALINK_MAGIC.to_le_bytes(), "空密钥时魔数明文可见");
        assert_eq!(StatePacket::from_bytes(&wire), Some(p));
    }

    // ---- Reply 边界 ----

    #[test]
    fn reply_count_zero_roundtrip() {
        let reply = Reply { session_id: 3, server_seq: 17, tracks: vec![] };
        let bytes = reply.to_bytes();
        assert_eq!(bytes.len(), REPLY_HEADER_LEN, "count=0 只有包头");
        assert_eq!(bytes[5], 3, "session_id @5");
        assert_eq!(bytes[6], 0, "count @6");
        assert_eq!(bytes[7], 17, "server_seq @7");
        assert_eq!(Reply::from_bytes(&bytes), Some(reply));
    }

    #[test]
    fn reply_count_one_and_max_roundtrip() {
        for n in [1usize, MAX_TRACKS] {
            let tracks: Vec<StatePacket> = (0..n)
                .map(|i| {
                    let mut p = sample();
                    p.client_id = (i + 1) as u64;
                    p
                })
                .collect();
            let reply = Reply { session_id: 1, server_seq: n as u8, tracks };
            let bytes = reply.to_bytes();
            assert_eq!(bytes.len(), REPLY_HEADER_LEN + n * STATE_LEN, "n={n} 包长");
            assert_eq!(bytes[6] as usize, n, "n={n} count 字节");
            assert_eq!(Reply::from_bytes(&bytes), Some(reply), "n={n} 应往返");
        }
    }

    #[test]
    fn reply_count_beyond_max_capped() {
        let tracks: Vec<StatePacket> = (0..MAX_TRACKS + 5)
            .map(|i| {
                let mut p = sample();
                p.client_id = i as u64;
                p
            })
            .collect();
        let reply = Reply { session_id: 1, server_seq: 0, tracks };
        let bytes = reply.to_bytes();
        assert_eq!(
            bytes.len(),
            REPLY_HEADER_LEN + MAX_TRACKS * STATE_LEN,
            "序列化应截断到 MAX_TRACKS"
        );
        assert_eq!(bytes[6] as usize, MAX_TRACKS);
        let parsed = Reply::from_bytes(&bytes).expect("应可解析");
        assert_eq!(parsed.tracks.len(), MAX_TRACKS);

        // 伪造超界 count（255）→ 解析同样截断到 MAX_TRACKS，不越界
        let mut forged = reply.to_bytes();
        forged[6] = 255;
        let parsed = Reply::from_bytes(&forged).expect("超界 count 应被截断");
        assert_eq!(parsed.tracks.len(), MAX_TRACKS);
        assert_eq!(parsed.server_seq, 0);
    }

    #[test]
    fn reply_rejects_bad_header_and_short_payload() {
        let bytes =
            Reply { session_id: 1, server_seq: 1, tracks: vec![sample(), sample()] }.to_bytes();

        // 包头不足 8 字节
        assert_eq!(Reply::from_bytes(&[]), None);
        assert_eq!(Reply::from_bytes(&bytes[..REPLY_HEADER_LEN - 1]), None);

        // 坏魔数 / 坏版本
        let mut bad = bytes.clone();
        bad[0] ^= 0xFF;
        assert_eq!(Reply::from_bytes(&bad), None, "坏魔数应被拒绝");
        let mut bad = bytes.clone();
        bad[4] = DATALINK_VERSION + 1;
        assert_eq!(Reply::from_bytes(&bad), None, "坏版本应被拒绝");

        // count 声明 2 条但只带 1 条 → 整包拒绝
        assert_eq!(Reply::from_bytes(&bytes[..REPLY_HEADER_LEN + STATE_LEN]), None);

        // 头部 count=1 但没有载荷 → 拒绝
        let mut hdr = bytes[..REPLY_HEADER_LEN].to_vec();
        hdr[6] = 1;
        assert_eq!(Reply::from_bytes(&hdr), None);

        // count=0 → 无视尾部垃圾，解析为空应答
        let mut hdr0 = bytes[..REPLY_HEADER_LEN].to_vec();
        hdr0[6] = 0;
        hdr0.extend_from_slice(&[0xAA; 10]);
        assert_eq!(Reply::from_bytes(&hdr0).map(|r| r.tracks.len()), Some(0));
    }

    // ---- 边界值 ----

    #[test]
    fn state_all_zero_fields_roundtrip() {
        // 新建包（全零字段）应可往返
        let p = StatePacket::new(0);
        assert_eq!(StatePacket::from_bytes(&p.to_bytes()), Some(p));
        assert_eq!(p.type_str(), "");
        // Default（magic/version 均为 0）的线上字节必须被拒
        let d = StatePacket::default();
        assert!(!d.validate());
        assert_eq!(StatePacket::from_bytes(&d.to_bytes()), None);
    }

    #[test]
    fn state_extreme_values_roundtrip() {
        let mut p = StatePacket::new(u64::MAX);
        p.session_id = u8::MAX;
        p.flags = u8::MAX;
        p.seq = u8::MAX;
        p.set_type("0123456789abcdef");
        p.ias = f64::MAX;
        p.tas = f64::MIN;
        p.altitude = -1e308;
        p.mach = f64::MIN_POSITIVE;
        p.heading = 0.0;
        p.vy = -0.0;
        p.x = 1e308;
        p.y = -0.1 - 0.2;
        p.poi_x = f64::from_bits(u64::MAX); // NaN
        p.poi_y = 0.1 + 0.2;
        p.poi_enabled = 1;

        let q = StatePacket::from_bytes(&p.to_bytes()).expect("极端值应可解析");
        assert_eq!(q.client_id, u64::MAX);
        assert_eq!(q.session_id, 255);
        assert_eq!(q.seq, 255, "seq=255（回绕前上界）应原样往返");
        assert_eq!(q.type_str(), "0123456789abcdef");
        assert_eq!(q.ias, f64::MAX);
        assert_eq!(q.tas, f64::MIN);
        assert_eq!(q.altitude, -1e308);
        assert_eq!(q.mach, f64::MIN_POSITIVE);
        assert_eq!(q.vy.to_bits(), (-0.0f64).to_bits(), "-0.0 应按位保留");
        assert_eq!(q.x, 1e308);
        assert_eq!(q.y, p.y);
        assert!(q.poi_x.is_nan(), "NaN 应按位往返");
        assert_eq!(q.poi_x.to_bits(), p.poi_x.to_bits());
        assert_eq!(q.poi_y, p.poi_y, "0.1+0.2 应精确往返");
        assert_eq!(q.poi_enabled, 1);
    }

    #[test]
    fn generate_client_id_is_positive() {
        assert!(generate_client_id() >= 1, "client_id 必须非零");
        assert!(generate_client_id() >= 1);
    }
}
