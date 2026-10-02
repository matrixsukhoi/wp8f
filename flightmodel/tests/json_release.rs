//! 交付物 4 的**可测证据**：JSON 解析的中间对象（`serde_json::Value`）在函数返回前就被释放，
//! 常驻内存只剩扁平后的 `HashMap`。
//!
//! 做法：本测试 crate 装一个**计数分配器**，量三个数：
//! * `before` —— 调用前的存活字节；
//! * `peak`   —— 调用期间的峰值（含 JSON 中间对象）；
//! * `after`  —— 调用返回后的存活字节（应当只剩扁平表）。
//!
//! 断言：
//! 1. `after - before` 不超过扁平表自身估算大小的 2 倍（说明 `Value` 已经不在了）；
//! 2. `peak - before` 明显大于 `after - before`（说明中间对象确实存在过，测量有效）；
//! 3. 把返回的表 `drop` 掉后存活字节回落到调用前的水平（没有别的泄漏）。
//!
//! 真实数据的**峰值 RSS 对比**（legacy 数据根 vs JSON 数据根，同一机型）另外用
//! `/usr/bin/time -v ./flightmodel … --format json` 取 `Maximum resident set size`，见阶段报告。

#![cfg(feature = "fm-json")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAlloc;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

#[inline]
fn on_alloc(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            on_alloc(l.size());
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        System.dealloc(p, l)
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        let np = System.realloc(p, l, new_size);
        if !np.is_null() {
            if new_size >= l.size() {
                on_alloc(new_size - l.size());
            } else {
                LIVE.fetch_sub(l.size() - new_size, Ordering::Relaxed);
            }
        }
        np
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// 造一份"结构像 FM 文件"的 JSON：对象套对象、数值/字符串/数组混合，键数可控。
fn synthetic_fm_json(blocks: usize) -> String {
    let mut s = String::with_capacity(blocks * 160 + 64);
    s.push('{');
    for i in 0..blocks {
        if i > 0 {
            s.push(',');
        }
        // 每块 6 个叶子键（对齐真实机型文件的"块 + 少量标量 + 一组多值"形态）
        s.push_str(&format!(
            r#""Block{i}": {{"Empty": {i}.5, "Crew": {i}, "Name": "block_{i}", "Speed": [{i}, {i}.25], "Flag": true, "Wing": {{"Span": {i}.125}}}}"#
        ));
    }
    s.push('}');
    s
}

#[test]
fn json_value_is_released_before_return() {
    let json = synthetic_fm_json(4_000);
    eprintln!("JSON 文本 {} 字节", json.len());

    // 预热一次：让 HashMap/分配器的首次增长、thread_local 等一次性分配都发生掉
    let warm = wp8f_flightmodel::parse_json_fm(&json).expect("预热解析");
    let warm_keys = warm.len();
    drop(warm);

    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);

    let map = wp8f_flightmodel::parse_json_fm(&json).expect("解析");

    let after = LIVE.load(Ordering::SeqCst);
    let peak = PEAK.load(Ordering::SeqCst);

    // 扁平表自身的大致占用：键 + 值 + 每个 entry 的固定开销（保守取 48 字节）
    let flat_bytes: usize = map
        .iter()
        .map(|(k, v)| k.len() + v.as_string().len() + 48)
        .sum();

    let retained = after.saturating_sub(before);
    let peak_growth = peak.saturating_sub(before);
    eprintln!(
        "键数={} before={} peak={} after={} | 常驻增长={} 峰值增长={} 扁平表估算={}",
        map.len(),
        before,
        peak,
        after,
        retained,
        peak_growth,
        flat_bytes
    );

    assert_eq!(map.len(), warm_keys, "两次解析键数应当一致");
    assert!(
        retained <= flat_bytes * 2,
        "返回后常驻内存 {retained} 远超扁平表估算 {flat_bytes} —— 中间 JSON 对象没被释放？"
    );
    assert!(
        peak_growth > flat_bytes,
        "峰值增长 {peak_growth} 不大于扁平表估算 {flat_bytes} —— 中间对象没被测量到，测试无效"
    );
    assert!(
        peak_growth > retained,
        "峰值增长 {peak_growth} 不大于常驻增长 {retained} —— 中间对象没被测量到，测试无效"
    );

    drop(map);
    let released = LIVE.load(Ordering::SeqCst);
    // 容差：测试框架自身在测量窗口里会分配几十~几百字节（打印/断言消息/线程局部），
    // 用严格相等会偶发假红。这里要挡的是"整个扁平表的量级（几百 KB ~ MB）没被释放"。
    let slack = 64 * 1024;
    assert!(
        released <= before + slack,
        "drop 之后存活字节 {released} 比解析前 {before} 高出 {slack} 字节以上 —— 有泄漏"
    );
}
