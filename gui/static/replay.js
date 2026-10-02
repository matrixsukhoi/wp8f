/* wp8f 控制台前端 —— 回放页签
 * 零构建 classic script：共享全局作用域，依赖 app.js 先加载
 *（`$` / `api` / `esc` / `report` / `notify` / `t` / `seriesColor` / `GRID3D_BASE` / `AXIS_STYLE` /
 *  `chartPlaceholder` / `withTheme` / `echartsGlOk`）。
 *
 * 文案全部走 `t()`（键前缀 `rp.`，另复用 `common.not_loaded`）；语言切换由 `app.js` 的
 * `setUiLang()` 经 `rpOnLangChange()` 重画。
 *
 * ⚠️ 本文件里不许有叫 `t` 的局部变量/形参（`t` 是 i18n 查表函数，一撞就是
 * `t is not a function`、整块渲染白屏）。
 *
 * ⚠️ 两处**故意不翻译**（探针按它做机器判据，不是给人看的文案）：
 * `rpGroundWhy()` 返回的 `rp.groundStats.reason`（`ui_checks` 断言其中的"没带底图"），
 * 以及 `rp.groundStats` 的 `mode`（`texture` / `grid`）。
 */
/* ---------- Replay 回放（ECharts GL 3D 轨迹 + 时间轴） ---------- */
const rp = {
  files: [],       // 已加入的回放：[{name, objects, t0, t1}]
  objects: [],     // 合并视图：每个对象带 file / color 序号（同坐标系叠加）
  t0: 0, t1: 0, t: 0, playing: false, timer: null, speed: 1,
  chart: null, gl: false, lineData: [], lineTime: [],
  ref: null,       // 经纬度原点（第一帧）→ 平面 km 坐标
  bbox: null,      // 全程范围（km / m）：{x0,x1,y0,y1,z0,z1}
  scaleKm: 50,     // 视野边长 km；0 = 全程自适应
  axis: null,      // 上次写入的坐标轴范围（未明显移动就不重排，避免视角抖动）
  arrowKm: 5,      // 速度矢量箭头长度（km，按视野取比例 → 视觉长度恒定）
  modelH: 0.25,    // 飞机模型水平单位（km）：水平方向按视野取比例
  modelV: 30,      // 飞机模型垂直单位（m）：垂直方向按高度轴跨度取比例
  planar: false,   // `.wpr` 记录：第 2/3 槽已是"相对首帧的 km"，不做经纬度换算
  mapFile: null,   // 提供底图的记录文件名（底图字节走 /api/replay/map）
  map: null,       // `.wpr` 的地图元数据：km 矩形 + 底图像素尺寸 + MIME
  groundOn: true,      // 回放页签的「xoy 底面」开关（关掉 = 只留 z=0 网格线）
  groundKey: "",       // 当前底面的缓存键（覆盖范围/底图）：没变就不重建
  groundSeries: null,  // 当前底面 series（surface 底图 / line3D 网格线）
  groundSrc: null,     // 底图：{url, ready, pending, w, h, file, err}（只做原图纹理，不解码像素）
  groundFrom: null,    // 当前底图来自哪一份记录（本记录内嵌；不带底图的记录不铺底面）
  groundStats: null,   // {mode, points, cols, rows, srcPx, pxM, coverKm, texels, ms, imageFrom}（探针读它）
};
const RP_TICK_MS = 50;   // 回放 tick 周期：播放推进与 setInterval 必须同源
const RP_MODEL_MAX = 4;   // 姿态模型/速度矢量最多绘制前 N 个对象（series 数量控制）

/* ---------- 飞机标记配色：**只有亮黄一种** ----------
 * 位置点 + 姿态线框（含机头→尾翼那条姿态连线）统一亮黄、不垫深色描边（描边看起来就是"黑线"），
 * 亮度靠更粗的主体线 + 更大的点保证。
 * 轨迹线 / 文件标签圆点 / 右侧遥测左边框仍是各对象的颜色（`seriesColor(i)`），多对象靠它区分；
 * 速度矢量箭头保持对象色（它属于该对象的轨迹方向）。 */
const RP_MARKER_COLOR = "#ffe600";   // 亮黄：当前位置点 + 姿态线框（整张图上唯一的标记色）
const RP_MARKER_MAIN_W = 2.4;        // 姿态线框线宽
const RP_MARKER_ID = "rp-marker";    // 姿态线框 series 的稳定 id（探针按 id 取，不按会翻译的 name）
const RP_MARKER_SYMBOL = 8;          // 当前位置点的符号尺寸

/* ---------- 轨迹 = 原始采样点 ----------
 * **不做任何平滑/重采样**：`lineData` 就是原始帧的 km 坐标，只按点数上限抽稀 ——
 * 插值只会掩盖记录间隔的真实观感。速度矢量取过去两点之差（见 `rpTangentAt`）。
 * 位置密不密由记录侧决定：`map_obj_record_every_frames` 调到 1–2 就是每帧都有真实位置。
 */

/* ---------- frames 的列下标：**由 `/api/replay/load` 的 `columns` 决定** ----------
 * 服务端（`gui/src/record.rs::FRAME_COLUMNS`）是唯一来源；缺 `columns` 说明契约变了 ——
 * 猜一份旧列序只会把字段静默读错位，所以直接报错。 */
let FC = {};                       // 列名 → 下标
function rpSetColumns(cols) {
  if (!Array.isArray(cols) || !cols.length) throw new Error("replay: 响应缺少 columns");
  FC = {};
  for (let i = 0; i < cols.length; i++) FC[cols[i]] = i;
}

/** 整条轨迹的时间二分：返回满足 `frames[i][0] <= time` 的最大下标（早于首帧 → 0）。
 *  ⚠️ 形参不许叫 `t`（`t` 是 i18n 查表函数，名字一撞就 `t is not a function`、整块白屏）。 */
function rpTimeIndex(frames, time) {
  let lo = 0, hi = frames.length - 1, i = 0;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (frames[mid][FC.t_ms] <= time) { i = mid; lo = mid + 1; } else { hi = mid - 1; }
  }
  return i;
}

/** 当前位置的**速度方向**（速度矢量箭头用）：过去两点之差 `P(k) − P(k−1)`（原始采样，
 *  不平滑也不做中心差分）。首帧、或相邻点重合时返回 null，由调用方退回机体轴。 */
function rpTangentAt(o, time) {
  const fr = (o && o.frames) || [];
  if (fr.length < 2) return null;
  const k = rpTimeIndex(fr, time);
  const a = fr[Math.max(0, k - 1)];
  const b = fr[k];
  const d = [b[FC.east_km] - a[FC.east_km], b[FC.north_km] - a[FC.north_km],
             (b[FC.alt_m] - a[FC.alt_m]) / 1000];
  if (Math.hypot(d[0], d[1], d[2]) < 1e-9) return null;
  return d;
}


/** 参考原点：取第一个有帧的对象的首帧（`.wpr` 平面模式下用不到，但保持一致 */
function rpUpdRef() {
  const o = rp.objects.find((x) => x.frames && x.frames.length);
  rp.ref = o ? { lon: o.frames[0][FC.east_km], lat: o.frames[0][FC.north_km] } : null;
}
/** 经纬度 → 局部平面 km 偏移（原点=第一帧；等距近似，几十 km 尺度足够）。
 *  `.wpr` 回放里第 2/3 槽本来就是"相对首帧的 km"（后端 planar=true），原样返回。 */
function ll2km(lon, lat) {
  if (rp.planar) return [lon, lat];
  if (!rp.ref) return [0, 0];
  const kx = 111.320 * Math.cos(rp.ref.lat * Math.PI / 180);
  return [(lon - rp.ref.lon) * kx, (lat - rp.ref.lat) * 110.574];
}

