//! 服务端追踪池：`client_id` → 追踪对象。
//!
//! 无状态协议的"状态"只在这里：同 ID 更新、新 ID 建档、超时自动释放。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::StatePacket;

#[derive(Debug, Clone, Copy)]
pub struct Track {
    pub entry: StatePacket,
    pub last_seen: Instant,
}

#[derive(Debug)]
pub struct TrackPool {
    tracks: HashMap<u64, Track>,
    timeout: Duration,
}

impl TrackPool {
    pub fn new(timeout: Duration) -> Self {
        Self {
            tracks: HashMap::new(),
            timeout,
        }
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// 更新（或新建）追踪对象，返回是否为新建。
    pub fn upsert(&mut self, entry: StatePacket, now: Instant) -> bool {
        let is_new = !self.tracks.contains_key(&entry.client_id);
        self.tracks.insert(
            entry.client_id,
            Track { entry, last_seen: now },
        );
        is_new
    }

    /// 释放超过 `timeout` 没来包的追踪对象，返回释放数量。
    pub fn prune(&mut self, now: Instant) -> usize {
        let timeout = self.timeout;
        let before = self.tracks.len();
        self.tracks.retain(|_, t| now.duration_since(t.last_seen) < timeout);
        before - self.tracks.len()
    }

    /// 全量快照（按 `client_id` 排序，输出稳定），并为 `requester` 的条目回填
    /// `FLAG_SENDER`，让客户端零计算地跳过自己。最多 `max` 条。
    pub fn snapshot(&self, requester: u64, max: usize) -> Vec<StatePacket> {
        let mut out: Vec<StatePacket> = self.tracks.values().map(|t| t.entry).collect();
        out.sort_by_key(|e| e.client_id);
        out.truncate(max);
        for e in &mut out {
            if e.client_id == requester {
                e.flags |= crate::FLAG_SENDER;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StatePacket;
    use crate::{FLAG_SENDER, MAX_TRACKS};

    fn entry(id: u64) -> StatePacket {
        let mut p = StatePacket::new(id);
        p.ias = 500.0;
        p
    }

    #[test]
    fn upsert_creates_then_updates() {
        let mut pool = TrackPool::new(Duration::from_secs(5));
        let t0 = Instant::now();
        assert!(pool.upsert(entry(1), t0), "首次出现应新建");
        assert!(!pool.upsert(entry(1), t0), "同 ID 应更新");
        assert!(pool.upsert(entry(2), t0), "新 ID 应新建");
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn prune_releases_stale_clients() {
        let mut pool = TrackPool::new(Duration::from_secs(5));
        let t0 = Instant::now();
        pool.upsert(entry(1), t0);
        pool.upsert(entry(2), t0 + Duration::from_secs(4));

        // t0+6s：ID 1 超时（6s > 5s），ID 2 仍在（2s < 5s）
        assert_eq!(pool.prune(t0 + Duration::from_secs(6)), 1);
        assert_eq!(pool.len(), 1);
        let snap = pool.snapshot(2, 16);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].client_id, 2);
    }

    #[test]
    fn snapshot_marks_sender_and_caps() {
        let mut pool = TrackPool::new(Duration::from_secs(60));
        let t0 = Instant::now();
        for id in 1..=4 {
            pool.upsert(entry(id), t0);
        }
        let snap = pool.snapshot(2, 2);
        assert_eq!(snap.len(), 2, "应按 max 截断");
        assert!(snap.iter().all(|e| e.client_id >= 1 && e.client_id <= 2), "应按 client_id 排序");
        let snap = pool.snapshot(3, 16);
        assert_eq!(snap.len(), 4);
        assert_eq!(snap.iter().filter(|e| e.flags & crate::FLAG_SENDER != 0).count(), 1);
        assert_eq!(snap[2].client_id, 3);
        assert_eq!(snap[2].flags & crate::FLAG_SENDER, crate::FLAG_SENDER);
    }

    #[test]
    fn upsert_same_client_id_replaces_old_state() {
        let mut pool = TrackPool::new(Duration::from_secs(60));
        let t0 = Instant::now();

        let mut a = entry(7);
        a.set_type("f_16c");
        a.seq = 1;
        a.ias = 500.0;
        assert!(pool.upsert(a, t0));

        let mut b = entry(7);
        b.set_type("f_15");
        b.seq = 2;
        b.ias = 700.0;
        assert!(!pool.upsert(b, t0 + Duration::from_millis(250)), "同 ID 应视为更新");
        assert_eq!(pool.len(), 1, "同 client_id 不应重复建档");

        let snap = pool.snapshot(999, MAX_TRACKS);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].ias, 700.0, "新状态应覆盖旧状态");
        assert_eq!(snap[0].type_str(), "f_15");
        assert_eq!(snap[0].seq, 2);

        // 超时按最后一次来包时间计：t0+59s 时距上次更新 58.75s < 60s 仍存活
        assert_eq!(pool.prune(t0 + Duration::from_secs(59)), 0);
        assert_eq!(pool.prune(t0 + Duration::from_secs(61)), 1);
    }

    #[test]
    fn snapshot_sorted_by_client_id() {
        let mut pool = TrackPool::new(Duration::from_secs(60));
        let t0 = Instant::now();
        for id in [30u64, 10, 40, 20] {
            let mut p = entry(id);
            p.seq = id as u8;
            pool.upsert(p, t0);
        }
        let snap = pool.snapshot(999, MAX_TRACKS);
        let ids: Vec<u64> = snap.iter().map(|e| e.client_id).collect();
        assert_eq!(ids, vec![10, 20, 30, 40], "快照必须按 client_id 升序");
    }

    #[test]
    fn snapshot_sender_mark_is_ephemeral() {
        let mut pool = TrackPool::new(Duration::from_secs(60));
        let t0 = Instant::now();
        pool.upsert(entry(1), t0);
        pool.upsert(entry(2), t0);

        let first = pool.snapshot(1, MAX_TRACKS);
        assert_eq!(first[0].flags & FLAG_SENDER, FLAG_SENDER, "自己应带 FLAG_SENDER");

        // 标记只作用于本次快照的副本，不污染池内数据
        let second = pool.snapshot(2, MAX_TRACKS);
        assert_eq!(second[0].flags & FLAG_SENDER, 0, "旧标记不应被带到下一次快照");
        assert_eq!(second[1].flags & FLAG_SENDER, FLAG_SENDER);
    }

    #[test]
    fn prune_boundary_at_exact_timeout() {
        let mut pool = TrackPool::new(Duration::from_secs(5));
        let t0 = Instant::now();
        pool.upsert(entry(1), t0);
        // 恰好等于 timeout（duration == timeout 不满足 < timeout）→ 过期
        assert_eq!(pool.prune(t0 + Duration::from_secs(5)), 1);

        pool.upsert(entry(2), t0);
        // 差 1ns 未到点 → 保留
        assert_eq!(pool.prune(t0 + Duration::from_secs(5) - Duration::from_nanos(1)), 0);
        assert_eq!(pool.len(), 1);
        // 空池再 prune 返回 0
        pool.upsert(entry(3), t0);
        pool.prune(t0 + Duration::from_secs(10));
        assert_eq!(pool.prune(t0 + Duration::from_secs(11)), 0);
    }

    #[test]
    fn snapshot_caps_at_max_tracks_and_zero() {
        let mut pool = TrackPool::new(Duration::from_secs(60));
        let t0 = Instant::now();
        for id in 1..=20u64 {
            pool.upsert(entry(id), t0);
        }

        let snap = pool.snapshot(999, MAX_TRACKS);
        assert_eq!(snap.len(), MAX_TRACKS, "应截断到应答上限 MAX_TRACKS");
        let ids: Vec<u64> = snap.iter().map(|e| e.client_id).collect();
        assert_eq!(
            ids,
            (1..=MAX_TRACKS as u64).collect::<Vec<_>>(),
            "截断保留 client_id 最小的条目"
        );
        assert!(snap.iter().all(|e| e.flags & FLAG_SENDER == 0), "requester 不在窗口内则不带标记");

        // requester 自身 id 较大、被截掉 → 快照里没有自己
        let snap = pool.snapshot(15, MAX_TRACKS);
        assert!(snap.iter().all(|e| e.client_id != 15), "被截掉的 requester 不出现在快照中");

        // max = 0 → 空
        assert!(pool.snapshot(1, 0).is_empty());
    }
}
