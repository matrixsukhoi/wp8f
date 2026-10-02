#!/usr/bin/env python3
"""i18n **键集对账**（B/C）：`resource/i18n/*.json` 内部一致，且与两处 `SUPPORTED` 对得上。

    python scripts/tests/i18n_keys.py            # 全量对账；不一致 → 退出码 1
    python scripts/tests/i18n_keys.py -v         # 额外打印每种语言的键数/表本体字节数
    python scripts/tests/i18n_keys.py -h

为什么单独有这么一个脚本（Rust 侧已经有两处 `every_supported_language_has_the_same_key_set_as_en`）：

* 那两条单测跑在 `cargo test -p wp8f-gui`（**只能编 Windows 目标**，WSL 里跑不了测试二进制）
  与 `cargo test -p wp8f-disp` 里 —— 改文案时先跑这个脚本能立刻给答案，不必等编译；
* 它们只看"键集是否一致"，**不看顺序**；本脚本默认把**顺序**也钉住
  （`resource/i18n/*.json` 是给人读的，顺序漂了 diff 会变成一坨）；
* 最重要的一条：**`SUPPORTED` 与磁盘上的文件名必须对得上**。
  B 把中文两份从 `zh-CN`/`zh-TW` 改成 `zh_simp`/`zh_trad` 时，最容易漏的就是
  "改了文件名、没改 `SUPPORTED`"（界面下拉少一项 / 启动日志说读不到语言包）。

断言口径（都能失败，没有恒真项）：
  1. `resource/i18n/` 下的 `<code>.json` 文件名集合 == `gui/src/i18n.rs::SUPPORTED`
     （GUI 的 15 种界面语言）；
  2. 每个文件都有非空 `_lang`（**必须等于文件名**）与非空 `_name`；
  3. 非 `_` 键集合与顺序与基准语言（`en`，兜底语言）完全一致；
  4. 占位符（`{name}` 这类）逐键一致 —— 少一个占位符会把消息里的实参吞掉；
  5. `resource/lang/`（**HUD 的标签表**，C 起与 i18n 分开）：
     * `disp/src/i18n.rs::SUPPORTED` 的每个 code 都必须有文件（内置只有 zh/en）；
     * 内置文件（`<code>.json`）的键集必须**覆盖** `resource/i18n/en.json` 里全部 `hud.*` 键
       （HUD 画标签只查这些键）；
     * 用户自建的 `xxx.json`（不在 SUPPORTED 里）只**报告**不判失败 —— 它可以用
       `hud_lang_path` 指过去、缺的键在运行期回退内嵌 en（`disp/src/i18n.rs` 的 missing/extra
       会记日志）。

退出码：0 通过；1 对账失败；2 环境不满足（读不到目录/文件）。
"""
import json
import re
import sys
from pathlib import Path

# 输出重定向到文件时 Windows 默认 GBK，打印 ✓/✕ 会 UnicodeEncodeError 直接崩
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = Path(__file__).resolve().parents[2]
I18N_DIR = ROOT / "resource" / "i18n"
# HUD 的标签表目录（C 起与 GUI 的 i18n 分开；见 disp/src/i18n.rs）
LANG_DIR = ROOT / "resource" / "lang"
# 基准语言：**兜底语言 en**（回退链的终点，键集以它为准）
BASE = "en"
VERBOSE = "-v" in sys.argv or "--verbose" in sys.argv


def usage() -> None:
    head = __doc__.split("退出码")[0]
    print(head.strip())
    raise SystemExit(0)


def supported_in(rs_file: Path) -> list[str]:
    """从 `xxx.rs` 里抠出 `pub const SUPPORTED: &[&str] = &[ … ];` 里的 code。

    宽松匹配（注释里出现 `"xx"` 不影响，因为只在 `SUPPORTED` 之后的第一对方括号里找）。
    抠不到就返回空表 —— 调用方会把它当"对账失败"，不会静默当成通过。
    """
    text = rs_file.read_text(encoding="utf-8")
    m = re.search(r"pub const SUPPORTED\s*:\s*&\[&str\]\s*=\s*&\[(.*?)\];", text, re.S)
    if not m:
        return []
    return re.findall(r'"([^"]+)"', m.group(1))