/** 叉乘 / 归一化（姿态基向量用） */
function rpCross(a, b) {
  return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
}
function rpNorm(v) {
  const n = Math.hypot(v[0], v[1], v[2]) || 1;
  return [v[0] / n, v[1] / n, v[2] / n];
}

/** 姿态基向量：F=机头，R=右翼，U=机背（世界系 x=东 y=北 z=上，单位向量；滚转正=右翼下沉） */
function rpBasis(rollDeg, pitchDeg, yawDeg) {
  const r = (Number(rollDeg) || 0) * Math.PI / 180;
  const p = (Number(pitchDeg) || 0) * Math.PI / 180;
  const y = (Number(yawDeg) || 0) * Math.PI / 180;
  const cp = Math.cos(p), sp = Math.sin(p), cy = Math.cos(y), sy = Math.sin(y);
  const F = [sy * cp, cy * cp, sp];                       // 航向 0°=正北，顺时针
  let R0 = [cy, -sy, 0];                                  // 水平右向量（做成正交）
  const d = R0[0] * F[0] + R0[1] * F[1] + R0[2] * F[2];
  R0 = rpNorm([R0[0] - d * F[0], R0[1] - d * F[1], R0[2] - d * F[2]]);
  const U0 = rpCross(R0, F);
  const cr = Math.cos(r), sr = Math.sin(r);
  const R = [R0[0] * cr - U0[0] * sr, R0[1] * cr - U0[1] * sr, R0[2] * cr - U0[2] * sr];
  const U = [R0[0] * sr + U0[0] * cr, R0[1] * sr + U0[1] * cr, R0[2] * sr + U0[2] * cr];
  return { F, R, U };
}

/** 机身顶点：a=前 / b=右 / c=上（单位化机体坐标）
 *  水平（东西/南北，轴单位 km）乘 hKm；垂直（高度，轴单位 m）乘 vM ——
 *  两个方向各自按"所在坐标轴的跨度"取比例，屏幕上形状才不会被 z 轴的量纲拉变形。 */
function rpBodyPt(P, B, hKm, vM, a, b, c) {
  const vx = B.F[0] * a + B.R[0] * b + B.U[0] * c;
  const vy = B.F[1] * a + B.R[1] * b + B.U[1] * c;
  const vz = B.F[2] * a + B.R[2] * b + B.U[2] * c;
  return [P[0] + vx * hKm, P[1] + vy * hKm, P[2] + vz * vM];
}

/** 简易飞机线框：俯视菱形轮廓 + 垂直尾翼 + **机头→水平尾翼顶端连线**
 *  （最后那条让姿态一眼可辨：连线朝上=抬头、倾斜=滚转，2 条折线变 3 条） */
function rpModelPolys(P, B, hKm, vM) {
  const pt = (a, b, c) => rpBodyPt(P, B, hKm, vM, a, b, c);
  const N = pt(3.2, 0, 0), WR = pt(-0.2, 2.0, 0), T = pt(-2.8, 0, 0), WL = pt(-0.2, -2.0, 0);
  return [
    [N, WR, T, WL, N],                    // 机翼 + 机身（闭合俯视轮廓）
    [T, pt(-2.6, 0, 1.4)],                // 垂尾
    [N, pt(-2.6, 1.1, 0.35)],             // 机头 → 水平尾翼顶端（姿态连线）
    [pt(-2.6, 1.1, 0), pt(-2.6, -1.1, 0)], // 水平尾翼
  ];
}

/** 速度矢量：**水平面内的 2D 箭头**（杆 + 两片倒刺，都画在当前高度上）。
 *  杆从模型外侧起画（留 33% 空隙（要盖过机身半长）），否则会盖住飞机模型。 */
function rpArrowPolys(P, dir, lenKm) {
  const h = Math.hypot(dir[0], dir[1]) || 1;
  const ux = dir[0] / h, uy = dir[1] / h;        // 地面航迹方向（速度矢量的水平投影）
  const tip = [P[0] + ux * lenKm, P[1] + uy * lenKm, P[2]];
  const gap = lenKm * 0.33;                      // 空隙要盖过机身半长，否则箭杆压住机头
  const tail = [P[0] + ux * gap, P[1] + uy * gap, P[2]];
  const px = -uy, py = ux;                       // 平面内法向
  const back = lenKm * 0.25, w = lenKm * 0.12;
  const barb = (s) => [tip[0] - ux * back + px * w * s, tip[1] - uy * back + py * w * s, P[2]];
  return [
    [tail, tip],
    [barb(1), tip, barb(-1)],
  ];
}

/** 箭头/模型尺度：按当前视野（水平 km）与高度跨度（垂直 m）分别取比例，
 *  缩放视野时视觉大小恒定，且水平/垂直各自贴合坐标轴比例。 */
function rpSizes() {
  const b = rp.bbox;
  const viewKm = rp.scaleKm || (b ? Math.max(b.x1 - b.x0, b.y1 - b.y0) : 50) || 50;
  const altSpan = b ? Math.max(b.z1 - b.z0, 300) : 2000;
  rp.arrowKm = Math.max(0.2, viewKm * 0.10);     // 箭头 ≈ 视野的 10%
  rp.modelH = Math.max(0.02, viewKm * 0.008);    // 机身长 6.0 单位 ≈ 视野的 4.8%（比当前点标记略大才看得清）
  rp.modelV = Math.max(4, altSpan * 0.020);      // 垂尾高 1.4 单位 ≈ 高度轴的 2.8%（不再竖着"戳"出来）
}

/** 坐标轴范围：跟随当前点（固定边长）或全程自适应 */
function rpAxisOption(cur) {
  const b = rp.bbox;
  let x0, x1, y0, y1;
  if (!rp.scaleKm && b) {
    const pad = Math.max((b.x1 - b.x0) * 0.06, 0.4);
    x0 = b.x0 - pad; x1 = b.x1 + pad;
    const pady = Math.max((b.y1 - b.y0) * 0.06, 0.4);
    y0 = b.y0 - pady; y1 = b.y1 + pady;
  } else {
    const half = Math.max(rp.scaleKm, 2) / 2;
    const c = cur || [0, 0];
    x0 = c[0] - half; x1 = c[0] + half;
    y0 = c[1] - half; y1 = c[1] + half;
  }
  if (x1 - x0 < 0.5) { const c = (x0 + x1) / 2; x0 = c - 0.25; x1 = c + 0.25; }
  if (y1 - y0 < 0.5) { const c = (y0 + y1) / 2; y0 = c - 0.25; y1 = c + 0.25; }
  // 高度轴始终按全程（含 300 m 余量，至少 800 m 跨度）；**下沿必须含 0** —— xoy 底面
  // （底图/网格线，z=0）画在那里，轴不含 0 时底面会落到盒子外面去
  let z0 = b ? Math.min(0, Math.min(b.z0, b.z1) - 300) : 0;
  let z1 = b ? Math.max(b.z0, b.z1) + 300 : 1000;
  if (z1 - z0 < 800) { const c = (z0 + z1) / 2; z0 = c - 400; z1 = c + 400; }
  // 3D 盒子的宽深比跟随 km 跨度 → 俯视时不被拉伸
  const boxWidth = 150;
  const boxDepth = Math.max(45, Math.min(230, boxWidth * ((y1 - y0) / (x1 - x0))));
  // 轴上的数字只按"好看步长"的位数显示（见 rpAxisDisplay）：范围本身仍是上面的原值 ——
  // 当前点位置是任意浮点数，轴范围 = 中心 ± 视野/2，不取整的话轴上会印出 43.674941952
  // 这种 9 位小数（用户实测反馈）。步长用与 echarts-gl 同一个 1/2/5×10ⁿ 族。
  const xStep = rpNiceStep(x1 - x0, 5);
  const yStep = rpNiceStep(y1 - y0, 5);
  const zStep = rpNiceStep(z1 - z0, 4);
  return {
    xAxis3D: Object.assign({ type: "value", name: t("rp.ax.ew"), min: x0, max: x1,
      interval: xStep, axisLabel: rpAxisDisplay(xStep) }, AXIS_STYLE),
    yAxis3D: Object.assign({ type: "value", name: t("rp.ax.ns"), min: y0, max: y1,
      interval: yStep, axisLabel: rpAxisDisplay(yStep) }, AXIS_STYLE),
    zAxis3D: Object.assign({ type: "value", name: t("rp.ax.alt"), min: z0, max: z1,
      interval: zStep, axisLabel: rpAxisDisplay(zStep) }, AXIS_STYLE),
    grid3D: Object.assign({}, GRID3D_BASE, { boxWidth, boxDepth }),
    _rng: [x0, x1, y0, y1],
  };
}

