//! 一帧的**格式化阶段**：各 HUD 模块在这里登记自己的格式化函数。
//!
//! 流水：core 读 8111 → core 算数值 → **本模块：统一格式化** → 整帧进环 → 渲染器只落屏。
//!
//! 登记是**编译期**的：下面的宏把表展开成顺序直写调用（`f1(d); f2(d); …`），
//! 不经过 `fn` 指针、trait 对象或任何运行期注册表 —— core 侧只看到两个普通函数，
//! 内联后与手写调用没有区别。加一个 HUD 模块 = 在表里加一行，不用改 core。
//!
//! 谁该登记在这里：**所有把数值变成文本的代码**（面板语义文本、两个 HUD 的版式文本、
//! 友军行、地图标签、状态项…）。渲染器里不许出现 `format_args!`/`format!`。
//! 不登记的只有颜色/布局这类"画的时候才知道"的决定。

use super::{DisplayData, MapDisplay};

/// 显示侧格式化登记表（作用于 `DisplayData`）。
macro_rules! hud_display_formatters {
    ($($formatter:path),* $(,)?) => {
        /// 显示侧格式化：一帧按 `DisplayData` 要印的文本在这里一次算完。
        #[inline]
        pub fn format_display(d: &mut DisplayData) {
            $( $formatter(d); )*
        }
    };
}

/// 地图侧格式化登记表（作用于 `MapDisplay`）。
macro_rules! hud_map_formatters {
    ($($formatter:path),* $(,)?) => {
        /// 地图侧格式化：`MapDisplay` 上的标签/距离文本在这里一次算完。
        #[inline]
        pub fn format_map(m: &mut MapDisplay) {
            $( $formatter(m); )*
        }
    };
}

hud_display_formatters!(crate::hud_text::fill);

hud_map_formatters!(crate::hud_text::fill_map);
