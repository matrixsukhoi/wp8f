# Voice Warning Module Plan

## Overview

实现基于 voidmei 的语音告警模块，支持 JSON 配置自定义告警规则。

## Voice Files

位于仓库根的 `resource/voice/`（原先在 `voice/`，2026-10 资源统一到 `resource/`）：

| File | Purpose |
|------|---------|
| `aoaCrit.wav` | Angle of Attack critical warning |
| `aoaHigh.wav` | Angle of Attack high warning |
| `warn_stall.wav` | Stall warning |
| `warn_ias.wav` | Indicated Airspeed exceedance |
| `warn_mach.wav` | Mach limit exceedance |
| `warn_gear.wav` | Landing gear overspeed |
| `warn_flap.wav` | Flap overspeed |
| `warn_loadfactor.wav` | G-load limits exceedance |
| `warn_engineoverheat.wav` | Engine overheat |
| `fail_engine.wav` | Engine failure |
| `fail_nofuel.wav` | Out of fuel |
| `warn_lowfuel.wav` | Low fuel warning |
| `warn_lowpressure.wav` | Low fuel pressure |
| `warn_altitude.wav` | High descent rate |
| `warn_terrain.wav` | Terrain collision warning |
| `warn_highvario.wav` | High vertical speed (descent) |
| `warn_brake.wav` | Speed brake warning |
| `warn_lowrpm.wav` | Low RPM warning |
| `warn_highrpm.wav` | High RPM warning |
| `rudderEff.wav` | Rudder effectiveness warning |
| `elevatorEff.wav` | Elevator effectiveness warning |
| `aileronEff.wav` | Aileron effectiveness warning |

## Implementation Approach

### 1. Audio Playback

Use `rodio` crate for cross-platform audio:
```toml
rodio = "0.19"
```

### 2. Warning Configuration

JSON file (`config/warnings.json`):
```json
{
  "enabled": true,
  "volume": 100,
  "warnings": {
    "stall": { "file": "warn_stall.wav", "condition": "ias < stall_speed" },
    "overspeed": { "file": "warn_ias.wav", "threshold": 0.95, "source": "vne" },
    "low_fuel": { "file": "warn_lowfuel.wav", "threshold_percent": 10 },
    "mach": { "file": "warn_mach.wav", "threshold": 0.95, "source": "mne" },
    "gear": { "file": "warn_gear.wav", "condition": "gear_down && ias > gear_limit" },
    "flap": { "file": "warn_flap.wav", "condition": "flaps_extended && ias > flap_limit" },
    "g_load": { "file": "warn_loadfactor.wav", "threshold": 1.0, "source": "ny" },
    "engine_overheat": { "file": "warn_engineoverheat.wav", "threshold": 900, "source": "water_temp" },
    "engine_failure": { "file": "fail_engine.wav", "condition": "engine_damage && fuel_pressure == 0" },
    "out_of_fuel": { "file": "fail_nofuel.wav", "condition": "fuel == 0" },
    "low_fuel": { "file": "warn_lowfuel.wav", "threshold_percent": 10 },
    "low_fuel_pressure": { "file": "warn_lowpressure.wav", "condition": "fuel_pressure_low" },
    "descent_rate": { "file": "warn_altitude.wav", "threshold": -8, "source": "vy" },
    "terrain": { "file": "warn_terrain.wav", "condition": "radio_alt < 100 && descending" },
    "high_vario": { "file": "warn_highvario.wav", "threshold": -15, "source": "vy" },
    "speed_brake": { "file": "warn_brake.wav", "condition": "airbrake > 0.9 && gear_up" },
    "low_rpm": { "file": "warn_lowrpm.wav", "threshold": 0.5, "source": "rpm_ratio" },
    "high_rpm": { "file": "warn_highrpm.wav", "threshold": 1.05, "source": "rpm_ratio" },
    "rudder_eff": { "file": "rudderEff.wav", "condition": "ias > rudder_eff_speed" },
    "elevator_eff": { "file": "elevatorEff.wav", "condition": "ias > elevator_eff_speed" },
    "aileron_eff": { "file": "aileronEff.wav", "condition": "ias > aileron_eff_speed" }
  }
}
```

### 3. Module Structure

```
core/src/
├── warnings.rs          # VoiceWarning module
└── warnings/
    ├── mod.rs           # Module exports
    ├── config.rs        # WarningConfig loading
    ├── audio.rs         # Audio playback via rodio
    └── triggers.rs      # Warning condition evaluation
```