/** 当前点明显移动 / 换尺度时才重写坐标轴（每帧重排会让 3D 视角抖动、且开销大） */
function rpAxisNeeded(cur) {
  if (!rp.axis) return true;
  if (rp.scaleKm === 0) return false;               // 全程模式固定
  const span = Math.max(0.5, rp.axis[1] - rp.axis[0]);
  const cx = (rp.axis[0] + rp.axis[1]) / 2;
  const cy = (rp.axis[2] + rp.axis[3]) / 2;
  return Math.hypot(cur[0] - cx, cur[1] - cy) > span * 0.05;
}


/** 尾迹点集：lineData 中时间 ≤ time 的点（**返回新数组**，按 lineTime 二分截断）。
 *  源必须是 lineData（全量轨迹）——曾误用恒空的 trailPts 占位数组，
 *  导致每 tick 尾迹只有 1 个点、line3D 单点不渲染（轨迹不显示的根因）。 */
function rpTrailAt(i, time) {
  const times = rp.lineTime[i] || [];
  const src = rp.lineData[i] || [];
  let k = -1, lo = 0, hi = times.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (times[mid] <= time) { k = mid; lo = mid + 1; } else { hi = mid - 1; }
  }
  return src.slice(0, Math.max(k + 1, 0));
}

/** 录制文件列表 → 下拉菜单（选择即加入回放） */
async function loadReplayFiles() {
  const sel = $("#rp-add");
  try {
    const items = await api("/api/replay/list");
    const added = new Set(rp.files.map((f) => f.name));
    const opts = [`<option value="">${esc(t("rp.select_record"))}</option>`];
    items.forEach((it) => {
      const kb = (it.size / 1024).toFixed(1);
      const when = new Date(it.mtime * 1000).toLocaleString();
      const mark = added.has(it.name) ? "✓ " : "";
      // 接口只列 `.wpr` 飞行记录（旧 .acmi/.csv 已不再收录），不必再按 kind/扩展名分类型
      const o = document.createElement("option");
      o.value = it.name;
      o.textContent = `${mark}${it.name}   (${kb} KB · ${when})`;
      if (added.has(it.name)) o.disabled = true;   // 已加入的不再重复选
      opts.push(o);
    });
    sel.replaceChildren(...opts.map((html) => {
      if (typeof html === "string") {
        // ⚠️ 这个局部量**不许叫 `t`**：`t` 是 i18n 查表函数（同 rpTimeIndex 的注释）
        const tpl = document.createElement("template");
        tpl.innerHTML = html.trim();
        return tpl.content.firstElementChild;
      }
      return html;
    }));
    sel.title = items.length ? t("rp.list.count", { n: items.length }) : t("rp.list.empty");
    if (!items.length) {
      const o = document.createElement("option");
      o.value = ""; o.textContent = t("rp.list.empty_option"); o.disabled = true;
      sel.appendChild(o);
    }
  } catch (e) {
    report(t("rp.err.list", { err: e }), false);
  }
}

/** 已加入文件标签（带颜色圆点 + 单独移除） */
function renderReplayChips() {
  const box = $("#rp-files");
  if (!rp.files.length) {
    box.innerHTML = `<span class="muted small">${esc(t("rp.no_files"))}</span>`;
    return;
  }
  box.innerHTML = rp.files.map((f, i) => {
    const dots = (f.objects || []).slice(0, 6).map((_o, k) =>
      `<span class="dot" style="background:${seriesColor(f.colorBase + k)}"></span>`).join("");
    return `<span class="chip" title="${esc(f.name)}">
      <span class="dots">${dots}</span>
      <span class="nm">${esc(f.name)}</span>
      <button class="x" data-rm="${esc(f.name)}" title="${esc(t("rp.chip.remove"))}">✕</button>
    </span>`;
  }).join("");
  box.querySelectorAll("button[data-rm]").forEach((b) => {
    b.addEventListener("click", () => removeReplayFile(b.dataset.rm));
  });
}

function ensureReplayChart() {
  if (rp.chart && rp.chart.isDisposed()) rp.chart = null;
  if (rp.chart) return rp.gl;
  if (!window.echarts) {
    chartPlaceholder(document.getElementById("rp-chart"), t("rp.err.no_echarts"));
    return false;
  }
  try {
    rp.chart = echarts.init(document.getElementById("rp-chart")); rp.chart.setOption(withTheme({}));
  } catch (e) {
    rp.chart = null;
    return false;
  }
  rp.gl = echartsGlOk();
  if (!rp.gl) {
    rp.chart.setOption({
      title: {
        text: t("rp.err.no_webgl"),
        left: "center", top: "middle",
        textStyle: { fontSize: 14, color: "#8c8c86" },
      },
    });
  }
  return rp.gl;
}

/** 线性插值取 time 时刻的帧（位置/姿态连续）：夹在 [i, i+1] 之间按时间比例 lerp。 */
function frameAt2(frames, time) {
  if (!frames.length) return null;
  let lo = 0, hi = frames.length - 1, ans = 0;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (frames[mid][FC.t_ms] <= time) { ans = mid; lo = mid + 1; } else { hi = mid - 1; }
  }
  const f0 = frames[ans];
  const f1 = frames[ans + 1];
  if (!f1 || time <= f0[0]) return f0;
  if (time >= f1[0]) return f1;
  const span = f1[0] - f0[0];
  if (span <= 0) return f0;
  const k = (time - f0[0]) / span;
  const out = new Array(12);
  out[0] = time;
  for (let j = 1; j < 12; j++) {
    const a = f0[j], b = f1[j];
    out[j] = Number.isFinite(a) && Number.isFinite(b) ? a + (b - a) * k : (a ?? b ?? 0);
  }
  return out;
}

/** mm:ss.mmm 时间格式（当前时间 / 总时长共用）。 */
function fmtMs(ms) {
  const mm = String(Math.floor(ms / 60000)).padStart(2, "0");
  const ss = String(Math.floor((ms % 60000) / 1000)).padStart(2, "0");
  const mmm = String(Math.floor(ms % 1000)).padStart(3, "0");
  return `${mm}:${ss}.${mmm}`;
}

/** 加入一个回放文件（可多次，同坐标系叠加） */
async function addReplayFile(name) {
  if (!name || rp.files.some((f) => f.name === name)) return;
  try {
    const d = await api(`/api/replay/load?file=${encodeURIComponent(name)}`);
    rpSetColumns(d.columns);          // frames 的列序由服务端给
    const objects = (d.objects || []).filter((o) => o.frames && o.frames.length);
    if (!objects.length) {
      notify(t("rp.err.no_tracks", { name }), false, "replay");
      return;
    }
    rp.files.push({
      name,
      objects,
      t0: d.t0 || 0,
      t1: d.t1 || 0,
      // `.wpr` 记录：坐标是"相对首帧的 km"（planar），并自带底图与地图矩形
      planar: !!d.planar,
      map: d.map || null,
      colorBase: rp.files.reduce((n, f) => n + (f.objects || []).length, 0),
    });
    rebuildReplay();
    setMsg(t("rp.added_n", { name, n: rp.files.length }));
    notify(t("rp.added", { name }), true, "replay");
    loadReplayFiles();   // 下拉里把已加入的标灰
  } catch (e) {
    report(t("rp.err.load", { err: e }), false, "replay");
  }
}

