#!/usr/bin/env python3
"""由 `resource/i18n/<code>.json` 生成随包 HUD 标签表 `resource/lang/{zh,en}.json`。

**真值唯一**：`hud.*` 标签只在 `resource/i18n/*.json` 里维护（15 种语言，控制台与 HUD 共用同一套
键），`resource/lang/*.json` 是**生成产物** —— HUD 的 `hud_lang_path` 与 GUI 的语言下拉仍读它，
所以它还得存在、而且必须与 i18n 一致（`--check` 就是干这个）。

    python scripts/gen_lang.py          # 重新生成（幂等：内容没变就不写盘）
    python scripts/gen_lang.py --check  # 只校验：过期返回 1（测试/CI 用）

映射（lang code ← i18n code）：`zh` ← `zh_simp`，`en` ← `en`。
目标文件的 `_lang` / `_name` / `_about` 等元数据**原样保留**（只刷新 `hud.*` 部分），
所以第一次运行是零 diff。

退出码：0 成功或已是最新；1 `--check` 下过期；2 参数/文件问题。
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LANG_DIR = ROOT / "resource" / "lang"
I18N_DIR = ROOT / "resource" / "i18n"

# lang code → i18n code（新增一门随包 HUD 语言：加一行，并让
# `disp/src/i18n.rs::SUPPORTED` 与磁盘文件名对上 —— `scripts/tests/i18n_keys.py` 会核对）
MAP = {
    "zh": "zh_simp",
    "en": "en",
}


def read_json(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        sys.exit(f"✗ 找不到 {path}（真值在 {I18N_DIR.relative_to(ROOT)}/，随包表由它生成）")
    except json.JSONDecodeError as e:
        sys.exit(f"✗ {path} 不是合法 JSON：{e}")


def render(lang: str, i18n: str) -> str:
    lang_path = LANG_DIR / f"{lang}.json"
    i18n_path = I18N_DIR / f"{i18n}.json"
    # 元数据（_lang/_name/_about …）以**现有随包表**为准：它们描述的是"HUD 这份表"本身
    meta = {k: v for k, v in read_json(lang_path).items() if str(k).startswith("_")}
    hud = {k: v for k, v in read_json(i18n_path).items() if str(k).startswith("hud.")}
    if not hud:
        sys.exit(f"✗ {i18n_path} 里没有任何 `hud.*` 键（标签真值必须在那里）")
    text = json.dumps({**meta, **hud}, ensure_ascii=False, indent=2) + "\n"
    return text.replace("\r\n", "\n")


def main() -> int:
    check = "--check" in sys.argv[1:]
    unknown = [a for a in sys.argv[1:] if a != "--check"]
    if unknown:
        sys.exit(f"✗ 未知参数：{' '.join(unknown)}（用法：gen_lang.py [--check]）")

    stale = []
    for lang, i18n in MAP.items():
        target = LANG_DIR / f"{lang}.json"
        want = render(lang, i18n)
        have = target.read_text(encoding="utf-8").replace("\r\n", "\n") if target.exists() else ""
        if want == have:
            print(f"· {target.relative_to(ROOT)} 已是最新（{i18n}）")
            continue
        stale.append(target.name)
        if check:
            continue
        with open(target, "w", encoding="utf-8", newline="\n") as f:
            f.write(want)
        print(f"✓ 生成 {target.relative_to(ROOT)}（来源 {i18n}）")

    if stale:
        if check:
            print("✗ 随包 HUD 标签表与 i18n 不一致：" + "，".join(stale), file=sys.stderr)
            print("  跑 `python scripts/gen_lang.py` 重新生成（hud.* 的真值在 "
                  "resource/i18n/，别直接改 resource/lang/）", file=sys.stderr)
            return 1
        return 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
