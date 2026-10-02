/// 语音资源根目录：**相对安装根**（控制台 `wp8f-gui.exe`（拖拽预览）与命令行拉起的
/// HUD 时 cwd 都是仓库根；也接受 exe 目录/上一级，见 [`resolve_dir`]）。
///
/// 语音按**包**组织：`resource/voice/<包名>/*.wav`，读哪个包由配置键 `voice_path` 定。
/// `voice_path` 决定（默认 [`DEFAULT_VOICE_PATH`]）。散在这个根目录下的 `*.wav`
/// **不会被加载**（控制台的语音包面板会把它们列出来提醒移位）。
pub const VOICE_DIR: &str = "resource/voice";

/// 默认语音包（D 之前那些 wav 整体搬进这个子目录，文件名原样）：
/// 配置键 `voice_path` 的 serde 默认值与 `config/default.json` 里写的都是它。
pub const DEFAULT_VOICE_PATH: &str = "resource/voice/zh_xiaoxuan_calm_youngadultfemale_1_04";

/// 代码里真正会点播的 wav（**不带扩展名**）：预载时逐条核对，缺哪个就明确警告一次。
///
/// 与 `play_wav` 的调用点一一对应（`check_*` / `play_startup`）；这份清单只用于**启动自检**
/// （真正加载的是目录里**全部** `*.wav`，用户往包里加新音不用改代码）。
pub const REQUIRED_VOICE: &[&str] = &[
    "start1",                // play_startup
    "aoaCrit", "aoaHigh",    // check_aoa
    "warn_ias",              // check_ias
    "warn_mach",             // check_mach
    "warn_gear",             // check_gear
    "warn_flap",             // check_flaps
    "warn_brake",            // check_brake
    "warn_engineoverheat",   // check_overheat
    "warn_loadfactor",       // check_g_load
    "warn_lowfuel",          // check_low_fuel
    "fail_nofuel",           // check_no_fuel
    "fail_engine",           // check_engine_failure（当前未接线：8111 没有燃油压力）
    "warn_highvario",        // check_vario
    "warn_altitude",         // check_altitude
    "warn_lowrpm", "warn_highrpm", // check_rpm
];

/// Visual alarm (blinking X) keeps flashing this long after the last voice
/// warning trigger, so short spikes are still noticeable (30 fps frames).
pub const ALARM_HOLD_FRAMES: u64 = 30;

/// Heavy warning checks run every N frames (the visual alarm flag is still
/// refreshed on every frame in `check_all`).
pub const CHECK_INTERVAL_FRAMES: u64 = 8;

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// 缺件只报一次：告警检查跑在 8 帧节拍上（`CHECK_INTERVAL_FRAMES`），
/// 不去重的话一个缺失的 wav 会在每次触发时刷屏。
fn report_missing_voice(path: &str) {
    static REPORTED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let reported = REPORTED.get_or_init(|| Mutex::new(HashSet::new()));
    let first = reported
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(path.to_string());
    if first {
        eprintln!("[VOICE] 找不到语音文件 {path}；语音资源应位于 {VOICE_DIR}/<语音包>/（配置键 voice_path）");
    }
}

/// **预载进内存的语音包**（D）：启动时把 `voice_path` 目录里的 `*.wav` 一次性读成字节，
/// `Box::leak` 成 `&'static [u8]` 常驻；之后播放走 `Decoder::new(Cursor<…>)`，
/// **运行期零磁盘 I/O**（目录改名/删除都不影响已载入的声音）。
#[derive(Debug)]
pub struct VoicePack {
    /// 实际加载的目录（相对安装根的写法，就是配置里 `voice_path` 的值）
    pub dir: String,
    /// 文件主干（`warn_gear`）→ wav 原始字节
    files: HashMap<String, &'static [u8]>,
    /// 加载成功的 wav 个数与总字节（"预载了多少"的实测口径）
    pub count: usize,
    pub total_bytes: usize,
/// 没能加载的（文件名 → 原因）：缺件 / 读失败 / 不是 RIFF-WAVE。只警告，不让 HUD 起不来。
    pub skipped: Vec<(String, String)>,
}

impl VoicePack {
    /// 取一个 wav 的字节（**只查内存**）。
    fn get(&self, name: &str) -> Option<&'static [u8]> {
        self.files.get(name).copied()
    }

    /// 文件是否在包里（诊断用）
    pub fn has(&self, name: &str) -> bool {
        self.files.contains_key(name)
    }

    pub fn names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.files.keys().map(|s| s.as_str()).collect();
        v.sort_unstable();
        v
    }
}

