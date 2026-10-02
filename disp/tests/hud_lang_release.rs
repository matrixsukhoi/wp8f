//! C 的**可测证据**：HUD 语言文件读完后**只留只读表**（常驻 / 峰值 / 回到基线）。
//!
//! 做法与 `flightmodel/tests/json_release.rs` 同一套：本测试 crate 装一个**计数分配器**，
//! 量三个数：
//!
//! * `before` —— 调用前的存活字节（基线）；
//! * `peak`   —— 调用期间的峰值（含"文件文本 + `serde_json::Value`"这些中间对象）；
//! * `after`  —— 调用返回后的存活字节（应当只剩那张 `Box::leak` 的只读表）。
//!
//! 断言（都能失败，没有恒真项）：
//! 1. `after - before` 不超过表本体（`LangInfo::bytes`）的 2 倍 + 固定余量
//!    —— 说明 JSON 文本与 `Value` **确实被释放了**；
//! 2. `LangInfo::json_bytes` == 文件字节数、`bytes`/`keys` == 文件里**非 `_` 键**的那部分
//!    —— 说明"读进来的是整份文件、留下的是 HUD 真正要查的标签"，`_about`/`_name` 这类
//!    元数据（`_about` 往往是文件里最长的一串）**一个字都没常驻**；
//! 3. `peak - before` 明显大于 `after - before`（至少多出文件文本那一份）
//!    —— 说明中间对象**确实存在过**，否则这条测量本身就是假的；
//! 4. 换一份**合成的大语言文件**再量一次，结论不变（真实文件只有 1.6 KB，量级太小看不出问题）。
//!
//! ⚠️ 计数分配器是**进程级**的：这个测试 crate 里只有本文件，"回到基线"的口径才成立
//! （同一个二进制里跑别的用例会让计数互相污染）—— 所以它放在 `tests/` 而不是 `src/` 的
//! 单测里（后者与 HUD 渲染用例共享进程）。

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// 两个用例共用一把锁：`LIVE`/`PEAK` 是**进程级**的，两个用例并行跑会互相把对方的
/// 分配算进来（第一版就是这么假红的：另一个用例的分配让"常驻增长"虚高到 99 KB）。
static MEASURE: Mutex<()> = Mutex::new(());

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

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// 量一次 `set_lang(path)`：返回 (常驻增长, 峰值增长, 加载概况)。
fn measure(path: &str) -> (usize, usize, wp8f_disp::i18n::LangInfo) {
    // 预热一次：HashMap/分配器的首次增长、thread_local、tracing 的 callsite 等一次性分配先发生掉
    let warm = wp8f_disp::i18n::set_lang(path).expect("预热加载");
    let (warm_bytes, warm_keys) = (warm.bytes, warm.keys);
    drop(warm);

    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let info = wp8f_disp::i18n::set_lang(path).expect("加载");
    let after = LIVE.load(Ordering::SeqCst);
    let peak = PEAK.load(Ordering::SeqCst);
    assert_eq!(info.bytes, warm_bytes, "两次加载的表本体应当一致");
    assert_eq!(info.keys, warm_keys, "两次加载的键数应当一致");
    (after.saturating_sub(before), peak.saturating_sub(before), info)
}

/// 语言文件里**该留下的**：非 `_` 键的 (键字节 + 值字节) 与条数 —— 直接按文件文本算出来的期望值。
fn expected_from_text(text: &str) -> (usize, usize) {
    let parsed: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_str(text).expect("语言文件必须是合法 JSON 对象");
    let mut bytes = 0usize;
    let mut keys = 0usize;
    for (k, v) in parsed.iter() {
        if k.starts_with('_') {
            continue; // `_lang`/`_name`/`_about` 是元数据，**不许**常驻
        }
        bytes += k.len() + v.as_str().expect("语言表只收键 → 字符串").len();
        keys += 1;
    }
    (bytes, keys)
}

/// 只读表的合理占用上限 = 表本体（键值字节）+ 每 entry 的 HashMap 开销（保守 64 B）+ 余量。
fn allowance(table: usize, keys: usize) -> usize {
    table + keys * 64 + 4 * 1024
}