**VoiceWarning struct:**
```rust
pub struct VoiceWarning {
    config: WarningConfig,
    audio_sinks: HashMap<String, OutputStream>,
    cooldowns: HashMap<String, Instant>,
    last_states: HashMap<String, bool>,
}

impl VoiceWarning {
    pub fn new(config_path: &str) -> Result<Self, Error>;
    pub fn check_and_play(&mut self, data: &FlightState, indic: &Indicators, fm: &FlightModel);
    fn play_warning(&mut self, name: &str);
    fn evaluate_condition(&self, name: &str, data: &FlightState, indic: &Indicators, fm: &FlightModel) -> bool;
}
```

### 4. Warning Triggers

| Warning | Condition | Threshold Source |
|---------|-----------|------------------|
| Stall | IAS < stall_speed | FM or default 200 km/h |
| Overspeed | IAS >= VNE * 0.95 | FM |
| Mach | Mach >= MNE * 0.95 | FM |
| Low Fuel | fuel_percent < 10% | FM or default |
| Engine Overheat | water_temp > 900°C | FM |
| G-Load | Ny > max_g or Ny < min_g | FM |
| Landing Gear | gear_down && IAS > gear_limit | FM |
| Control Surface | IAS > aileron/elevator/rudder_eff_speed | FM |
| Engine Failure | engine_damage && fuel_pressure == 0 | - |
| Out of Fuel | fuel == 0 | - |

### 5. Edge Detection

Only trigger on state CHANGE to avoid spam:
- Track previous state per warning in `last_states`
- Only play when: `!last_states[name] && evaluate_condition()`

### 6. Cooldown

Each warning has a cooldown period (e.g., 2 seconds) to prevent repeated rapid playback:
```rust
if let Some(last_play) = cooldowns.get(name) {
    if last_play.elapsed() < cooldown_duration {
        return; // Still in cooldown
    }
}
```

## Integration with Core

In `core/src/main.rs`:
```rust
let mut voice_warning = VoiceWarning::new(hud_layout().voice_warnings_enabled).ok();

loop {
    // ... data processing ...
    
    if let Some(ref mut vw) = voice_warning {
        vw.check_and_play(&state, &indic, &fm);
    }
}
```

## Visual Alarm (HUD blinking X)

一旦语音告警触发（`VoiceWarning::trigger`：任一告警条件成立并尝试播放语音），
miniHUD 与 circle HUD 会以 **4Hz** 频率闪烁绘制**绿色 X**：
miniHUD 的 X 覆盖整个 miniHUD 面板区域，circle HUD 的 X **内切于圆环**（四角落在圆环上，不超出圆环）。

- **触发**：`check_*` 系列函数通过 `trigger()` 同时标记可视告警与播放语音；
  只要告警条件成立就保持告警状态（不受语音冷却时间影响），
  条件消失后再保持 `ALARM_HOLD_FRAMES`（30 帧 ≈ 1 秒）熄灭。
- **状态传递**：`VoiceWarning` → `DisplayData.voice_alarm`（共享内存环形缓冲）→ `disp` 绘制。
- **绘制**：`disp/src/hud_common.rs`
  - `warning_blink_visible()`：基于墙钟时间的闪烁门控，与渲染帧率无关，各 HUD 同步
  - `draw_warning_x()`：数字色 X（`data_color` + 深色轮廓；配置键 `warning_x_color` 已删除）
  - `draw_voice_alarm_x()`：`voice_alarm` 有效时按闪烁门控绘制
  - `mini_hud.rs` / `circle_hud.rs` 各自调用：miniHUD 覆盖面板包围盒，circle HUD 内切于圆环
- **配置**（`config/*.json`）：

  | 键 | 默认 | 说明 |
  |----|------|------|
  | `warning_blink_hz` | `4.0` | 闪烁频率（明暗周期/秒，`<= 0` 为常亮） |

  X 的颜色**不再是独立配置键**（`warning_x_color` 已删除）：统一取数字色 `data_color`。

- **无音频环境**：配置项 `voice_warnings_enabled: false`（默认 `true`）或没有可用音频输出设备时
  自动降级为静默检查，可视告警仍然有效。语音告警**没有编译期 feature**，永远编译进程序。

## Questions

1. **Audio Library:** Is `rodio` acceptable?
2. **Config Location:** Default to `./config/warnings.json` or require full path?
3. **Warning Priority:** If multiple warnings trigger simultaneously, queue or mix?
4. **Per-Aircraft Config:** Should warnings use aircraft-specific FM data or global defaults?