static PACK: OnceLock<VoicePack> = OnceLock::new();

/// 候选基准目录（按顺序试）：cwd、cwd 的上一级、exe 所在目录、exe 所在目录的上一级、
/// 以及测试时的仓库根（cargo 把测试进程的 cwd 设成包目录）。
fn dir_bases() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut out: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.clone());
        if let Some(up) = cwd.parent() {
            out.push(up.to_path_buf());
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.to_path_buf());
            if let Some(up) = dir.parent() {
                out.push(up.to_path_buf());
            }
        }
    }
    #[cfg(test)]
    out.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".."));
    out
}

/// 把配置里的 `voice_path`（相对安装根的**目录**）解析成一个真实存在的目录。
///
/// 找不到 → `Err`，调用方当**警告**处理（语音是增强功能，缺了 HUD 照常显示，只是没声音）。
pub fn resolve_dir(voice_path: &str) -> Result<std::path::PathBuf, String> {
    use std::path::Path;
    let raw = voice_path.trim();
    if raw.is_empty() {
        return Err(format!("voice_path 为空；应指向 {VOICE_DIR}/<语音包>/"));
    }
    let p = Path::new(raw);
    let mut tried: Vec<std::path::PathBuf> = Vec::new();
    if p.is_absolute() {
        tried.push(p.to_path_buf());
    } else {
        for base in dir_bases() {
            tried.push(base.join(p));
        }
    }
    tried
        .iter()
        .find(|c| c.is_dir())
        .cloned()
        .ok_or_else(|| {
            format!(
                "找不到语音包目录 {raw}（找过 {}）；应指向 {VOICE_DIR}/ 下的子目录",
                tried.iter().map(|t| t.display().to_string()).collect::<Vec<_>>().join(" / ")
            )
        })
}

/// 目录里是不是一个合法的 WAV（RIFF/WAVE 头）。只看头 12 字节 —— 目的是把
/// "0 字节 / 下坏了的文件"挡在预载之外并**明确报告**，而不是等播放时才莫名静音。
fn is_riff_wave(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

/// **启动时预载整个语音包**（幂等：只有第一次真读盘）。
///
/// 目录不存在 / 一个 wav 都没有 → 返回一个空包（`count == 0`），调用方打警告即可
/// 语音缺失**不让 HUD 起不来**（与字体那条口径相反）。
/// 单个文件读失败或不是 WAV → 记进 [`VoicePack::skipped`]（去重警告），**跳过它继续加载别的**。
pub fn preload(voice_path: &str) -> &'static VoicePack {
    PACK.get_or_init(|| {
        let dir = match resolve_dir(voice_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[VOICE] {e}");
                return VoicePack {
                    dir: voice_path.to_string(),
                    files: HashMap::new(),
                    count: 0,
                    total_bytes: 0,
                    skipped: Vec::new(),
                };
            }
        };
        let mut files: HashMap<String, &'static [u8]> = HashMap::new();
        let mut skipped: Vec<(String, String)> = Vec::new();
        let mut total_bytes = 0usize;
        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("wav")).unwrap_or(false))
                    .collect()
            })
            .unwrap_or_default();
        entries.sort(); // 稳定顺序：日志与实测数字可复现
        for path in entries {
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            match std::fs::read(&path) {
                Ok(bytes) if is_riff_wave(&bytes) => {
                    total_bytes += bytes.len();
                    // Box::leak：进程生命周期内只读一次（与字体同一套手法），之后播放不再碰磁盘
                    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
                    files.insert(name, leaked);
                }
                Ok(bytes) => skipped.push((
                    name,
                    format!("不是 RIFF/WAVE（{} 字节，头 4 字节 {:?}）", bytes.len(),
                            String::from_utf8_lossy(&bytes[..bytes.len().min(4)])),
                )),
                Err(e) => skipped.push((name, format!("读失败：{e}"))),
            }
        }
        // 代码里会点播的那些：缺一个就明确警告一次（不是致命错误）
        for need in REQUIRED_VOICE {
            if !files.contains_key(*need) {
                eprintln!("[VOICE] 语音包里缺少 {need}.wav（{}）—— 该告警将保持静音", dir.display());
            }
        }
        for (name, why) in &skipped {
            eprintln!("[VOICE] 跳过损坏/读不了的语音 {name}.wav（{why}）");
        }
        VoicePack {
            dir: voice_path.to_string(),
            count: files.len(),
            total_bytes,
            files,
            skipped,
        }
    })
}