function removeReplayFile(name) {
  const i = rp.files.findIndex((f) => f.name === name);
  if (i < 0) return;
  rp.files.splice(i, 1);
  // 重新分配颜色基数（移除后颜色不跳号）
  let base = 0;
  rp.files.forEach((f) => { f.colorBase = base; base += (f.objects || []).length; });
  rebuildReplay();
  report(rp.files.length ? t("rp.removed_n", { name, n: rp.files.length }) : t("rp.removed", { name }),
         true, "replay");
  loadReplayFiles();
}

/** 复位回放视图：图表 + 右侧遥测 + 统计/时间轴文字。
 *  「删掉最后一个文件」与「清空」共用同一条清理路径 —— 否则 3D 图和右侧遥测会继续
 *  空集时也要清干净，不能留着上一条记录的轨迹。 */
function resetReplayView() {
  rpStop();
  rp.objects = [];
  rp.lineData = [];
  rp.lineTime = [];
  rp.t0 = rp.t1 = rp.t = 0;
  rp.ref = null;
  rp.bbox = null;
  rp.axis = null;
  if (rp.chart && !rp.chart.isDisposed()) rp.chart.clear();
  rp.chart = null;   // 容器随即被占位层接管（会 dispose 实例），旧引用必须丢弃
  rp.gl = false;
  rp.planar = false;
  rp.map = null;
  rp.mapFile = null;
  rp.groundSrc = null;
  rpGroundReset();     // 底面缓存（series/覆盖区/统计）一起清掉
  rpGroundNoteClear(); // 底图来源提示（没底图/借底图那行字）也一起清掉
  $("#rp-objects").textContent = t("common.not_loaded");
  $("#rp-total").textContent = "00:00.000";
  $("#rp-clock").textContent = "00:00.000";
  $("#rp-time").max = 1;
  $("#rp-time").value = 0;
  $("#rp-telemetry").textContent = t("rp.no_replay");
  chartPlaceholder(document.getElementById("rp-chart"), t("rp.no_files"));
}

function clearReplayFiles() {
  rp.files = [];
  resetReplayView();
  renderReplayChips();
  loadReplayFiles();
  report(t("rp.cleared"), true, "replay");
}

/** 记录上下文同步（坐标系模式 + 底图来源）：`ll2km` 靠 `rp.planar` 决定原样返回还是按经纬度
 *  换算，所以**必须在建轨迹（lineData）/算当前位置之前调用**，否则平面记录的轨迹会被当成
 *  经纬度做等距换算（×111 km/°）而飞出视野。
 *  多文件混载时：任一为平面即按平面处理，底图取第一个带地图元数据的文件。 */
function rpSyncContext() {
  const planar = rp.files.some((f) => f.planar);
  const mf = rp.files.find((f) => f.map) || null;
  if (planar !== rp.planar || (mf && mf.name) !== rp.mapFile) {
    rp.planar = planar;
    rp.map = mf ? mf.map : null;
    rp.mapFile = mf ? mf.name : null;
    rp.groundSrc = null;        // 换记录 → 底图像素与底面都要重来
    rpGroundReset();
    rpGroundLoad();             // 异步解码新底图（到位后 rpGroundApply 自己重铺）
    rpGroundNote();             // "没带底图 / 借了哪一份"的来源提示（提示行立刻反映当前记录）
  }
}

/** 合并所有文件 → 同坐标系重绘（轨迹/箭杆/当前点 series 全部重建） */
function rebuildReplay() {
  rpStop();
  // 合并对象：文件内对象顺序保持，颜色按全局序号分配
  rp.objects = [];
  rp.files.forEach((f) => {
    f.objects.forEach((o) => {
      rp.objects.push(Object.assign({}, o, {
        name: rp.files.length > 1 ? `${f.name} › ${o.name}` : o.name,
        file: f.name,
      }));
    });
  });
  // 空集（删掉最后一个文件 / 已无任何可用轨迹）：走与「清空」相同的清理路径，
  // 而不是把上一次的轨迹和遥测留在屏幕上
  if (!rp.objects.length) {
    resetReplayView();
    renderReplayChips();
    return;
  }
  rp.t0 = Math.min(...rp.files.map((f) => f.t0));
  rp.t1 = Math.max(...rp.files.map((f) => f.t1));
  if (!Number.isFinite(rp.t0)) rp.t0 = 0;
  if (!Number.isFinite(rp.t1)) rp.t1 = 0;
  rp.t = rp.t0;
  const dur = Math.max(1, rp.t1 - rp.t0);
  $("#rp-time").max = dur;
  $("#rp-time").value = 0;
  $("#rp-total").textContent = fmtMs(dur);
  // 轨迹**直接画原始采样点**（不做任何平滑/重采样：整套算法已按用户要求删除）。
  // 记录密不密由记录侧决定（`map_obj_record_every_frames`）；这里只按点数上限抽稀。
  const durTxt = t("rp.summary", {
    n: rp.files.length, o: rp.objects.length, s: (dur / 1000).toFixed(1),
  });
  $("#rp-objects").textContent = durTxt;
  renderReplayChips();

  // 轨迹一次性构建（大录制降采样到 ≤2500 点/对象）；坐标 = 世界 km（`rpSyncContext` 已定好
  // 坐标系模式：平面记录原样返回，旧 ACMI 才走经纬度换算）
  rpSyncContext();     // **必须在 ll2km / 建 lineData 之前**（见 rpSyncContext 的注释）
  rpUpdRef();
  rp.lineData = [];
  rp.lineTime = [];
  rp.bbox = null;
  rp.objects.forEach((o) => {
    const step = Math.max(1, Math.ceil(o.frames.length / 2500));
    const pts = [];
    const times = [];
    let lastIdx = -1;
    const push = (f) => {
      const km = ll2km(f[FC.east_km], f[FC.north_km]);
      pts.push([km[0], km[1], f[FC.alt_m]]);
      times.push(f[FC.t_ms]);
      if (!rp.bbox) rp.bbox = { x0: km[0], x1: km[0], y0: km[1], y1: km[1],
                                z0: f[FC.alt_m], z1: f[FC.alt_m] };
      const b = rp.bbox;
      if (km[0] < b.x0) b.x0 = km[0];
      if (km[0] > b.x1) b.x1 = km[0];
      if (km[1] < b.y0) b.y0 = km[1];
      if (km[1] > b.y1) b.y1 = km[1];
      if (f[FC.alt_m] < b.z0) b.z0 = f[FC.alt_m];
      if (f[FC.alt_m] > b.z1) b.z1 = f[FC.alt_m];
    };
    for (let k = 0; k < o.frames.length; k += step) {
      push(o.frames[k]);
      lastIdx = k;
    }
    if (lastIdx !== o.frames.length - 1) push(o.frames[o.frames.length - 1]);
    rp.lineData.push(pts);
    rp.lineTime.push(times);
  });

  // 箭头/模型按视野与高度跨度取比例 → 缩放尺度时视觉大小恒定
  rpSizes();
  rp.axis = null;   // 强制重写坐标轴

  if (ensureReplayChart() && rp.objects.length) {
    const lineSeries = rp.objects.map((o, i) => {
      const seed = rpTrailAt(i, rp.t);
      const full = rp.lineData[i] || [];
      if (seed.length === 0 && full.length) seed.push(full[0]);
      if (seed.length === 1 && full.length > 1) seed.push(full[1]);
      return {
        type: "line3D",
        name: o.name,
        data: seed,
        lineStyle: { color: seriesColor(i), width: 2.5 },
      };
    });
    // 每个对象：速度矢量 2D 箭头（杆 + 倒刺）× 1 + 姿态线框 × 2 条（**只有亮黄**，不再垫描边，
    // 只画前 N 个对象）。
    const nModel = Math.min(rp.objects.length, RP_MODEL_MAX);
    const vecSeries = [];
    const mdlSeries = [];
    for (let i = 0; i < nModel; i++) {
      const c = seriesColor(i);
      for (let k = 0; k < 2; k++) {
        vecSeries.push({
          type: "line3D", name: t("rp.series.vec", { name: rp.objects[i].name }), data: [],
          lineStyle: { color: c, width: k === 0 ? 4 : 3 },
        });
      }
      for (let k = 0; k < 2; k++) {
        mdlSeries.push({
          // id 必须**唯一**（同一份 option 里两条 id 相同会让 ECharts 静默丢掉整组 series）
          id: `${RP_MARKER_ID}-${i}-${k}`,
          type: "line3D", name: t("rp.series.att", { name: rp.objects[i].name }), data: [],
          lineStyle: { color: RP_MARKER_COLOR, width: RP_MARKER_MAIN_W },
        });
      }
    }
    const f0 = rp.objects[0].frames[0];
  const ax0 = rpAxisOption(ll2km(f0[FC.east_km], f0[FC.north_km]));
    rp.axis = ax0._rng;
    delete ax0._rng;
    rpGroundReset();
    // 底面（xoy 平面上的底图 / 网格线）排在**最后**：同类型的 series 会按下标匹配更新，
// 插到前面会打乱轨迹/箭杆的下标匹配；它带 id，更新按 id 命中。
    const ground = rpGroundSeries(rp.axis);
    rp.groundPushed = ground;
    rp.chart.setOption(Object.assign({
      animation: false,
      // 遥测文字改成悬停提示：常驻标签会盖住飞机模型，而且右侧面板已有同样的数据
      tooltip: { trigger: "item", formatter: (p) => (p.data && p.data.tel) || p.seriesName },
      series: [
        ...lineSeries,
        ...vecSeries,
        ...mdlSeries,
        {
          type: "scatter3D",
          name: "current",
          data: [],
          symbolSize: RP_MARKER_SYMBOL,
// 位置点与姿态线框同色，没有深色边框。
          itemStyle: { color: RP_MARKER_COLOR },
        },
        ground,
      ],
    }, ax0), true);
    rpGroundLoad();     // 底图异步解码：到位后自己重铺（见 rpGroundLoad）
    updateReplay();
  }
}