/// 真实语言文件（`resource/lang/zh.json`）：常驻只该是表本体那么多，
/// 而且**留下的就是"非 `_` 键"那部分**（`_about` 那类元数据一个字都不许常驻）。
#[test]
fn real_hud_lang_file_leaves_only_the_read_only_table() {
    let _g = MEASURE.lock().unwrap_or_else(|e| e.into_inner());
    let file = repo_root().join("resource").join("lang").join("zh.json");
    let text = std::fs::read_to_string(&file).expect("resource/lang/zh.json 必须存在");
    let (want_bytes, want_keys) = expected_from_text(&text);

    let (retained, peak, info) = measure(&file.display().to_string());
    let cap = allowance(info.bytes, info.keys);
    eprintln!(
        "resource/lang/zh.json：文件 {} B / 标签 {} 键；读入 JSON {} B；常驻增长 {} B；峰值增长 {} B；\
         表本体 {} B（期望 {} B）；上限 {} B",
        text.len(), info.keys, info.json_bytes, retained, peak, info.bytes, want_bytes, cap
    );

    assert_eq!(info.json_bytes, text.len(), "`json_bytes` 应当是**整个文件**的字节数（读入量）");
    assert_eq!(info.keys, want_keys, "留下的键数应当正好是文件里非 `_` 键的条数（元数据不进表）");
    assert_eq!(
        info.bytes, want_bytes,
        "表本体应当是「非 `_` 键的键值字节和」—— 多出来的部分说明把 `_about` 之类的元数据也留下了"
    );
    assert!(
        retained <= cap,
        "常驻增长 {retained} B 超过「表本体 {} B + {} 键的桶开销 + 余量」= {cap} B \
         —— JSON 文本/中间对象没被释放？",
        info.bytes, info.keys
    );
    assert!(
        peak > retained,
        "峰值增长 {peak} B 不大于常驻增长 {retained} B —— 中间对象没被测量到，测试无效"
    );
    // 元数据有没有被漏掉：`_about` 是这份文件里最长的一串，常驻量必须明显小于整个文件
    assert!(
        info.bytes < text.len(),
        "表本体 {} B 不小于文件 {} B —— 像是把整份 JSON 留下了",
        info.bytes, text.len()
    );
}

/// 合成一份**大**语言文件（同样的键形状、值加长）：量级够大，才看得出"释放"。
#[test]
fn big_synthetic_hud_lang_file_is_released_too() {
    let _g = MEASURE.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join("wp8f_hud_lang_release");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("big.json");

    // 400 条 × 每条 ~120 字节的值 → 文件 ~55 KB，表本体 ~51 KB
    let mut s = String::from("{\n");
    for i in 0..400 {
        s.push_str(&format!(
            "  \"hud.synthetic.{i:04}\": \"{}\"{}\n",
            "x".repeat(110),
            if i == 399 { "" } else { "," }
        ));
    }
    s.push_str("}\n");
    std::fs::write(&file, &s).unwrap();

    let (retained, peak, info) = measure(&file.display().to_string());
    let cap = allowance(info.bytes, info.keys);
    eprintln!(
        "合成大语言文件：文件 {} B / {} 键；读入 JSON {} B；常驻增长 {} B；峰值增长 {} B；表本体 {} B；上限 {} B",
        s.len(), info.keys, info.json_bytes, retained, peak, info.bytes, cap
    );
    assert_eq!(info.json_bytes, s.len(), "`json_bytes` 应当是整个文件的字节数");

    assert!(
        retained <= cap,
        "常驻增长 {retained} B 超过「表本体 {} B + {} 键的桶开销 + 余量」= {cap} B \
         —— 大文件的 JSON 没被释放",
        info.bytes, info.keys
    );
    // 峰值至少要高出"文件文本 + Value 中间对象"这一份：文件文本本身就有 s.len() 字节
    assert!(
        peak.saturating_sub(retained) >= s.len() / 2,
        "峰值比常驻只高 {} B（文件 {} B）—— 中间对象没被测量到，测试无效",
        peak.saturating_sub(retained),
        s.len()
    );

    let _ = std::fs::remove_dir_all(&dir);
}