def placeholders(s: str) -> list[str]:
    out = []
    rest = s
    while True:
        i = rest.find("{")
        if i < 0:
            break
        j = rest.find("}", i)
        if j < 0:
            break
        out.append(rest[i + 1:j])
        rest = rest[j + 1:]
    return sorted(out)


def main() -> int:
    if "-h" in sys.argv or "--help" in sys.argv:
        usage()
    if not I18N_DIR.is_dir():
        print(f"✗ 读不到 {I18N_DIR}", file=sys.stderr)
        return 2

    problems: list[str] = []

    disk = sorted(p.stem for p in I18N_DIR.glob("*.json"))
    gui_rs = ROOT / "gui" / "src" / "i18n.rs"
    disp_rs = ROOT / "disp" / "src" / "i18n.rs"
    gui_supported = supported_in(gui_rs)
    disp_supported = supported_in(disp_rs)

    # ① 文件名 ↔ SUPPORTED
    #    `resource/i18n/` 归 **gui**（15 种界面语言）；`resource/lang/` 归 **disp**（HUD 标签表，
    #    C 起两份文件分家，两边各管一摊，不再要求两处 SUPPORTED 逐字一致）。
    if not gui_supported:
        problems.append(f"在 {gui_rs.relative_to(ROOT)} 里没抠到 SUPPORTED（正则失效？）")
    if not disp_supported:
        problems.append(f"在 {disp_rs.relative_to(ROOT)} 里没抠到 SUPPORTED（正则失效？）")
    if gui_supported and sorted(gui_supported) != disk:
        problems.append(f"gui/src/i18n.rs::SUPPORTED {sorted(gui_supported)} != {I18N_DIR.name}/ 磁盘文件 {disk}")

    # ②/③/④ 逐文件：_lang / _name / 键集与顺序 / 占位符
    # ⚠️ **先把全部文件读进来**再逐语言比：文件是按名字排序遍历的，基准语言 `en`
    # 排在 de/el 后面 —— 边读边比会让 de/el 拿着一张空基准表，报出满屏假差异。
    parsed: dict[str, tuple[list[str], dict[str, str]]] = {}
    rows: list[tuple[str, int, int]] = []
    for code in disk:
        path = I18N_DIR / f"{code}.json"
        try:
            pairs = json.loads(path.read_text(encoding="utf-8"),
                               object_pairs_hook=lambda kv: kv)
        except Exception as e:  # noqa: BLE001
            problems.append(f"{path.name}: JSON 解析失败：{e}")
            continue
        items = dict(pairs)
        meta_lang = items.get("_lang")
        meta_name = items.get("_name")
        if meta_lang != code:
            problems.append(f"{path.name}: _lang={meta_lang!r} 必须等于文件名 {code!r}")
        if not isinstance(meta_name, str) or not meta_name.strip():
            problems.append(f"{path.name}: 缺少非空 _name（语言下拉要显示它）")
        keys = [k for k, _ in pairs if not str(k).startswith("_")]
        by_key = {k: v for k, v in pairs if not str(k).startswith("_")}
        bytes_ = sum(len(str(k)) + len(str(v)) for k, v in by_key.items())
        parsed[code] = (keys, by_key)
        rows.append((code, len(keys), bytes_))

    if BASE not in parsed:
        problems.append(f"基准语言 {BASE}.json 读不到（它是兜底语言，必须有）")
        base_keys: list[str] = []
        base_map: dict[str, str] = {}
    else:
        base_keys, base_map = parsed[BASE]

    for code, (keys, by_key) in parsed.items():
        if code == BASE:
            continue
        if keys != base_keys:
            only_a = [k for k in base_keys if k not in by_key][:6]
            only_b = [k for k in keys if k not in base_map][:6]
            same_set = set(keys) == set(base_keys)
            problems.append(
                f"{code}.json: 与 {BASE} 的键{'顺序' if same_set else '集合'}不一致"
                f"（共 {len(keys)} 键；{BASE} 有它没有：{only_a}；它有 {BASE} 没有：{only_b}）")
        for k, v in by_key.items():
            want = placeholders(base_map.get(k, ""))
            if not want:
                continue
            got = placeholders(v if isinstance(v, str) else "")
            if got != want:
                problems.append(f"{code}.json: {k} 占位符不一致 {got} != {want}")

    # ⑤ HUD 的标签表目录 `resource/lang/`（C 起与 i18n 分家）
    hud_missing: list[str] = []
    if not LANG_DIR.is_dir():
        problems.append(f"读不到 HUD 语言目录 {LANG_DIR.relative_to(ROOT)}/（C 起 HUD 用它）")
    else:
        lang_files = sorted(p.stem for p in LANG_DIR.glob("*.json"))
        for code in disp_supported:
            if code not in lang_files:
                problems.append(f"disp/src/i18n.rs::SUPPORTED 里的 {code!r} 没有 {LANG_DIR.name}/{code}.json")
        # HUD 只查 `hud.*` 键：内置文件必须覆盖 i18n 基准（en）里的全部 hud.* 键
        need = sorted(k for k in base_keys if k.startswith("hud."))
        for code in disp_supported:
            p = LANG_DIR / f"{code}.json"
            if not p.is_file():
                continue
            try:
                pairs = json.loads(p.read_text(encoding="utf-8"), object_pairs_hook=lambda kv: kv)
            except Exception as e:  # noqa: BLE001
                problems.append(f"{LANG_DIR.name}/{p.name}: JSON 解析失败：{e}")
                continue
            got = {k for k, _ in pairs}
            lost = [k for k in need if k not in got]
            if lost:
                problems.append(
                    f"{LANG_DIR.name}/{p.name}: 缺 {len(lost)} 个 hud.* 键（HUD 会回退内嵌 en）：{lost[:6]}")
            # `_lang` 必须等于文件名（与 i18n 同一口径）
            items = dict(pairs)
            if items.get("_lang") != code:
                problems.append(f"{LANG_DIR.name}/{p.name}: _lang={items.get('_lang')!r} 必须等于文件名 {code!r}")
        extra_lang = [c for c in lang_files if c not in disp_supported]
        if VERBOSE:
            print(f"HUD 语言目录 {LANG_DIR.name}/：内置 {disp_supported}；"
                  f"自建文件 {extra_lang or '（无）'}；hud.* 键 {len(need)} 条")
            for code in lang_files:
                p = LANG_DIR / f"{code}.json"
                pairs = json.loads(p.read_text(encoding="utf-8"), object_pairs_hook=lambda kv: kv)
                body = sum(len(str(k)) + len(str(v)) for k, v in pairs if not str(k).startswith("_"))
                n = sum(1 for k, _ in pairs if not str(k).startswith("_"))
                tag = "内置" if code in disp_supported else "自建（可用 hud_lang_path 指过去）"
                print(f"  {code:<12} {n:>4} 键   表本体 {body:>6} B   {tag}")

    if VERBOSE:
        print(f"基准语言 {BASE}：{len(base_keys)} 键")
        for code, n, b in rows:
            print(f"  {code:<10} {n:>5} 键   表本体 {b:>8} B")
        total = sum(b for _, _, b in rows)
        print(f"  十五种合计：{total} B（{total / 1024:.1f} KiB）")

    if problems:
        print(f"✗ i18n 键集对账失败（{len(problems)} 处）：", file=sys.stderr)
        for p in problems:
            print(f"    - {p}", file=sys.stderr)
        print("\n  怎么办：文案以 `resource/i18n/en.json`（兜底语言）为准逐键补齐；"
              "改语言代码要同时改**文件名**、`_lang`、`SUPPORTED` 与 `LEGACY_ALIASES`；"
              "HUD 的标签表在 `resource/lang/`（键集 = i18n 里全部 `hud.*` 键）"
              "（见 docs/i18n.md §2/§4）。", file=sys.stderr)
        return 1

    print(f"✓ i18n 键集对账通过：{len(disk)} 种界面语言 × {len(base_keys)} 键 + "
          f"HUD 语言表 {len(disp_supported)} 份（{LANG_DIR.name}/），"
          f"`_lang`/`_name`/键集与顺序/占位符全部一致，SUPPORTED 与磁盘文件名对得上")
    return 0


if __name__ == "__main__":
    sys.exit(main())