/* ---------- xoy 底面地图：底图 → z=0 平面上的原图纹理四边形（surface） ----------
 *  画法只有一种：一块 `surface` 四边形（4 个顶点 / 2 个三角形），贴图 = `.wpr` 里那份原始图片
 *  字节，由 GPU 直接采样（`colorMaterial.detailTexture`）—— 顶点数恒定，拉远拉近都不重建几何、
 *  不重采样；JS 侧**不读底图像素**（省掉一次解码与 16 MB RGBA 常驻）。
 *  贴图落位（隔离实验实测）：`surface` 的 texcoord 是
 *  数据网格的归一化下标、四边形按"行 0 = 最南"排，于是
 *  `textureTiling = 覆盖区尺寸 / 地图尺寸`、`textureOffset = 覆盖区西南角 / 地图尺寸`。
 *  只铺"当前坐标轴窗口 ∩ 地图矩形"：底面不参与 3D 裁剪，多铺会戳出盒子、压住轴标签。
 *  没有底图（记录没带图 / 加载失败 / 关掉开关）→ 退化成 z=0 网格线（line3D）。
 *  点数与耗时记在 `rp.groundStats`（探针直接读它，不靠肉眼）。
 */
const RP_GROUND_ID = "rp-ground";      // series id：更新按 id 命中，不打乱其它 series 的下标匹配
/** series 名 = `rp.ground` 这条文案（**探针按名字找它**，别改成别处对不上的字符串）；
 *  用函数而不是常量：切语言时要跟着变。 */
const rpGroundName = () => t("rp.ground");
/** 毫秒计时（performance 不可用时退回 Date.now） */
const rpNow = () => (window.performance && performance.now ? performance.now() : Date.now());

/** 底图矩形（世界 km，北为正）；元数据缺失/退化 → null（此时没有可采样的底图） */
function rpGroundRect() {
  const m = rp.map;
  if (!m) return null;
  const n = (v) => (Number.isFinite(Number(v)) ? Number(v) : NaN);
  const x0 = n(m.x0_km), x1 = n(m.x1_km), y0 = n(m.y0_km), y1 = n(m.y1_km);
  if (![x0, x1, y0, y1].every(Number.isFinite)) return null;
  if (!(x1 - x0 > 1e-6) || !(y1 - y0 > 1e-6)) return null;
  return { x0, x1, y0, y1 };
}

/** 「好看」的网格步长：1 / 2 / 5 × 10ⁿ（退化网格用，目标约 target 条线） */
function rpNiceStep(span, target) {
  const raw = Math.max(span / Math.max(1, target), 1e-4);
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const k = raw / mag;
  return (k <= 1 ? 1 : k <= 2 ? 2 : k <= 5 ? 5 : 10) * mag;
}

/** 步长 → 显示小数位（10/2/5 → 0 位，0.5/0.2 → 1 位，0.05 → 2 位…）。
 *  取整只作用于**显示**：坐标轴范围、`ll2km`、`rp.bbox`、`rp.groundStats` 与导出数据
 *  一律保持原精度（只改显示，不改内部计算）。 */
function rpStepDecimals(step) {
  if (!Number.isFinite(step) || step <= 0) return 0;
  return Math.min(3, Math.max(0, Math.ceil(-Math.log10(step))));
}

/** 按位数四舍五入（-0 → 0，避免轴上出现 "-0"） */
function rpRoundShow(v, d) {
  const k = Math.pow(10, d);
  const r = Math.round((Number(v) || 0) * k) / k;
  return Object.is(r, -0) ? 0 : r;
}

/** 坐标轴的 axisLabel 配置：按步长取整的显示值 —— 轴上不再出现 9 位小数。
 *  刻度间隔由**轴级的 `interval`**（见 rpAxisOption）定，这里只管印出来的数字：
 *  `formatter` 是显示层的东西，ECharts 的 `getViewLabels()` 走的就是它（3D 轴也一样）。 */
function rpAxisDisplay(step) {
  const d = rpStepDecimals(step);
  return { formatter: (v) => String(rpRoundShow(v, d)) };
}

/** 提示行里的 km 数字（1 位小数）：只格式化**显示**，`rp.groundStats` 里的原值不动 */
function rpKm1(v) {
  const n = Number(v);
  return Number.isFinite(n) ? (Math.round(n * 10) / 10).toFixed(1) : "—";
}

/** 底图加载：把底图 URL 交给 `colorMaterial.detailTexture` 由 GPU 直接采样 ——
 *  JS 侧不读底图像素（不分配 2048×2048 的 RGBA）。字节走 `/api/replay/map`（同源）。
 *
 *  底图来源只有一处：**本记录内嵌的底图**（`rp.map.has_image`）。记录没带底图时**不铺底图**，
 *  也不去借别的记录的图，保持退化网格线。 */
function rpGroundLoad() {
  const rect = rpGroundRect();
  const file = rp.mapFile;
  if (!rect || !file) return;
  if (rp.map && rp.map.has_image === false) {
    return;                            // 记录没带底图：不铺底面底图（退化成网格线）
  }
  rp.groundFrom = file;
  const url = `/api/replay/map?file=${encodeURIComponent(file)}`;
  if (rp.groundSrc && rp.groundSrc.url === url) return;   // 已就绪 / 在途
  const entry = { url, ready: false, pending: true, w: 0, h: 0, err: "", file };
  rp.groundSrc = entry;
  const im = new Image();
  im.onload = () => {
    entry.w = Math.max(1, im.naturalWidth || 1);
    entry.h = Math.max(1, im.naturalHeight || 1);
    entry.ready = true;
    entry.pending = false;
    rpGroundReset();                // 图变了 → 强制重建
    rpGroundApply();
    rpGroundNote();
  };
  im.onerror = () => {
    entry.err = t("rp.ground.err");
    entry.pending = false;
    rpGroundReset();
    rpGroundApply();
    rpGroundNote();
  };
  im.src = url;
}