/// 已预载的包（没预载时是 `None`）。**播放路径只读它，不碰磁盘。**
pub fn pack() -> Option<&'static VoicePack> {
    PACK.get()
}

/// 预载概况：`(目录, 文件数, 总字节)`（启动日志用）。
pub fn pack_summary() -> Option<(String, usize, usize)> {
    pack().map(|p| (p.dir.clone(), p.count, p.total_bytes))
}

use wp8f_flightmodel::FlightModel;

#[derive(Clone)]
/// 告警阈值。9 个字段里只有下面 3 个真的被读（其余 6 个是历史遗留，
/// 曾经挂在 `#[allow(dead_code)]` 的 impl 底下从来没被发现）。
pub struct WarningThresholds {
    pub low_fuel_percent: f64,
    pub engine_temp_warning: f64,
    pub vario_warning: f64,
}

impl Default for WarningThresholds {
    fn default() -> Self {
        Self {
            low_fuel_percent: 10.0,
            engine_temp_warning: 900.0,
            vario_warning: -8.0,
        }
    }
}

pub struct VoiceWarning {
    /// 音频开关（配置项 `voice_warnings_enabled`，默认 true）。
    /// 启动时读一次：HUD 布局配置不热重载。`false` 时不初始化音频设备、
    /// 不播放任何声音，但告警检查与视觉告警（闪烁 X）完全不受影响。
    enabled: bool,
    _stream: Option<rodio::OutputStream>,
    _stream_handle: Option<rodio::OutputStreamHandle>,
    sinks: HashMap<String, rodio::Sink>,
    cooldowns: HashMap<String, Instant>,
    _last_states: HashMap<String, bool>,
    _frame_count: u64,
    alarm_active: bool,
    alarm_until_frame: u64,
    thresholds: WarningThresholds,
}