/** 底面来源提示行（`#rp-ground-note`）：正常记录留空；**没带底图 / 底图不可用**时明确写出来。
 *  —— 回放里没有地图要在界面上说明原因，不能只写进 stats。
 *  用户看到的是"底面什么都没有"，看不出是记录没带底图。 */
function rpGroundNote() {
  const el = document.getElementById("rp-ground-note");
  if (!el) return;
  const s = rp.groundSrc;
  let text = "";
  if (rp.groundFrom && s && s.err) {
    text = t("rp.ground.unusable", { err: s.err });
  } else if (!rp.groundFrom) {
    text = t("rp.ground.nomap");
  }
  el.textContent = text;
  el.hidden = !text;
  if (text) report(text, false, "replay-ground");   // 同一条消息只留一条（key 替换式）
}

/** 退化原因（一句话，写进 stats 供探针读） */
function rpGroundWhy() {
  if (!rp.groundOn) return "开关关闭";
  if (!rpGroundRect()) return "记录无地图元数据";
  if (!rp.groundFrom) return "记录没带底图（不铺底面地图）";
  const s = rp.groundSrc;
  if (!s) return "底图未开始加载";
  if (s.err) return `底图不可用: ${s.err}`;
  if (s.pending) return "底图加载中";
  return "底图不可用";
}

/** 退化形态：z=0 上的网格线（世界对齐：按视野内的整数格铺，平移不重建） */
function rpGroundGrid(x0, x1, y0, y1) {
  const step = rpNiceStep(Math.max(x1 - x0, y1 - y0), 10);
  const i0 = Math.floor(x0 / step), i1 = Math.ceil(x1 / step);
  const j0 = Math.floor(y0 / step), j1 = Math.ceil(y1 / step);
  const key = `grid|${step}|${i0}|${i1}|${j0}|${j1}`;
  if (key === rp.groundKey && rp.groundSeries) return rp.groundSeries;
  const t0 = rpNow();
  const gx0 = i0 * step, gx1 = i1 * step, gy0 = j0 * step, gy1 = j1 * step;
  const data = [];
  for (let i = i0; i <= i1; i++) data.push([[i * step, gy0, 0], [i * step, gy1, 0]]);
  for (let j = j0; j <= j1; j++) data.push([[gx0, j * step, 0], [gx1, j * step, 0]]);
  const series = {
    id: RP_GROUND_ID, name: rpGroundName(),
    type: "line3D", silent: true, animation: false, data,
    lineStyle: { color: "#c2c2bb", width: 1, opacity: 0.9 },
  };
  rp.groundKey = key;
  rp.groundCover = null;
  rp.groundSeries = series;
  rp.groundStats = {
    mode: "grid", points: data.length, lines: data.length, stepKm: step,
    ms: Math.round((rpNow() - t0) * 10) / 10, reason: rpGroundWhy(),
  };
  return series;
}

/** **原图纹理**底面（唯一画法）：一块 `surface` 四边形，贴图 = `.wpr` 里那份原始 JPEG 字节，
 *  由 GPU 直接采样 —— 顶点数恒定，拉远拉近都不重建几何、不重采样。
 *
 *  贴图落位：`surface` 的 texcoord 是数据网格的归一化下标，四边形按"行 0 = 最南"排
 *  （claygl 默认 flipY）→ `textureTiling = 覆盖区尺寸 / 地图尺寸`、
 *  `textureOffset = 覆盖区西南角 / 地图尺寸`（与原图同区域逐像素比对，平均绝对差 1.1/255）。 */
function rpGroundTexture(x0, x1, y0, y1, rect, src) {
  // 覆盖区 = 当前坐标轴窗口 ∩ 地图矩形（与网格档同口径：底面不参与 3D 裁剪，不能铺出盒子）
  const qx0 = Math.max(rect.x0, x0), qx1 = Math.min(rect.x1, x1);
  const qy0 = Math.max(rect.y0, y0), qy1 = Math.min(rect.y1, y1);
  if (!(qx1 - qx0 > 1e-6) || !(qy1 - qy0 > 1e-6)) return rpGroundGrid(x0, x1, y0, y1);
  const mapW = rect.x1 - rect.x0, mapH = rect.y1 - rect.y0;
  const tx = (qx1 - qx0) / mapW, ty = (qy1 - qy0) / mapH;
  const ox = (qx0 - rect.x0) / mapW, oy = (qy0 - rect.y0) / mapH;
  const key = `texture|${src.url}|${tx.toFixed(9)}|${ty.toFixed(9)}|${ox.toFixed(9)}|${oy.toFixed(9)}`;
  if (key === rp.groundKey && rp.groundSeries) return rp.groundSeries;
  const t0 = rpNow();
  const white = "rgb(255,255,255)";     // 顶点色留白：贴图原样显示（不再乘一层颜色）
  const data = [
    [qx0, qy0, 0, white], [qx1, qy0, 0, white],
    [qx0, qy1, 0, white], [qx1, qy1, 0, white],
  ];
  const series = {
    id: RP_GROUND_ID, name: rpGroundName(),
    type: "surface", silent: true, animation: false,
    shading: "color", wireframe: { show: false },
    itemStyle: { opacity: 1, color: (p) => (p.data && p.data[3]) || "#ffffff" },
    // 贴图走 colorMaterial.detailTexture（echarts-gl 的材质贴图槽）：原图字节 → GPU 纹理
    colorMaterial: { detailTexture: src.url, textureTiling: [tx, ty], textureOffset: [ox, oy] },
    data,
  };
  rp.groundKey = key;
  rp.groundCover = { x0: qx0, x1: qx1, y0: qy0, y1: qy1 };
  rp.groundSeries = series;
  rp.groundBuilds = (rp.groundBuilds || 0) + 1;
  const r4 = (v) => Math.round(v * 10000) / 10000;
  rp.groundStats = {
    mode: "texture", points: 4, cols: 2, rows: 2,
    srcPx: [src.w, src.h], pxM: Math.round((mapW / src.w) * 1000),   // 原图 1 px = 多少米
    coverKm: [r4(qx1 - qx0), r4(qy1 - qy0)],
    texels: [Math.round(tx * src.w), Math.round(ty * src.h)],   // 这一块铺了多少原图像素
    ms: Math.round((rpNow() - t0) * 10) / 10,
    spanKm: [Math.round((x1 - x0) * 10) / 10, Math.round((y1 - y0) * 10) / 10],
    rectKm: [rect.x0, rect.x1, rect.y0, rect.y1],
    imageFrom: src.file || rp.mapFile,
    rebuilds: rp.groundBuilds,
  };
  return series;
}

/** 生成/复用底面 series；`win` = 当前坐标轴窗口 [x0, x1, y0, y1]（动态重采样的依据）。
 *  只有两条路：**原图纹理**（有可用底图）与 **z=0 网格线**（没有底图 / 开关关掉）。 */
function rpGroundSeries(win) {
  const [x0, x1, y0, y1] = win;
  const rect = rpGroundRect();
  const src = rp.groundSrc;
  const usable = !!(rp.groundOn && rect && src && src.ready && src.w > 0 && src.h > 0);
  if (!usable) return rpGroundGrid(x0, x1, y0, y1);
  return rpGroundTexture(x0, x1, y0, y1, rect, src);   // 唯一画法：原图纹理（4 顶点，GPU 直接采样）
}

/** 底面提示行（`#rp-ground-res-note`）：只有原图纹理档才显示 —— 把"贴的就是记录里的原图、
 *  一个原图像素多少米、这一块铺了多少原图像素、多少顶点、建了多久"写在界面上。 */
function rpGroundResNote() {
  const el = document.getElementById("rp-ground-res-note");
  if (!el) return;
  const st = rp.groundStats;
  if (!st || st.mode !== "texture") {     // 网格线档不写这一行（来源提示行已经说明原因）
    el.textContent = "";
    el.hidden = true;
    return;
  }
  el.textContent = t("rp.ground.res_note", {
    w: st.srcPx[0], h: st.srcPx[1], pxm: st.pxM,
    cw: rpKm1(st.coverKm[0]), ch: rpKm1(st.coverKm[1]),
    tw: st.texels[0], th: st.texels[1], p: st.points, ms: st.ms,
  });
  el.hidden = false;
}

/** 把底面 series 推给图表：同一个 series 对象（缓存命中）就不重复 setOption。
 *  ⚠️ **两行提示不管缓存命不命中都要重写**：它们是 `textContent = t(...)`（幂等且零成本），
 *  而语言切换走 `rebuildReplay()` 时视野没变、算出的常是同一个 series 对象，
 *  早期 return 会把提示行留在旧语言里。 */
function rpGroundApply() {
  if (!rp.chart || rp.chart.isDisposed() || !rp.gl || !rp.axis) return;
  if (!rp.objects.length) return;          // 还没装载记录：别往空图里塞底面 series
  const s = rpGroundSeries(rp.axis);
  if (!s) return;
  if (s !== rp.groundPushed) {
    rp.groundPushed = s;
    rp.chart.setOption({ series: [s] });
  }
  rpGroundNote();        // 底图来源（不带底图的记录不铺底面）
  rpGroundResNote();     // 原图纹理档的画法/尺寸提示行
}

/** 底面缓存复位（清空回放 / 换记录 / 换底图时用）。
 *  ⚠️ `groundFrom`（"底图来自哪一份记录"）**不在这里清**：
 *  它描述的是当前记录的底图来源，由 [`rpGroundLoad`] 设置、[`resetReplayView`] 在清空时清掉。 */
function rpGroundReset() {
  rp.groundKey = "";
  rp.groundSeries = null;
  rp.groundPushed = null;
  rp.groundCover = null;
  rp.groundStats = null;
}

/** 清空底面来源提示（清空回放文件时用） */
function rpGroundNoteClear() {
  rp.groundFrom = null;
  for (const id of ["rp-ground-note", "rp-ground-res-note"]) {
    const el = document.getElementById(id);
    if (el) {
      el.textContent = "";
      el.hidden = true;
    }
  }
}

/** 转换：把回放里第一个 `.wpr` 导出成 TacView ACMI / TacView CSV / FlatCSV（写入 logs/） */
async function convertReplay() {
  const btn = $("#rp-convert");
  // 只能转换已加入回放的 `.wpr` 飞行记录（平面坐标 + 底图；接口也只列/只收 .wpr）
  const f0 = rp.files.find((f) => f.planar) || null;
  const w = f0 ? f0.name : "";
  if (!w) {
    report(t("rp.convert.none"), false, "replay");
    return;
  }
  const format = $("#rp-convert-format").value;
  const lat = $("#rp-convert-lat").value;
  const lon = $("#rp-convert-lon").value;
  const q = `/api/replay/convert?file=${encodeURIComponent(w)}` +
    `&format=${encodeURIComponent(format)}&lat=${encodeURIComponent(lat)}&lon=${encodeURIComponent(lon)}`;
  btn.disabled = true;
  try {
    const d = await api(q);
    report(t("rp.convert.done", { src: w, dst: d.name, size: d.size }), true, "replay");
    loadReplayFiles();               // 产物就在 logs/ 下，刷新列表即可直接回放
  } catch (err) {
    report(t("rp.convert.failed", { err: err.message }), false, "replay");
  } finally {
    btn.disabled = false;
  }
}

function updateReplay() {
  if (!rp.objects.length) return;

  // 平面模式与底图来自 `.wpr`（多文件混载时：任一为平面即按平面处理，底图取第一个带的）。
  // 必须在 ll2km 之前确定 —— 平面记录的第 2/3 槽是"世界坐标 km"，不需要经纬度换算。
// 这里再调一次是兜底（rebuildReplay 里已先调过；时间轴拖动不重建轨迹）。
  rpSyncContext();

  // 时间钟（当前时间，显示在滑条旁；格式 mm:ss.mmm）
  $("#rp-clock").textContent = fmtMs(rp.t - rp.t0);

  const N = rp.objects.length;
  const frames = rp.objects.map((o) => frameAt2(o.frames, rp.t));
  const kmPos = frames.map((f) => ll2km(f[FC.east_km], f[FC.north_km]));

  const curs = rp.objects.map((o, i) => {
    const f = frames[i];
    // 悬停提示（纯航空记法 + 数字，走 t() 只为让别的语言能改 M/AoA 这类写法）
    const tel = `${o.name}\n` +
      t("rp.tip.l1", { tas: (f[FC.tas_ms] * 3.6).toFixed(0), ias: (f[FC.ias_ms] * 3.6).toFixed(0) }) + "\n" +
      t("rp.tip.l2", { mach: f[FC.mach].toFixed(2), aoa: f[FC.aoa_deg].toFixed(1) }) + "\n" +
      t("rp.tip.l3", { alt: Math.round(f[FC.alt_m]), ny: f[FC.ny].toFixed(2) });
    // 当前位置标记固定亮黄（对象身份靠轨迹线与右侧遥测的左边框颜色区分，见 RP_MARKER_COLOR）
    return { value: [kmPos[i][0], kmPos[i][1], f[FC.alt_m]], tel,
             itemStyle: { color: RP_MARKER_COLOR } };
  });

  // 每个对象：速度矢量 2D 箭头（水平面内，方向=地面航迹）+ 姿态线框飞机（**只有亮黄**）
  const nModel = Math.min(N, RP_MODEL_MAX);
  const vecData = new Array(nModel * 2);
  const mdlData = new Array(nModel * 2);
  for (let i = 0; i < nModel; i++) {
    const f = frames[i];
    const B = rpBasis(f[FC.roll_deg], f[FC.pitch_deg], f[FC.heading_deg]);
    const aoa = (Number(f[FC.aoa_deg]) || 0) * Math.PI / 180;
    // 速度矢量 = **重采样点之差**（等价于插值曲线的切向），不再是"姿态轴绕 AoA"的近似；
    // 静止 / 只有单点时退回机体轴（`v = F·cos(aoa) − U·sin(aoa)`）。
    const dir = rpTangentAt(rp.objects[i], rp.t) || [
      B.F[0] * Math.cos(aoa) - B.U[0] * Math.sin(aoa),
      B.F[1] * Math.cos(aoa) - B.U[1] * Math.sin(aoa),
      B.F[2] * Math.cos(aoa) - B.U[2] * Math.sin(aoa),
    ];
    const P = [kmPos[i][0], kmPos[i][1], f[FC.alt_m]];
    const arrow = rpArrowPolys(P, dir, rp.arrowKm);
    const model = rpModelPolys(P, B, rp.modelH, rp.modelV);
    for (let k = 0; k < 2; k++) {
      vecData[i * 2 + k] = { data: arrow[k] };
      // 2 条/对象：轮廓 + 垂尾，都是亮黄（没有描边组了 —— 见 RP_MARKER_COLOR 一段）
      mdlData[i * 2 + k] = { data: model[k] };
    }
  }

  if (rp.chart && rp.gl) {
    // series = [尾迹 × N, 速度矢量 × 2×M, 姿态模型 × 2×M, scatter 当前点]：
    // 尾迹只画"已飞过部分"（rpTrailAt 截断 + 末端插值点，每次新数组触发 GL 重绘）；
    // GL 不可用时图表只有标题、没有 series 定义，此时不推无类型的 series。
    const upd = [];
    for (let i = 0; i < N; i++) {
      const tail = rpTrailAt(i, rp.t);
      const f = frames[i];
      if (Number.isFinite(kmPos[i][0]) && Number.isFinite(kmPos[i][1]) && Number.isFinite(f[FC.alt_m])) {
        tail.push([kmPos[i][0], kmPos[i][1], f[FC.alt_m]]);
      }
      upd.push({ data: tail });
    }
    upd.push(...vecData, ...mdlData, { data: curs });
    const cur0 = kmPos[0] || [0, 0];
    if (rpAxisNeeded(cur0)) {
      const ax = rpAxisOption(cur0);
      rp.axis = ax._rng;
      delete ax._rng;
      rp.chart.setOption(Object.assign({ series: upd }, ax));
    } else {
      rp.chart.setOption({ series: upd });
    }
    rpGroundApply();     // 坐标轴窗口变了 → 底面按新窗口重采样（缓存命中时是空操作）
  }

  // 右侧遥测面板（速度/表速/马赫/攻角/高度/过载/航向/俯仰）
  const rows = rp.objects.map((o, i) => {
    const f = frames[i];
    const c = seriesColor(i);
    const hdg = Number.isFinite(f[FC.heading_deg]) ? f[FC.heading_deg].toFixed(1) : "—";
    const pit = Number.isFinite(f[FC.pitch_deg]) ? f[FC.pitch_deg].toFixed(1) : "—";
    return `<div class="tel-row" style="border-left-color:${c}">
      <div class="tel-name">${esc(o.name)}</div>
      <div class="tel-vals">
        ${esc(t("rp.tel.ias"))} <b>${(f[FC.tas_ms] * 3.6).toFixed(0)}</b> · ${esc(t("rp.tel.cas"))} <b>${(f[FC.ias_ms] * 3.6).toFixed(0)}</b> km/h<br>
        ${esc(t("rp.tel.mach"))} <b>${f[FC.mach].toFixed(2)}</b> · ${esc(t("rp.tel.aoa"))} <b>${f[FC.aoa_deg].toFixed(1)}°</b><br>
        ${esc(t("rp.tel.alt"))} <b>${Math.round(f[FC.alt_m])}</b> m · ${esc(t("rp.tel.g"))} <b>${f[FC.ny].toFixed(2)}</b> g<br>
        ${esc(t("rp.tel.hdg"))} <b>${hdg}°</b> · ${esc(t("rp.tel.pitch"))} <b>${pit}°</b>
      </div></div>`;
  });
  $("#rp-telemetry").innerHTML = rows.join("") || esc(t("rp.telemetry.none"));
}