impl VoiceWarning {
    fn init_with_thresholds(
        enabled: bool,
        thresholds: WarningThresholds,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        use rodio::OutputStream;
        // 关掉开关时不碰声卡（不占音频设备）；没有可用音频设备时同样降级为
        // 静默检查，保证视觉告警（闪烁 X）照常工作。
        let (stream, stream_handle) = if enabled {
            match OutputStream::try_default() {
                Ok((stream, handle)) => (Some(stream), Some(handle)),
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };
        Ok(Self {
            enabled,
            _stream: stream,
            _stream_handle: stream_handle,
            sinks: HashMap::new(),
            cooldowns: HashMap::new(),
            _last_states: HashMap::new(),
            _frame_count: 0,
            alarm_active: false,
            alarm_until_frame: 0,
            thresholds,
        })
    }

    /// `enabled` 来自布局配置 `voice_warnings_enabled`（默认 true），
    /// 只在启动时读一次（HUD 布局配置不热重载）。
    pub fn new(enabled: bool) -> Result<Self, Box<dyn std::error::Error>> {
        Self::init_with_thresholds(enabled, WarningThresholds::default())
    }

    fn get_cooldown_secs(name: &str) -> u64 {
        match name {
            "aoa_crit" => 1,
            "aoa_high" | "flaps" => 8,
            "stall" | "g_load" => 2,
            "gear" | "brake" => 7,
            "ias" | "mach" | "rpm_low" | "rpm_high" | "altitude" | "vario" => 10,
            "overheat" | "low_fuel" | "no_fuel" | "engine_fail" => 60,
            _ => 10,
        }
    }

    /// 播放一个告警语音（**只从预载的内存里取字节**，运行期不碰磁盘）。
    ///
    /// * `name` = 冷却键（同一类告警的节流按它算）；
    /// * `filename` = 语音包里的**文件主干**（`warn_gear` → `warn_gear.wav`）；
    /// * 包里没有这个文件（缺件/损坏被跳过）→ 去重警告一次并返回，**不影响别的告警**。
    fn play_wav(&mut self, name: &str, filename: &str) {
        if !self.enabled {
            return;
        }
        use rodio::Sink;
        use std::time::Duration;

        let cooldown_duration = Duration::from_secs(Self::get_cooldown_secs(name));

        if let Some(last_instant) = self.cooldowns.get(name) {
            if last_instant.elapsed() < cooldown_duration {
                return;
            }
        }

        self.cooldowns.insert(name.to_string(), Instant::now());

        // D：字节来自启动时预载的语音包（`preload`）——`Cursor` 包一层就能交给 rodio，
        // 所以**播放路径上没有任何 `File::open`**（目录删了/改名了也照常出声）。
        let bytes = pack().and_then(|p| p.get(filename));
        let Some(bytes) = bytes else {
            report_missing_voice(&format!(
                "{}/{}.wav",
                pack().map(|p| p.dir.clone()).unwrap_or_else(|| DEFAULT_VOICE_PATH.to_string()),
                filename
            ));
            return;
        };
        if let Ok(source) = rodio::Decoder::new(std::io::Cursor::new(bytes)) {
            if let Some(ref handle) = self._stream_handle {
                if let Ok(sink) = Sink::try_new(handle) {
                    sink.append(source);
                    self.sinks.insert(name.to_string(), sink);
                }
            }
        }
    }

    /// 合成机动告警音样本（1102.5Hz 正弦，40 采样/周期 @44.1k，整周期无缝）。
    /// `envelope` 为 true 时加 5ms/12ms 淡入淡出（短促 blip 防爆音）。
    fn synth_tone_samples(periods: usize, envelope: bool) -> Vec<f32> {
        const SAMPLES_PER_PERIOD: usize = 40; // 44100 / 1102.5 Hz
        let n = periods * SAMPLES_PER_PERIOD;
        let total_s = n as f64 / 44100.0;
        let mut data = Vec::with_capacity(n);
        for i in 0..n {
            let phase = 2.0 * std::f64::consts::PI * (i as f64) / SAMPLES_PER_PERIOD as f64;
            let mut s = (phase.sin() as f32) * 0.32;
            if envelope {
                let t = i as f64 / 44100.0;
                let a = (t / 0.005).min(1.0);
                let d = ((total_s - t) / 0.012).min(1.0);
                s *= (a * d) as f32;
            }
            data.push(s);
        }
        data
    }

    /// 机动告警蜂鸣（一声短促 blip）——独立于语音冷却体系
    pub fn play_maneuver_blip(&mut self) {
        if !self.enabled {
            return;
        }
        use rodio::Sink;
        let buf =
            rodio::buffer::SamplesBuffer::new(1, 44100, Self::synth_tone_samples(77, true));
        if let Some(ref handle) = self._stream_handle {
            if let Some(old) = self.sinks.remove("maneuver_blip") {
                old.stop();
            }
            if let Ok(sink) = Sink::try_new(handle) {
                sink.append(buf);
                self.sinks.insert("maneuver_blip".to_string(), sink);
            }
        }
    }

    /// 机动告警长鸣（整周期无缝循环），至 `stop_maneuver_solid`
    pub fn start_maneuver_solid(&mut self) {
        if !self.enabled {
            return;
        }
        use rodio::{Sink, Source};
        let buf = rodio::buffer::SamplesBuffer::new(
            1,
            44100,
            Self::synth_tone_samples(1000, false),
        );
        if let Some(ref handle) = self._stream_handle {
            if let Some(old) = self.sinks.remove("maneuver_solid") {
                old.stop();
            }
            if let Ok(sink) = Sink::try_new(handle) {
                sink.append(buf.repeat_infinite());
                self.sinks.insert("maneuver_solid".to_string(), sink);
            }
        }
    }

    pub fn stop_maneuver_solid(&mut self) {
        if let Some(sink) = self.sinks.remove("maneuver_solid") {
            sink.stop();
        }
    }

    /// Fire a warning voice and arm the **visual** alarm (blinking X).
    /// Danger classes: overspeed (IAS / Mach / gear / flaps), critical AoA,
    /// G-load limit exceedance and the altitude/terrain-proximity warning.
    fn trigger_danger(&mut self, name: &str, filename: &str) {
        self.alarm_active = true;
        self.play_wav(name, filename);
    }

    /// Fire a warning voice only — system / equipment cautions (overheat, RPM,
    /// fuel, descent rate, brake, ...) do NOT flash the X.
    fn trigger(&mut self, name: &str, filename: &str) {
        self.play_wav(name, filename);
    }

    /// Whether a voice alarm is currently active (warning condition fired this check).
    pub fn is_alarm_active(&self) -> bool {
        self.alarm_active
    }

    pub fn play_startup(&mut self) {
        // Startup chime only, not a warning alarm.
        self.play_wav("start1", "start1");
    }

    pub fn check_aoa(
        &mut self,
        aoa: f64,
        ias: f64,
        flaps: f64,
        no_flaps_aoa: f64,
        full_flaps_aoa: f64,
    ) {
        if ias > 80.0 {
            let aoa_max = if flaps > 0.5 {
                full_flaps_aoa
            } else if flaps > 0.0 {
                no_flaps_aoa + (full_flaps_aoa - no_flaps_aoa) * flaps * 2.0
            } else {
                no_flaps_aoa
            };
            if aoa > aoa_max - 1.0 {
                self.trigger_danger("aoa_crit", "aoaCrit");
            } else if aoa > aoa_max * 0.75 {
                self.trigger("aoa_high", "aoaHigh");
            }
        }
    }

    pub fn check_ias(&mut self, ias: f64, threshold: f64) {
        if ias >= threshold {
            self.trigger_danger("ias", "warn_ias");
        }
    }

    pub fn check_mach(&mut self, mach: f64, threshold: f64) {
        if mach >= threshold {
            self.trigger_danger("mach", "warn_mach");
        }
    }

    /// 起落架超速告警：gear > 0 且 `armed` 且表速超限。`armed` 由边沿判定给出：
    /// 下降沿（收回指令）清除并**锁存**（收回中途的同值帧不重新告警），
    /// 直到再次上升沿（重新放开）才恢复告警。
    pub fn check_gear(&mut self, gear: f64, ias: f64, threshold: f64, armed: bool) {
        if armed && gear > 0.0 && ias >= threshold {
            self.trigger_danger("gear", "warn_gear");
        }
    }

    pub fn check_flaps(&mut self, flaps: f64, flap_allow_angle: f64) {
        if flaps > 0.0 && flap_allow_angle - flaps < 2.0 {
            self.trigger_danger("flaps", "warn_flap");
        }
    }

    pub fn check_overheat(&mut self, temp_water: f64, _work_time: u64) {
        if temp_water > self.thresholds.engine_temp_warning {
            self.trigger("overheat", "warn_engineoverheat");
        }
    }

    /// 过载语音告警：|Ny| 达到允许过载的 90% 时告警（与 HUD 红色告警同一判据
    /// `display::updater::g_load_warning_level`，按当前重量换算允许过载）
    pub fn check_g_load(&mut self, ny: f64, neg_limit: f64, pos_limit: f64) {
        if crate::display::updater::g_load_warning_level(ny, neg_limit, pos_limit) > 0 {
            self.trigger_danger("g_load", "warn_loadfactor");
        }
    }

    pub fn check_low_fuel(&mut self, fuel_percent: f64) {
        if fuel_percent <= self.thresholds.low_fuel_percent {
            self.trigger("low_fuel", "warn_lowfuel");
        }
    }

    pub fn check_no_fuel(&mut self, fuel: f64) {
        if fuel == 0.0 {
            self.trigger("no_fuel", "fail_nofuel");
        }
    }

    pub fn check_engine_failure(&mut self, fuel_pressure: f64, throttle: f64) {
        if fuel_pressure == 0.0 && throttle > 20.0 {
            self.trigger("engine_fail", "fail_engine");
        }
    }

    pub fn check_vario(&mut self, vy: f64, gear: f64) {
        if gear >= 50.0 && vy <= self.thresholds.vario_warning {
            // 降落时下降率高：仅语音，不闪烁 X（驾驶舱常态场景，避免刷屏）
            self.trigger("vario", "warn_highvario");
        }
    }

    pub fn check_altitude(&mut self, vy: f64, height: f64, gear: f64) {
        if gear <= 0.0 && vy < -height / 10.0 {
            self.trigger_danger("altitude", "warn_altitude");
        }
    }

    pub fn check_brake(&mut self, gear: f64, airbrake: f64) {
        if gear < 100.0 && airbrake >= 90.0 {
            self.trigger("brake", "warn_brake");
        }
    }

    pub fn check_rpm(&mut self, rpm: f64, throttle: f64, rpm_min: f64, rpm_max: f64, rpm_max_allowed: f64) {
        if throttle > 30.0 && rpm < rpm_min {
            self.trigger("rpm_low", "warn_lowrpm");
        }
        if rpm > rpm_max && rpm > rpm_max_allowed * 0.99 {
            self.trigger("rpm_high", "warn_highrpm");
        }
    }

    /// Run the warning checks and refresh the visual alarm flag.
    ///
    /// The heavy checks run every `CHECK_INTERVAL_FRAMES` frames, but
    /// `display_data.voice_alarm` is refreshed on **every** call and this must
    /// run *before* `commit_frame()` so the HUD always renders current state.
    /// (Writing it only on check frames made the blinking X a stale one-frame
    /// strobe instead of a real flash.)
    pub fn check_all(
        &mut self,
        display_data: &mut crate::DisplayData,
        flight_model: &FlightModel,
        frame_count: u64,
    ) {
        if frame_count % CHECK_INTERVAL_FRAMES == 0 {
            self.run_checks(display_data, flight_model, frame_count);
        }

        // Visual alarm: flashing X follows the voice alarm. It stays active while
        // any warning voice fires and for ALARM_HOLD_FRAMES after the last one.
        display_data.voice_alarm = self.alarm_active || frame_count < self.alarm_until_frame;
    }

    fn run_checks(
        &mut self,
        display_data: &mut crate::DisplayData,
        flight_model: &FlightModel,
        frame_count: u64,
    ) {
        self.alarm_active = false;

        let wing_sweep = display_data.wing_sweep;
        let ias_warning = flight_model.aerodynamics.get_ias_warning_line(wing_sweep);
        let mach_warning = flight_model.aerodynamics.get_mach_warning_line(wing_sweep);
        let gear_warning = flight_model.gear_warning_speed();
        let flap_allow_angle = flight_model.flap_allow_angle(display_data.ias, false);
        let no_flaps_aoa = flight_model.aerodynamics.get_cl_aoa_max(0.0, wing_sweep).1;
        let full_flaps_aoa = flight_model.aerodynamics.get_cl_aoa_max(1.0, wing_sweep).1;

        self.check_ias(display_data.ias, ias_warning);
        self.check_mach(display_data.mach, mach_warning);

        self.check_aoa(
            display_data.aoa,
            display_data.ias,
            display_data.flaps,
            no_flaps_aoa,
            full_flaps_aoa,
        );
        self.check_gear(display_data.gear, display_data.ias, gear_warning, display_data.gear_armed);
        self.check_flaps(display_data.flaps, flap_allow_angle);
        self.check_brake(display_data.gear, display_data.airbrake);
        self.check_vario(display_data.vy, display_data.gear);
        self.check_altitude(display_data.vy, display_data.altitude, display_data.gear);
        debug_assert!(display_data.engine_count > 0, "engine_count must be > 0");
        self.check_low_fuel(display_data.fuel_percent);

        let rpm = display_data.engine_rpm[..display_data.engine_count].iter().copied().fold(0.0, f64::max);
        self.check_rpm(rpm, display_data.engine_throttle[0], flight_model.rpm_min, flight_model.rpm_max, flight_model.rpm_max_allowed);

        // 过载告警：按当前重量换算允许过载，|Ny| ≥ 90% 才告警
        let g_weight = flight_model.flight_weight(display_data.fuel_kg + display_data.fuel1_kg);
        let (g_neg, g_pos) = flight_model.allowed_load_factor(g_weight);
        self.check_g_load(display_data.ny, g_neg, g_pos);
        self.check_no_fuel(display_data.fuel_kg);
        let max_water = display_data.engine_temp_water[..display_data.engine_count]
            .iter()
            .copied()
            .fold(0.0, f64::max);
        self.check_overheat(max_water, 0);
        // `check_engine_failure` is intentionally not wired: the 8111 feed has
        // no fuel pressure data to feed it with.

        // Visual alarm: flashing X follows the voice alarm. It stays active while
        // any warning voice fires and for ALARM_HOLD_FRAMES after the last one.
        if self.alarm_active {
            self.alarm_until_frame = frame_count + ALARM_HOLD_FRAMES;
        }
    }
}

impl Default for VoiceWarning {
    fn default() -> Self {
        Self::new(true).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 危险类（闪烁 X）：超速族（表速/马赫/起落架/襟翼）+ 攻角临界 + 过载 + 高度（地形接近）
    #[test]
    fn danger_classes_arm_visual_alarm() {
        let mut vw = VoiceWarning::new(false).unwrap();
        assert!(!vw.is_alarm_active());
        vw.check_ias(2000.0, 1500.0);
        assert!(vw.is_alarm_active(), "表速超限应闪烁 X");

        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_mach(2.2, 2.0);
        assert!(vw.is_alarm_active(), "马赫超限应闪烁 X");

        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_gear(100.0, 800.0, 700.0, true);
        assert!(vw.is_alarm_active(), "起落架超速应闪烁 X");

        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_flaps(120.0, 118.0);
        assert!(vw.is_alarm_active(), "襟翼超速应闪烁 X");

        let mut vw = VoiceWarning::new(false).unwrap();
        // 13.0 ≥ 90%×14.0 = 12.6 → 告警；89% 不告警
        vw.check_g_load(13.0, -8.0, 14.0);
        assert!(vw.is_alarm_active(), "≥90% 允许过载应闪烁 X");
        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_g_load(12.5, -8.0, 14.0);
        assert!(!vw.is_alarm_active(), "<90% 允许过载不应告警");
        // 负方向按负限值判断
        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_g_load(-7.3, -8.0, 14.0);
        assert!(vw.is_alarm_active(), "负向 ≥90% 也应告警");

        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_aoa(37.5, 200.0, 0.0, 38.0, 37.0);
        assert!(vw.is_alarm_active(), "攻角临界应闪烁 X");

        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_altitude(-50.0, 300.0, 0.0);
        assert!(vw.is_alarm_active(), "高度告警应闪烁 X");
    }

    /// 起落架告警由边沿判定门控：收回（下降沿）不告警，上升/持平才告警
    #[test]
    fn gear_warn_gated_by_fall_edge() {
        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_gear(50.0, 800.0, 700.0, false);
        assert!(!vw.is_alarm_active(), "收回中（下降沿）不应告警");

        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_gear(50.0, 800.0, 700.0, true);
        assert!(vw.is_alarm_active(), "上升/持平 + 超速应告警");

        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_gear(50.0, 500.0, 700.0, true);
        assert!(!vw.is_alarm_active(), "未超速不应告警");

        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_gear(0.0, 800.0, 700.0, true);
        assert!(!vw.is_alarm_active(), "收起位不应告警");
    }

    /// 系统/设备类（仅语音，不闪烁 X）：油温、转速、燃油、失效、减速板、攻角偏高、下降率高（降落）
    #[test]
    fn system_cautions_do_not_arm_visual_alarm() {
        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_overheat(1000.0, 0);
        assert!(!vw.is_alarm_active(), "油温高不应闪烁 X");

        vw.check_rpm(5000.0, 80.0, 8500.0, 13000.0, 15000.0);
        assert!(!vw.is_alarm_active(), "转速低不应闪烁 X");

        vw.check_low_fuel(5.0);
        assert!(!vw.is_alarm_active(), "低燃油不应闪烁 X");

        vw.check_no_fuel(0.0);
        assert!(!vw.is_alarm_active(), "无燃油不应闪烁 X");

        vw.check_engine_failure(0.0, 50.0);
        assert!(!vw.is_alarm_active(), "发动机失效不应闪烁 X");

        vw.check_brake(0.0, 100.0);
        assert!(!vw.is_alarm_active(), "减速板不应闪烁 X");

        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_vario(-10.0, 100.0);
        assert!(!vw.is_alarm_active(), "下降率高（降落）不应闪烁 X");

        let mut vw = VoiceWarning::new(false).unwrap();
        vw.check_aoa(32.0, 200.0, 0.0, 38.0, 37.0);
        assert!(!vw.is_alarm_active(), "攻角偏高（未到临界）不应闪烁 X");
    }

    // ---------------------------------------------------------------- D：语音包与预载 --

    /// 造一个最小但**合法**的 WAV（RIFF/WAVE 头 + 一小段 PCM 数据）。
    fn fake_wav(samples: usize) -> Vec<u8> {
        let data_len = samples * 2; // 16 bit mono
        let mut v = Vec::with_capacity(44 + data_len);
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());       // fmt chunk size
        v.extend_from_slice(&1u16.to_le_bytes());        // PCM
        v.extend_from_slice(&1u16.to_le_bytes());        // mono
        v.extend_from_slice(&44100u32.to_le_bytes());    // sample rate
        v.extend_from_slice(&88200u32.to_le_bytes());    // byte rate
        v.extend_from_slice(&2u16.to_le_bytes());        // block align
        v.extend_from_slice(&16u16.to_le_bytes());       // bits
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data_len as u32).to_le_bytes());
        v.extend(std::iter::repeat(0u8).take(data_len));
        v
    }

    /// 语音文件清单：**从磁盘上真实的语音包目录**数出来的（`resource/voice/<包>`）。
    #[test]
    fn voice_pack_dir_has_the_wavs_the_code_plays() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let dir = root.join(DEFAULT_VOICE_PATH);
        if !dir.is_dir() {
            eprintln!("跳过：{} 不存在（语音资源不入库时正常）", dir.display());
            return;
        }
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("wav")).unwrap_or(false))
            .map(|p| p.file_stem().unwrap().to_string_lossy().to_string())
            .collect();
        names.sort();
        let total: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().map(|x| x.eq_ignore_ascii_case("wav")).unwrap_or(false))
            .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
            .sum();
        eprintln!("语音包 {}：{} 个 wav，共 {} 字节", DEFAULT_VOICE_PATH, names.len(), total);
        for need in REQUIRED_VOICE {
            assert!(names.iter().any(|n| n == need),
                    "语音包里没有 {need}.wav（代码会点播它）—— 实际有 {names:?}");
        }
        // 每个都要真的是 RIFF/WAVE（把"下坏了的文件"挡在预载之外）
        for n in &names {
            let b = std::fs::read(dir.join(format!("{n}.wav"))).unwrap();
            assert!(is_riff_wave(&b), "{n}.wav 不是 RIFF/WAVE（{} 字节）", b.len());
        }
    }

    /// **预载后不依赖磁盘**：把文件读进内存 → 删掉整个目录 → 字节仍在、仍能交给解码器。
    ///
    /// 这就是"运行期零磁盘 I/O"的直接证据（播放路径上只剩 `Cursor<&'static [u8]>`）。
    #[test]
    fn preloaded_pack_survives_deleting_the_directory() {
        let dir = std::env::temp_dir().join("wp8f_voice_preload_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("warn_gear.wav"), fake_wav(64)).unwrap();
        std::fs::write(dir.join("start1.wav"), fake_wav(32)).unwrap();
        // 一个坏文件：必须只被跳过 + 记进 skipped，不影响整包
        std::fs::write(dir.join("broken.wav"), b"not a wav at all").unwrap();
        let total: usize = fake_wav(64).len() + fake_wav(32).len();

        let p = preload(&dir.display().to_string());
        assert_eq!(p.count, 2, "两个合法 wav 应当都进包：{:?}", p.names());
        assert_eq!(p.total_bytes, total, "总字节应当等于两个 wav 的大小");
        assert!(p.has("warn_gear") && p.has("start1"));
        assert_eq!(p.skipped.len(), 1, "坏文件应当被记下来：{:?}", p.skipped);
        assert!(p.skipped[0].0 == "broken", "坏文件名字不对：{:?}", p.skipped);

        // 删掉整个目录：内存里的字节不受影响（**这是"不依赖磁盘"的证据**）
        std::fs::remove_dir_all(&dir).unwrap();
        let p = pack().expect("预载是常驻的");
        let b = p.get("warn_gear").expect("删目录后仍要拿得到字节");
        assert!(is_riff_wave(b), "删目录后拿到的字节应当还是完整的 wav");
        // 且还能真的构造出解码器（说明交给 rodio 的那条路是通的）
        assert!(rodio::Decoder::new(std::io::Cursor::new(b)).is_ok(),
                "预载的字节应当能被 rodio 解码");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 目录解析：找不到时返回 `Err`（不是 panic、不是空包）；空值也算 `Err`。
    #[test]
    fn voice_dir_resolution_errors_instead_of_panicking() {
        assert!(resolve_dir("resource/voice/__no_such_pack__").is_err());
        assert!(resolve_dir("").is_err());
        // 仓库里那份默认语音包必须解析得到（cwd=core/ 时靠 exe/上一级与测试基准）
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        if root.join(DEFAULT_VOICE_PATH).is_dir() {
            assert!(resolve_dir(DEFAULT_VOICE_PATH).is_ok(), "默认语音包应当能解析到");
        }
    }
}