function rpTick() {
  if (!rp.playing) return;
  rp.t += RP_TICK_MS * rp.speed;
  if (rp.t >= rp.t1) { rp.t = rp.t1; rpStop(); }
  $("#rp-time").value = rp.t - rp.t0;
  updateReplay();
}

function rpStop() {
  rp.playing = false;
  if (rp.timer) { clearInterval(rp.timer); rp.timer = null; }
  $("#rp-play").textContent = t("rp.play");
}

$("#rp-play").addEventListener("click", () => {
  if (rp.playing) {
    rpStop();
    return;
  }
  if (!rp.objects.length) { report(t("rp.err.no_recording"), false); return; }
  if (rp.t >= rp.t1) rp.t = rp.t0;
  rp.playing = true;
  $("#rp-play").textContent = t("rp.pause");
  rp.timer = setInterval(rpTick, RP_TICK_MS);
});
$("#rp-time").addEventListener("input", (e) => {
  rpStop(); // 时间轴拖动与播放互斥
  rp.t = rp.t0 + Number(e.target.value);
  updateReplay();
});
$("#rp-speed").addEventListener("change", (e) => {
  rp.speed = Number(e.target.value);
});
// 视野尺度（km）：跟随当前点缩放 / 全程自适应
$("#rp-scale").addEventListener("change", (e) => {
  rp.scaleKm = Number(e.target.value) || 0;
  rp.axis = null;                       // 强制重写坐标轴
  rpGroundReset();                      // 分辨率跟着视野走 → 底面必须重采样
  if (rp.objects.length && rp.chart && rp.gl) {
    rpSizes();
    const f0 = frameAt2(rp.objects[0].frames, rp.t);
    const ax = rpAxisOption(ll2km(f0[1], f0[2]));
    rp.axis = ax._rng;
    delete ax._rng;
    rp.chart.setOption(ax);
    updateReplay();
    report(rp.scaleKm ? t("rp.view.follow", { n: rp.scaleKm }) : t("rp.view.fit_toast"),
           true, "replay");
  }
});
// xoy 底面开关：关掉就退化成 z=0 网格线（性能兜底 / 只想看轨迹时用）
$("#rp-ground").addEventListener("change", (e) => {
  rp.groundOn = e.target.checked;
  rpGroundReset();
  rpGroundApply();
  const st = rp.groundStats;
  report(rp.groundOn ? t("rp.ground.on") : t("rp.ground.off"), true, "replay");
  if (st) notify(t("rp.ground.mode_toast", {
    mode: st.mode === "texture" ? t("rp.ground.mode.texture") : t("rp.ground.mode.grid"),
  }), true, "replay-ground");
  rpGroundNote();
  rpGroundResNote();
});
$("#rp-add").addEventListener("change", (e) => {
  const name = e.target.value;
  e.target.value = "";              // 立即复位，方便再次选择
  if (name) addReplayFile(name);
});
$("#rp-clear").addEventListener("click", clearReplayFiles);
$("#rp-convert").addEventListener("click", convertReplay);
$("#rp-pick").addEventListener("click", () => $("#rp-file-input").click());
$("#rp-file-input").addEventListener("change", async (e) => {
  const files = Array.from(e.target.files || []);
  e.target.value = "";
  for (const f of files) {
    try {
      const buf = await f.arrayBuffer();
      const r = await fetch(`/api/replay/upload?name=${encodeURIComponent(f.name)}`, {
        method: "POST",
        headers: { "Content-Type": "application/octet-stream" },
        body: buf,
      });
      if (!r.ok) throw new Error(`${r.status}: ${await r.text()}`);
      const saved = await r.json();
      await addReplayFile(saved.name);
      if (saved.name !== f.name) notify(t("rp.upload.renamed", { name: saved.name }), true, "replay");
    } catch (err) {
      notify(t("rp.upload.failed", { name: f.name, err }), false, "replay");
    }
  }
});

/** 语言切换后重画本页由 JS 生成的文案。配置器那一半由 `app.js` 的 `setRenderForm` 重渲染、
 *  FM 那一半由 `fmOnLangChange()`；回放页签的这一半在这里：
 *  下拉状态 / 文件标签空态 / 3D 图的轴名与 series 名 / 右侧遥测 / 底面两行提示 / 播放按钮。
 *
 *  用 `rebuildReplay()` 重建（轴名与 series 名只在建图时写一次），但**保留当前播放位置**
 *  与播放状态 —— 切语言不该把回放倒回起点。`app.js` 用可选全局方式调它。 */
function rpOnLangChange() {
  loadReplayFiles();
  renderReplayChips();
  if (!rp.objects.length) {
    resetReplayView();          // 空态文案 + 图表占位都按新语言重写（内含 rpStop）
    return;
  }
  const keepT = rp.t;
  const wasPlaying = rp.playing;
  rebuildReplay();              // 内含 rpStop()
  rp.t = keepT;
  $("#rp-time").value = rp.t - rp.t0;
  updateReplay();
  // 底面那两行是**纯 textContent**：重建常常算出同一个 series（视野没变）而走缓存短路，
  // 这里再显式刷一次，保证切语言后不留旧语言的字（`rpGroundApply` 也补了同样的保险）。
  rpGroundNote();
  rpGroundResNote();
  if (wasPlaying) {
    rp.playing = true;
    $("#rp-play").textContent = t("rp.pause");
    rp.timer = setInterval(rpTick, RP_TICK_MS);
  }
}
