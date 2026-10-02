/* wp8f 控制台前端 —— **飞行模型页签**
 * 零构建 classic script：共享全局作用域，依赖 app.js 先加载
 *（`$` / `api` / `esc` / `report` / `t` / `seriesColor` / `GRID3D_BASE` / `AXIS_STYLE` /
 *  `chartPlaceholder` / `liveChart` / `withTheme` / `fmEachChart`）。
 *
 * **文案自 P6 起全部走 `t()`**（键前缀 `fm.`）：机型信息卡片、图表轴名/图例/标注/空图提示、
 * toast 与状态提示，一条硬编码中文都不留。键在 `resource/i18n/<lang>.json` 里（15 种语言同键集），
 * 缺键会在 `scripts/tests/ui_checks.py` 的"无缺失键"断言里红。
 * 语言切换后由 `app.js` 的 `setUiLang()` 调 `fmOnLangChange()` 重画这一页（见文件末尾）。
 */

/* ---------- FM 机型选择（下拉通道） ---------- */
let fmAll = [];
let fmActive = null;

/** FM 数据库版本（本地飞机性能数据里的 `resource/data/version`）：写在页签小标题右侧（`#fm-version`）。
 *  服务端启动日志打的是同一份值；读不到（数据没搬到 resource/data）就写"未知"，不编造。 */
async function loadFmVersion() {
  const el = document.getElementById("fm-version");
  if (!el) return;
  try {
    const d = await api("/api/fm/version");
    el.textContent = (d && d.ok && d.version)
      ? t("fm.version", { v: d.version })
      : t("fm.version_unknown");
  } catch (e) {
    el.textContent = t("fm.version_unknown");
  }
}

async function loadAircraft() {
  loadFmVersion();       // 与机型列表各走各的，别让版本这一行拖住列表
  if (fmAll.length) return;
  try {
    fmAll = await api("/api/fm/aircraft");
    renderFmChoices();
    // 自动加载当前选中的机型：此前下拉选中了却不加载，用户看到的是一片空白图表
    if (fmActive && !fmCurrentName) await fmAutoLoad(fmActive);
  } catch (e) {
    report(t("fm.err.aircraft_list", { err: e }), false);
  }
}

function fmMatches() {
  return fmAll; // 过滤选项已移除：下拉直接列出全部机型
}

/** 重建下拉 options（保持选中；机型仅用下拉选择） */
function renderFmChoices() {
  const sel = $("#fm-aircraft");
  const matched = fmMatches();
  if (fmActive == null || !matched.includes(fmActive)) fmActive = matched[0] ?? null;
  const opts = matched.map((n) => {
    const o = document.createElement("option");
    o.value = n;
    o.textContent = n;              // 机型名是数据里的标识，不翻译
    o.selected = n === fmActive;
    return o;
  });
  sel.replaceChildren(...opts);
  if (fmActive) sel.value = fmActive;
  sel.title = t("fm.aircraft_count", { n: matched.length });
}

function currentFmName() {
  const sel = $("#fm-aircraft");
  return sel && sel.value ? sel.value : (fmActive || "");
}

$("#fm-aircraft").addEventListener("change", (e) => {
  fmActive = e.target.value;
  fmAutoLoad(fmActive); // 下拉选择即自动加载
});

/* ---------- FM 计算参数（燃油比例 / 额外重量，300ms 防抖联动刷新） ---------- */
let fmMaxFuelKg = 0;
function fmWeightParams() {
  let fuel = Number($("#fm-fuel").value);
  if (!Number.isFinite(fuel)) fuel = 50;
  fuel = Math.min(100, Math.max(0, fuel));
  let extra = Number($("#fm-extra").value);
  if (!Number.isFinite(extra)) extra = 0;
  extra = Math.min(100000, Math.max(0, extra));
  return { fuel, extra };
}

function fmQuery() {
  const p = fmWeightParams();
  return `fuel_pct=${p.fuel}&extra_weight=${p.extra}`;
}

let fmRefreshTimer = null;
function scheduleFmRefresh() {
  clearTimeout(fmRefreshTimer);
  fmRefreshTimer = setTimeout(async () => {
    const name = currentFmName();
    if (!name) return;
    // 已固定的对比机型必须跟着新参数一起重取，否则同图上参数不一致（见 refetchFmPinned）
    const failed = await refetchFmPinned();
    if (failed.length) {
      report(t("fm.err.pinned_refetch", { list: failed.join(", ") }), false, "fm-pinned");
    }
    await loadFmInfo(name);
    await addFmCompare(name, true);
  }, 300);
}
$("#fm-fuel").addEventListener("input", (e) => {
  $("#fm-fuel-val").textContent = `${e.target.value}% · ${Math.round(fmMaxFuelKg * Number(e.target.value) / 100)} kg`;
  scheduleFmRefresh();
});
$("#fm-extra").addEventListener("change", scheduleFmRefresh);

/* ---------- FM 曲线（ECharts + echarts-gl）与多机型对比 ---------- */
const fmCompare = {}; // name -> curves json
let chartCL = null;
let chartPower = null;
let chart3d = null;
let chartTasAlt = null;
let chartThrustTas = null;
let chartThrustIas = null;
let chartEm = null;

/* ---------- EM（能量机动）图：完全照 `flightmodel/scripts/plot_flight_model.py` 的 plot_em ----------
 * X = 表速 IAS (km/h)、Y = 盘旋率 (deg/s)、y 轴固定 0..33；
 * Ps 色带（WEP，±400 m/s，暖红→近白→冷绿的同一组颜色）+ 瞬时/持续(WEP)/持续(军推) 三条包线 +
 * 两条峰值标注（速度 · °/s · G）+ 等过载线 n=2/3/4/6/8/12 + VNE 竖线；
 * 横轴右边界 = min(VNE, 数据上界)，左边界 = 数据下界 —— 与脚本逐条一致。
 * 数据来自 `flightmodel --curves-json` 的 `em` 段（只喷气机有）。 */
const EM_PS_COLORS = ["#8b1a1a", "#c0392b", "#e8a87c", "#f3ddc2", "#f7f7f5",
                      "#b9e3a8", "#6fbf73", "#2e8b57", "#145a32"];
const EM_LOAD_FACTORS = [2, 3, 4, 6, 8, 12];
const EM_Y_MAX = 33;          // 与脚本的 ylim(0, 33) 一致（网格采样范围）
const EM_PS_RANGE = 400;      // 脚本 vmin/vmax = ±400 m/s
let fmEmAltIdx = 0;           // 当前看的高度档（脚本是一档一张图，这里用下拉切换）
let fmEm = null;              // 最近一次 curves 响应里的 em 段（主选机型）

/** 在曲线数组里找峰值（脚本的 mark_peak：取最大盘旋率那一点）。 */
function emPeak(vs, ws, ns) {
  let bi = -1;
  for (let i = 0; i < ws.length; i++) if (bi < 0 || ws[i] > ws[bi]) bi = i;
  return bi < 0 ? null : { v: vs[bi], w: ws[bi], n: ns ? ns[bi] : null };
}

/** 等过载线：ω = g·√(n²−1)/V_tas，横轴是表速 → 乘 ias_per_tas（脚本同一公式）。 */
function emLoadLine(vs, n, iasPerTas) {
  const out = [];
  for (const v of vs) {
    const omega = (180 / Math.PI) * 9.81 * Math.sqrt(n * n - 1) * iasPerTas / (v / 3.6);
    if (omega <= EM_Y_MAX) out.push([v, omega]);
  }
  return out;
}

function renderEmChart() {
  if (!chartEm) return;
  // 容器被隐藏（活塞机型的 `data-fm-jet-only`）时别 setOption：ECharts 会报
  // "Dom has no width or height" 警告，而且画了也没人看。
  const box = document.getElementById("fm-chart-em");
  if (box && box.style.display === "none") return;
  // 实例可能是容器还隐藏时建的（0×0）：先用容器当前尺寸校正，再画
  if (chartEm.getWidth() === 0 || chartEm.getHeight() === 0) chartEm.resize();
  if (!fmEm) { emptyChart(chartEm, t("fm.em.none")); return; }
  const idx = Math.min(Math.max(fmEmAltIdx, 0), fmEm.alts.length - 1);
  const a = fmEm.alts[idx];
  const g = a.grid, c = a.curve;
  const xLeft = Math.min(...g.v);
  const xRight = Math.min(fmEm.vne_ias > 0 ? fmEm.vne_ias : Infinity, Math.max(...g.v));
  const iasPerTas = a.ias_per_tas > 0 ? a.ias_per_tas : 1;

  // ① Ps 色带：网格是规则 (v × w) 的，用 custom 画格子 —— 连续坐标轴（脚本的 tripcolor 等价物），
  //    格子大小由 api.size 换算，保证铺满且不留缝。
  const band = [];
  for (let i = 0; i < g.v.length; i++) {
    if (g.v[i] > xRight || g.ps_wep[i] === null) continue;
    band.push([g.v[i], g.w[i], g.ps_wep[i]]);
  }
  // 网格步长：数据是**扁平**的（v 主序、ω 次序），所以不能拿数组长度当档数 ——
  // 扫到第一个值变化的位置才是 v 的步长（ω 的步长取前两项之差）。
  let vStep = 10, wStep = 1;
  for (let i = 1; i < g.v.length; i++) {
    if (g.v[i] !== g.v[0]) { vStep = g.v[i] - g.v[0]; break; }
  }
  if (g.w.length > 1) wStep = g.w[1] - g.w[0];
  const bandSeries = {
    name: t("fm.em.ps"), type: "custom", clip: true, silent: true,
    renderItem: (params, api) => {
      const [cx, cy] = api.coord([api.value(0), api.value(1)]);
      const [cw, ch] = api.size([vStep, wStep]);
      return {
        type: "rect",
        shape: { x: cx - cw / 2, y: cy - ch / 2, width: cw, height: ch },
        // 取 visualMap 映射出来的颜色（`api.style()` 在 custom 系列里不一定带上它）
        style: { fill: api.visual("color") || "#808080" },
      };
    },
    encode: { x: 0, y: 1, tooltip: 2 },
    data: band,
  };

  // ② 三条包线（颜色/线型照脚本：instant 实线蓝、sustained WEP 橙虚线、sustained mil 紫点线）
  const lineOf = (name, ws, color, type) => ({
    name, type: "line", showSymbol: false, smooth: false,
    lineStyle: { color, width: 2, type },
    itemStyle: { color },
    data: c.v.map((v, i) => [v, ws[i]]),
  });
  const series = [
    bandSeries,
    lineOf(t("fm.em.instant"), c.w_inst, "#1f4e9c", "solid"),
    lineOf(t("fm.em.sus_wep"), c.w_sus_wep, "#ff8c00", "dashed"),
    lineOf(t("fm.em.sus_mil"), c.w_sus_mil, "#8e44ad", "dotted"),
  ];

  // ③ 峰值标注：`INST 1234 km/h | 25.1°/s  7.5G`
  const peakLabel = (label, pk, color, dy) => ({
    value: [pk.v, pk.w],
    symbolSize: 7,
    itemStyle: { color },
    label: {
      show: true, position: "top", distance: dy, fontSize: 10, color,
      backgroundColor: "rgba(255,255,255,0.85)", borderColor: color, borderWidth: 0.8,
      padding: [2, 4], lineHeight: 12,
      formatter: `${label} ${pk.v.toFixed(0)} km/h\n${pk.w.toFixed(1)}°/s  ${pk.n == null ? "" : pk.n.toFixed(1) + "G"}`,
    },
  });
  const peaks = [];
  const pkInst = emPeak(c.v, c.w_inst, c.n_av);
  if (pkInst) peaks.push(peakLabel(t("fm.em.instant_short"), pkInst, "#1f4e9c", 10));
  const pkSus = emPeak(c.v, c.w_sus_wep, c.n_sus);
  if (pkSus) peaks.push(peakLabel(t("fm.em.sus_short"), pkSus, "#e07b00", 10));
  if (peaks.length) series.push({ type: "scatter", symbol: "circle", data: peaks, z: 5, silent: true });

  // ④ 等过载线 + VNE 竖线（脚本：右边界就是 VNE，标 "VNE xxxx"）
  const vs = c.v.filter((v) => v >= Math.max(xLeft, 50) && v <= xRight);
  EM_LOAD_FACTORS.forEach((n) => {
    const pts = emLoadLine(vs, n, iasPerTas);
    if (pts.length < 2) return;
    series.push({
      // 名字不进图例（脚本里等过载线也不上图例）：`legend.data` 白名单见下
      type: "line", showSymbol: false, silent: true, z: 1, legendHoverLink: false,
      lineStyle: { color: "dimgray", width: 0.8, type: [2, 2], opacity: 0.7 },
      itemStyle: { color: "dimgray" },
      endLabel: { show: true, formatter: `${n}G`, fontSize: 8, color: "dimgray" },
      data: pts,
    });
  });
  if (fmEm.vne_ias > 0 && fmEm.vne_ias <= xRight + 1e-6) {
    series.push({
      type: "line", silent: true, z: 6, showSymbol: false, data: [],
      markLine: {
        symbol: "none",
        lineStyle: { color: "#333333", width: 1.6, type: "solid" },
        label: { formatter: t("fm.em.vne", { v: fmEm.vne_ias.toFixed(0) }), fontSize: 9, color: "#333333" },
        data: [{ xAxis: fmEm.vne_ias }],
      },
    });
  }

  chartEm.setOption({
    backgroundColor: "transparent",
    animation: false,
    tooltip: {
      trigger: "item",
      formatter: (p) => (p.seriesType === "custom"
        ? `${p.value[0].toFixed(0)} km/h<br>${p.value[1].toFixed(1)}°/s<br>${t("fm.em.ps")} ${Number(p.value[2]).toFixed(1)}`
        : `${p.seriesName}<br>${Array.isArray(p.value) ? p.value[0].toFixed(0) + " km/h<br>" + p.value[1].toFixed(1) + "°/s" : ""}`),
    },
    legend: {
      type: "scroll", textStyle: { color: "#444" }, top: 0,
      // 只列四条主曲线（等过载线不上图例，与脚本一致）
      data: [t("fm.em.ps"), t("fm.em.instant"), t("fm.em.sus_wep"), t("fm.em.sus_mil")],
    },
    grid: { left: 62, right: 78, top: 34, bottom: 46 },
    visualMap: {
      type: "continuous", min: -EM_PS_RANGE, max: EM_PS_RANGE, calculable: false,
      orient: "vertical", right: 8, top: "middle", itemHeight: 160,
      textStyle: { color: "#8c8c86", fontSize: 10 },
      inRange: { color: EM_PS_COLORS },
      text: [`+${EM_PS_RANGE}`, `-${EM_PS_RANGE}`],   // 色标上限（脚本 colorbar 的 ±400）
      seriesIndex: 0, dimension: 2,   // [v, w, Ps] 里第 3 维才是 Ps
    },
    xAxis: Object.assign({ type: "value", name: t("fm.ax.ias"), min: xLeft, max: xRight }, AXIS_STYLE),
    yAxis: Object.assign({ type: "value", name: t("fm.ax.turn_rate"), min: 0, max: EM_Y_MAX }, AXIS_STYLE),
    series,
  }, true);
}

/** 高度下拉：数据里有几档就填几项（与脚本一档一张图等价，只是这里用下拉切）。 */
function fillEmAltSelect() {
  const sel = document.getElementById("fm-em-alt");
  if (!sel) return;
  const alts = fmEm ? fmEm.alts.map((a) => a.alt) : [];
  sel.innerHTML = "";
  alts.forEach((alt, i) => {
    const o = document.createElement("option");
    o.value = String(i);
    o.textContent = `${alt.toFixed(0)} m`;
    sel.appendChild(o);
  });
  sel.disabled = alts.length === 0;
  if (alts.length) sel.value = String(Math.min(fmEmAltIdx, alts.length - 1));
}

/** 当前机型是不是喷气（`null` = 还没选机型）。活塞机型只保留
 *  「CL-α / 真空速-高度 / 活塞功率-高度」三张图：
 *  推力-真空速、推力-表速、推力 3D 曲面在活塞机上没有数据、也没有意义 ——
 *  连标题一起隐藏（`data-fm-jet-only` 挂在 `index.html` 的 `.sec` 标题与图容器上）。
 *  喷气机型保持原样；没选机型时按"全部显示"，与改造前一致。 */
let fmIsJet = null;
function applyFmChartVisibility() {
  const jet = fmIsJet !== false;
  for (const el of document.querySelectorAll("[data-fm-jet-only]")) {
    el.style.display = jet ? "" : "none";
  }
  // **隐藏期间建的 ECharts 实例是 0×0 尺寸**，之后再显示出来光 setOption 也画不出来
  // （ECharts 会警告 "Dom has no width or height"）→ 显示出来的那几个要 resize。
  // 只动"现在真的有尺寸"的实例：给隐藏容器 resize 只会刷一堆同样的告警。
  for (const c of [chartThrustTas, chartThrustIas, chart3d, chartEm]) {
    const el = c && typeof c.getDom === "function" ? c.getDom() : null;
    if (el && el.clientWidth > 0 && el.clientHeight > 0 && typeof c.resize === "function") c.resize();
  }
  if (jet) renderEmChart();   // 显示出来的同时补一帧（隐藏时那次渲染被跳过了）
}
/** 从接口数据里读 `is_jet` 并应用（info 与 curves 两条路都带这个字段，谁先到都行）。 */
function noteFmEngineKind(isJet) {
  if (typeof isJet !== "boolean" || isJet === fmIsJet) return;
  fmIsJet = isJet;
  applyFmChartVisibility();
}
/* 近似 viridis（对齐 matplotlib 3D 曲面配色） */
const VIRIDIS = ["#440154", "#414487", "#2a788e", "#22a884", "#7ad151", "#fde725"];

function ensureCharts() {
  if (!window.echarts) return false;
  // liveChart：实例被 dispose（占位函数会 dispose）后重新 init，避免 setOption 静默失效
  chartCL = liveChart(chartCL, "fm-chart-cl");
  chartPower = liveChart(chartPower, "fm-chart-power");
  chart3d = liveChart(chart3d, "fm-chart-3d");
  chartTasAlt = liveChart(chartTasAlt, "fm-chart-tasalt");
  chartThrustTas = liveChart(chartThrustTas, "fm-chart-thrust-tas");
  chartThrustIas = liveChart(chartThrustIas, "fm-chart-thrust-ias");
  chartEm = liveChart(chartEm, "fm-chart-em");
  for (const c of [chartCL, chartPower, chart3d, chartTasAlt, chartThrustTas, chartThrustIas, chartEm]) {
    if (c) c.setOption(withTheme({}));
  }
  return true;
}

function emptyChart(chart, msg) {
  chart.setOption({
    animation: false,
    graphic: [{
      type: "text", left: "center", top: "middle",
      style: { text: msg, fill: "#8c8c86", font: "13px sans-serif" },
    }],
    series: [],
  }, true);
}

/* 高度下拉：换档只重画 EM 图（数据已缓存，无需再请求）。
   注意：这是 FM 页签里唯一"只属于一张图"的控件，挂在 `data-fm-jet-only` 的行里，
   活塞机型整行一起隐藏。 */
document.addEventListener("change", (e) => {
  if (e.target && e.target.id === "fm-em-alt") {
    fmEmAltIdx = Number(e.target.value) || 0;
    renderEmChart();
  }
});

function renderCurves() {
  if (!ensureCharts()) return;
  const names = Object.keys(fmCompare);
  // 主机的喷气/活塞决定那三张推力图是否显示（对比机型不改变这个判断）
  if (names.length) noteFmEngineKind(fmCompare[names[0]].is_jet);
  // EM 图只画主选机型（脚本也是一次一张，没有多机对比）
  fmEm = names.length ? (fmCompare[names[0]].em || null) : null;
  fillEmAltSelect();
  renderEmChart();
  const clSeries = [];
  const pwSeries = [];
  let zmin = Infinity, zmax = -Infinity;
  let zunit = "kgf";

  names.forEach((n, i) => {
    const d = fmCompare[n];
    const c = seriesColor(i);
    clSeries.push({
      name: t("fm.series.clean", { name: n }), type: "line", showSymbol: false,
      lineStyle: { color: c, width: 2 },
      itemStyle: { color: c },
      data: d.aoa.map((a, k) => [a, d.cl_clean[k]]),
    });
    clSeries.push({
      name: t("fm.series.full", { name: n }), type: "line", showSymbol: false,
      lineStyle: { color: c, width: 1.3, type: "dashed" },
      itemStyle: { color: c },
      data: d.aoa.map((a, k) => [a, d.cl_full[k]]),
    });

    // ① 活塞功率-高度（对齐 matplotlib：X=Power(hp), Y=Altitude(m)）
    if (d.power_alt) {
      pwSeries.push({
        name: t("fm.series.mil", { name: n }), type: "line", showSymbol: false,
        lineStyle: { color: c, width: 2 }, itemStyle: { color: c },
        data: d.power_alt.mil.map((p, k) => [p, d.power_alt.alt[k]]),
      });
      pwSeries.push({
        name: t("fm.series.wep", { name: n }), type: "line", showSymbol: false,
        lineStyle: { color: c, width: 2, type: "dashed" }, itemStyle: { color: c },
        data: d.power_alt.wep.map((p, k) => [p, d.power_alt.alt[k]]),
      });
    }

    // ③ 推力 3D 曲面（velocity × altitude → thrust；行主序 x 递增，GL 据此推断网格）
  });

  /* 每机型一个 surface 块（echarts-gl data 为 [x,y,z] 扁平三元组，行主序） */
  const surfBlocks = [];
  names.forEach((n, i) => {
    const d = fmCompare[n];
    if (!d.thrust3d) return;
    zunit = d.unit || "kgf";
    const t3 = d.thrust3d;
    const pts = [];
    for (let ai = 0; ai < t3.alt.length; ai++) {
      for (let vi = 0; vi < t3.v.length; vi++) {
        const z = t3.z[ai][vi];
        if (Number.isFinite(z)) {
          if (z < zmin) zmin = z;
          if (z > zmax) zmax = z;
          pts.push([t3.v[vi], t3.alt[ai], z]);
        }
      }
    }
    if (pts.length) {
      surfBlocks.push({
        type: "surface", name: n, wireframe: { show: false },
        itemStyle: { opacity: 0.92 },
        data: pts,
      });
    }
  });

  chartCL.setOption({
    backgroundColor: "transparent",
    animation: false,
    tooltip: { trigger: "axis" },
    legend: { type: "scroll", textStyle: { color: "#444" } },
    grid: { left: 54, right: 20, top: 30, bottom: 42 },
    xAxis: Object.assign({ type: "value", name: t("fm.ax.aoa"), min: -25, max: 30 }, AXIS_STYLE),
    yAxis: Object.assign({ type: "value", name: t("fm.ax.cl") }, AXIS_STYLE),
    series: clSeries,
  }, true);

  /* ④ TAS-高度（对照 plot_tas_altitude：WEP 橙实线 / mil 蓝虚线 / VNE 灰点线 /
     MNE 青点线 / limit=min(VNE,MNE) 黑实线 / 每 2000m 速度标记 /
     **每个标记高度都标出最大平飞点的阻力**（脚本只在 2000m 那一处标，按用户口径铺开到每个标记高度）/
     最大速度点标注 WEP xxxxkm/h at xxxx m —— 随重量参数联动重算） */
  const tasSeries = [];
  let tasAny = false;
  names.forEach((n, i) => {
    // ⚠️ 这个局部变量**不能叫 `t`**：`t` 是本文件里到处在用的 i18n 查表函数，
    // 名字一撞就是 `t is not a function` —— 整张 TAS-高度图连同后面所有图一起白屏
    //（`ensureCharts()` 已建好实例，异常却抛在 setOption 之前）。
    const ta = fmCompare[n].tas_alt;
    if (!ta) return;
    tasAny = true;
    const wep = [], mil = [], marksW = [], marksM = [];
    let maxIdx = -1, maxV = -Infinity;
    for (let k = 0; k < ta.alt.length; k++) {
      const alt = ta.alt[k];
      const vw = ta.tas_wep[k], vm = ta.tas_mil[k];
      if (vw > 0) {
        wep.push([vw, alt]);
        if (vw > maxV) { maxV = vw; maxIdx = k; }
        if (alt % 2000 === 0) {
          // 阻力 = 该高度最大平飞点的推力（T = D）；**每个标记高度都标**，换行跟在速度下面
          const drag = ta.drag_wep && ta.drag_wep[k] > 0
            ? `\n${t("fm.lbl.drag", { v: ta.drag_wep[k].toFixed(0) })}` : "";
          marksW.push({
            value: [vw, alt],
            label: { show: true, position: "right", fontSize: 10, color: "#a05a00",
              lineHeight: 11, formatter: `${vw.toFixed(0)}${drag}` },
          });
        }
      }
      if (vm > 0) {
        mil.push([vm, alt]);
        if (alt % 2000 === 0) {
          const dragM = ta.drag_mil && ta.drag_mil[k] > 0
            ? `\n${t("fm.lbl.drag", { v: ta.drag_mil[k].toFixed(0) })}` : "";
          marksM.push({
            value: [vm, alt],
            label: { show: true, position: "left", fontSize: 10, color: "#1f4e9c",
              lineHeight: 11, formatter: `${vm.toFixed(0)}${dragM}` },
          });
        }
      }
    }
    // 限制线只画首个机型（VNE/MNE 是机型属性，避免图例爆炸）
    if (i === 0) {
      const vneL = [], mneL = [], limL = [];
      for (let k = 0; k < ta.alt.length; k++) {
        vneL.push([ta.vne_tas[k], ta.alt[k]]);
        mneL.push([ta.mne_tas[k], ta.alt[k]]);
        limL.push([Math.min(ta.vne_tas[k], ta.mne_tas[k]), ta.alt[k]]);
      }
      tasSeries.push({ name: t("fm.series.vne", { name: n }), type: "line", showSymbol: false, silent: true,
        data: vneL, lineStyle: { color: "#8a8a8a", width: 1.2, type: "dotted" },
        itemStyle: { color: "#8a8a8a" } });
      tasSeries.push({ name: t("fm.series.mne", { name: n }), type: "line", showSymbol: false, silent: true,
        data: mneL, lineStyle: { color: "#0e7490", width: 1.2, type: "dotted" },
        itemStyle: { color: "#0e7490" } });
      tasSeries.push({ name: t("fm.ax.tas_limit"), type: "line", showSymbol: false,
        silent: true, data: limL, lineStyle: { color: "#333333", width: 1.6 },
        itemStyle: { color: "#333333" } });
    }
    if (wep.length) {
      tasSeries.push({ name: t("fm.series.wep", { name: n }), type: "line", showSymbol: false,
        data: wep, lineStyle: { color: "#ff8c00", width: 1.4 }, itemStyle: { color: "#ff8c00" } });
    }
    if (mil.length) {
      tasSeries.push({ name: t("fm.series.mil", { name: n }), type: "line", showSymbol: false,
        data: mil, lineStyle: { color: "#1f4e9c", width: 1.4, type: "dashed" },
        itemStyle: { color: "#1f4e9c" } });
    }
    if (marksW.length) {
      tasSeries.push({ type: "scatter", data: marksW, symbol: "circle", symbolSize: 5,
        itemStyle: { color: "#e07b00" } });
    }
    if (marksM.length) {
      tasSeries.push({ type: "scatter", data: marksM, symbol: "circle", symbolSize: 4,
        itemStyle: { color: "#1f4e9c" } });
    }
    if (maxIdx >= 0) {
      tasSeries.push({ type: "scatter", symbol: "diamond", symbolSize: 10,
        itemStyle: { color: "#c0392b" },
        data: [{ value: [maxV, ta.alt[maxIdx]],
          label: { show: true, position: "right", fontWeight: "bold", fontSize: 11,
            color: "#c0392b",
            formatter: t("fm.lbl.wep_max", { v: maxV.toFixed(0), alt: ta.alt[maxIdx].toFixed(0) }) } }] });
    }
  });
  if (tasAny) {
    chartTasAlt.setOption({
      backgroundColor: "transparent",
      animation: false,
      tooltip: { trigger: "item" },
      legend: { type: "scroll", textStyle: { color: "#444" } },
      grid: { left: 64, right: 88, top: 30, bottom: 42 },
      xAxis: Object.assign({ type: "value", name: t("fm.ax.tas"), min: 0 }, AXIS_STYLE),
      yAxis: Object.assign({ type: "value", name: t("fm.ax.alt"), min: 0 }, AXIS_STYLE),
      series: tasSeries,
    }, true);
  } else {
    emptyChart(chartTasAlt, t("fm.err.no_tas"));
  }

  /* ⑤⑥ 推力 vs TAS / vs IAS（对照 plot_thrust_altitude；**这是"推力/功率 - 速度"的唯一一张图**：
     原来另有一张「推力 / 功率 - 速度族」画的是同一份数据 —— `speed_kmh` 就是真空速，
     喷气机两张图逐点相同，已按用户要求删掉重复的那张，保留信息量更大的这张）：
     每高度一条线（WEP 实线 / mil 虚线）、线端标高度、VNE(灰)/MNE(青) 竖限制线、
     曲线在限制处截断并插值到边界、IAS 图右缘 = VNE。
     纵轴单位取曲线数据自带的 unit：喷气 = kgf（推力），活塞 = hp（功率）。 */
  const thrustUnit = names.map((n) => (fmCompare[n] || {}).unit).find((u) => typeof u === "string" && u) || "kgf";
  const thrustAxisName = thrustUnit === "hp"
    ? t("fm.ax.power_hp")
    : t("fm.ax.thrust", { unit: thrustUnit });
  const renderThrustClip = (chart, mode) => {
    const series = [];
    let any = false;
    let xMax = 0;
    names.forEach((n, i) => {
      const d = fmCompare[n];
      if (!d.thrust || !d.limits) return;
      const fences = [];
      const altKeys = Object.keys(d.thrust)
        .filter((k) => d.limits[k])
        .sort((a, b) => Number(a) - Number(b));
      altKeys.forEach((altKey) => {
        const alt = Number(altKey);
        const lim = d.limits[altKey];
        const ratio = lim.ratio > 0 ? lim.ratio : 1;
        const conv = (v) => (mode === "ias" ? v * ratio : v);
        // 曲线截断点；轴右缘（含两条竖限制线）—— IAS 图右缘 = VNE
        const xLim = mode === "ias" ? lim.vne_ias : Math.min(lim.vne_tas, lim.mach_tas);
        const xEdge = mode === "ias" ? lim.vne_ias : Math.max(lim.vne_tas, lim.mach_tas);
        if (Number.isFinite(xEdge) && xEdge > xMax) xMax = xEdge;
        const clip = (ys) => {
          const out = [];
          const xs = d.speed_kmh;
          for (let k = 0; k < xs.length; k++) {
            const x = conv(xs[k]);
            if (x > xLim) break;
            out.push([x, ys[k]]);
          }
          // 端点线性插值到限制处（对齐 np.interp 的截断语义）
          if (out.length && out[out.length - 1][0] < xLim &&
              xs.length > 1 && conv(xs[xs.length - 1]) > xLim) {
            let k = 0;
            while (k + 1 < xs.length && conv(xs[k + 1]) < xLim) k++;
            const x0 = conv(xs[k]), x1 = conv(xs[k + 1]);
            const y = x1 > x0 ? ys[k] + (ys[k + 1] - ys[k]) * ((xLim - x0) / (x1 - x0)) : ys[k];
            out.push([xLim, y]);
          }
          return out;
        };
        const c = seriesColor(i);
        const wepData = clip(d.thrust[altKey].wep || []);
        const milData = clip(d.thrust[altKey].mil || []);
        if (wepData.length) {
          any = true;
          const pts = wepData.slice();
          const last = pts[pts.length - 1];
          pts[pts.length - 1] = { value: last,
            label: { show: true, position: "top", fontSize: 10, color: c,
              formatter: t("fm.lbl.km", { n: alt / 1000 }) } };
          series.push({ name: `${n} ${alt / 1000} km`, type: "line", showSymbol: false,
            data: pts, lineStyle: { color: c, width: 1.8 }, itemStyle: { color: c } });
        }
        if (milData.length) {
          any = true;
          series.push({ type: "line", showSymbol: false, data: milData,
            lineStyle: { color: c, width: 1.3, type: "dashed", opacity: 0.75 },
            itemStyle: { color: c, opacity: 0.75 } });
        }
        fences.push({ name: t("fm.lbl.vne_limit"), xAxis: mode === "ias" ? lim.vne_ias : lim.vne_tas,
          lineStyle: { color: "#666666", type: "dashed", width: 1.1 } });
        fences.push({ name: t("fm.lbl.mne_limit"), xAxis: mode === "ias" ? lim.mach_ias : lim.mach_tas,
          lineStyle: { color: "#0e7490", type: "dashed", width: 1.1 } });
      });
      if (fences.length) {
        series.push({ name: t("fm.series.vne_mne", { name: n }), type: "line", showSymbol: false, silent: true,
          data: [],
          markLine: { symbol: "none", silent: true,
            label: { show: true, position: "end", fontSize: 10, formatter: "{b}" },
            data: fences } } );
        any = true;
      }
    });
    if (any) {
      chart.setOption({
        backgroundColor: "transparent",
        animation: false,
        tooltip: { trigger: "axis" },
        legend: { type: "scroll", textStyle: { color: "#444" } },
        grid: { left: 64, right: 24, top: 30, bottom: 42 },
        xAxis: Object.assign({ type: "value", min: 0,
          max: Number.isFinite(xMax) && xMax > 0 ? xMax : undefined,
          name: mode === "ias" ? t("fm.ax.ias") : t("fm.ax.tas") }, AXIS_STYLE),
        yAxis: Object.assign({ type: "value", name: thrustAxisName, min: 0 }, AXIS_STYLE),
        series,
      }, true);
    } else {
      emptyChart(chart, t("fm.err.no_thrust"));
    }
  };
  renderThrustClip(chartThrustTas, "tas");
  renderThrustClip(chartThrustIas, "ias");

  /* ① 功率-高度（对齐 matplotlib 轴标/图例；无活塞数据时提示） */
  if (pwSeries.length) {
    // 极值标注只保留一个：Mil/WEP 最大功率点中**高度更低**者（同高取先出现）
    const pd = names.map((n) => fmCompare[n]).find((x) => x && x.power_alt);
    if (pd && pd.power_alt) {
      const pa = pd.power_alt;
      const argmax = (arr) => {
        let bi = -1, bv = -Infinity;
        (arr || []).forEach((v, i) => { if (v > bv) { bv = v; bi = i; } });
        return bi;
      };
      const mi = argmax(pa.mil);
      const wi = argmax(pa.wep);
      let kind = "Mil", bi = mi;
      if (wi >= 0 && mi >= 0 && pa.alt[wi] < pa.alt[mi]) { kind = "WEP"; bi = wi; }
      if (bi >= 0) {
        const bv = (kind === "Mil" ? pa.mil : pa.wep)[bi];
        const ba = pa.alt[bi];
        pwSeries.push({
          type: "scatter", symbol: "diamond", symbolSize: 9,
          itemStyle: { color: kind === "Mil" ? "#1f4e9c" : "#c0392b" },
          data: [{ value: [bv, ba],
            label: { show: true, position: "right", fontSize: 11, fontWeight: "bold",
              color: "#333",
              formatter: t("fm.lbl.power_max", { kind, v: bv.toFixed(0), alt: ba.toFixed(0) }) } }],
        });
      }
    }
    chartPower.setOption({
      backgroundColor: "transparent",
      animation: false,
      tooltip: { trigger: "axis" },
      legend: { type: "scroll", textStyle: { color: "#444" } },
      grid: { left: 64, right: 20, top: 30, bottom: 42 },
      xAxis: Object.assign({ type: "value", name: t("fm.ax.power_hp"), min: 0 }, AXIS_STYLE),
      yAxis: Object.assign({ type: "value", name: t("fm.ax.alt"), min: 0 }, AXIS_STYLE),
      series: pwSeries,
    }, true);
  } else {
    emptyChart(chartPower, t("fm.err.piston_only"));
  }

  /* ③ 推力 3D 曲面（对齐 matplotlib：Velocity × Altitude → Thrust） */
  if (surfBlocks.length && echartsGlOk()) {
    chart3d.setOption({
      animation: false,
      tooltip: { trigger: "item" },
      visualMap: {
        show: true, dimension: 2, min: zmin, max: zmax, calculable: true,
        inRange: { color: VIRIDIS },
        textStyle: { color: "#666" },
        right: 10, top: 10,
      },
      xAxis3D: Object.assign({ type: "value", name: t("fm.ax.velocity") }, AXIS_STYLE),
      yAxis3D: Object.assign({ type: "value", name: t("fm.ax.alt") }, AXIS_STYLE),
      zAxis3D: Object.assign({ type: "value", name: t("fm.ax.thrust", { unit: zunit }) }, AXIS_STYLE),
      grid3D: Object.assign({}, GRID3D_BASE, { boxWidth: 150, boxDepth: 120 }),
      series: surfBlocks,
    }, true);
  } else {
    emptyChart(chart3d, t("fm.err.jet3d_only"));
  }
}

/* ---------- FM 信息面板：分组卡片（可读性优先 · 短词 · 简化数值 · 文案走 t()） ---------- */

/** 引擎类型的取值（与 `flightmodel` 的 `engine_type_string()` 一一对应，键 = `fm.engine.<值>`）。
 *  上游将来多出一种类型时，`engineName()` 会原样显示那个值，而不是把键名画到界面上。 */
const FM_ENGINE_TYPES = ["Jet", "Piston", "Turboprop", "Rocket", "Unknown"];

async function loadFmInfo(name) {
  const box = $("#fm-info");
  box.innerHTML = `<div class="muted">${esc(t("fm.loading_named", { name }))}</div>`;
  try {
    const d = await api(`/api/fm/${encodeURIComponent(name)}?${fmQuery()}`);
    // 机型信息里也带 `is_jet` → 那三张推力图随活塞/喷气隐藏或显示（与 curves 那条路等价）
    noteFmEngineKind(d.is_jet);

    // 数值简化：固定精度 + 去尾零；非数值/缺值 → —
    const fmt = (v, p = 1) => {
      if (typeof v !== "number" || !Number.isFinite(v)) return "—";
      let s = v.toFixed(p);
      if (s.indexOf(".") >= 0) s = s.replace(/0+$/, "").replace(/\.$/, "");
      return s === "-0" ? "0" : s;
    };
    const txt = (v) => (v === null || v === undefined || v === "" ? "—" : String(v));
    const kv = (k, v) => `<div class="fm-kv"><span>${esc(k)}</span><b>${esc(v)}</b></div>`;
    const kvs = (rows) => `<div class="fm-kvs">${rows.join("")}</div>`;
    const card = (title, inner) =>
      `<section class="fm-card"><h4>${esc(title)}</h4>${inner}</section>`;
    const g2 = (a, b) =>
      typeof a === "number" && typeof b === "number" ? `${fmt(a)} ~ ${fmt(b)} g` : "—";

    const w = d.weight || {};
    const pw = d.power || {};
    const lf = d.load_factor || {};
    const lim = d.limits || {};
    const kg = (v) => fmt(v, 0);
    const cards = [];
    // 引擎类型的显示名走 i18n；后端给了表里没有的值（上游新增类型）就原样显示，不上屏键名
    const engineName = (v) => (FM_ENGINE_TYPES.includes(v) ? t("fm.engine." + v) : txt(v));

    // ① 基本参数
    cards.push(card(t("fm.card.basic"), kvs([
      kv(t("fm.k.aircraft"), txt(d.aircraft)),
      kv(t("fm.k.engine"), engineName(d.engine_type)),
      kv(t("fm.k.wep"), d.has_wep ? t("fm.yes") : t("fm.no")),
      kv(t("fm.k.aoa_crit_clean"),
        typeof d.aoa_crit_no_flaps === "number" ? `${fmt(d.aoa_crit_no_flaps)}°` : "—"),
      kv(t("fm.k.aoa_crit_full"),
        typeof d.aoa_crit_full_flaps === "number" ? `${fmt(d.aoa_crit_full_flaps)}°` : "—"),
    ])));

    // ② 重量（kg，整数）——当前重量随燃油比例/额外重量联动
    fmMaxFuelKg = d.max_fuel || 0;
    cards.push(card(t("fm.card.weight"), kvs([
      kv(t("fm.k.weight_now"), kg(d.takeoff_weight)),
      kv(t("fm.k.fuel_now"), kg(typeof d.fuel_pct === "number" ? fmMaxFuelKg * d.fuel_pct / 100 : 0)),
      kv(t("fm.k.extra_weight"), typeof d.extra_weight === "number" ? `${fmt(d.extra_weight, 0)} kg` : "—"),
      kv(t("fm.k.empty"), kg(w.empty ?? d.empty_weight)),
      kv(t("fm.k.empty_flight"), kg(w.empty_flight ?? d.empty_flight_weight)),
      kv(t("fm.k.max_fuel"), kg(d.max_fuel)),
      kv(t("fm.k.takeoff_half"), kg(d.takeoff_weight_50)),
      kv(t("fm.k.oil"), kg(w.oil)),
      kv(t("fm.k.nitro"), kg(w.nitro)),
      kv(t("fm.k.pilot"), kg(w.pilot)),
      kv(t("fm.k.ammo"), kg(w.ammo)),
      kv(t("fm.k.cm"), kg(w.cm)),
      kv(t("fm.k.external"), kg(w.external)),
    ])));

    // ③ 动力（喷气：推力/推重比；活塞：Mil/WEP 功率 + 两组 T/W）
    const powerRows = d.is_jet
      ? [
          kv(t("fm.k.thrust_max"), `${kg(d.thrust_max)} kgf`),
          kv(t("fm.k.tw"), fmt(pw.tw_mil, 2)),
          ...(typeof d.engine_power === "number" && d.engine_power > 0
            ? [kv(t("fm.k.rated_power"), `${kg(d.engine_power)} hp`)]
            : []),
        ]
      : [
          kv(t("fm.k.mil_power"), `${kg(pw.mil)} hp`),
          kv(t("fm.k.wep_power"), `${kg(pw.wep)} hp`),
          kv(t("fm.k.tw_mil"), fmt(pw.tw_mil, 2)),
          kv(t("fm.k.tw_wep"),
            w.empty > 0 && typeof pw.wep === "number" ? fmt(pw.wep / w.empty, 2) : "—"),
          ...(typeof d.engine_power === "number" && typeof pw.mil === "number" &&
          Math.abs(d.engine_power - pw.mil) > 1
            ? [kv(t("fm.k.rated_power"), `${kg(d.engine_power)} hp`)]
            : []),
        ];
    cards.push(card(t("fm.card.power"), kvs(powerRows)));

    // ④ 飞行限制
    cards.push(card(t("fm.card.limits"), kvs([
      kv(t("fm.k.vne"), typeof lim.vne === "number" ? `${kg(lim.vne)} km/h` : "—"),
      kv(t("fm.k.mne"), typeof lim.vne_mach === "number" ? fmt(lim.vne_mach, 2) : "—"),
      kv(t("fm.k.ias_warn"), typeof lim.ias_warn === "number" ? `${kg(lim.ias_warn)} km/h` : "—"),
      kv(t("fm.k.mach_warn"), typeof lim.mach_warn === "number" ? fmt(lim.mach_warn, 3) : "—"),
      kv(t("fm.k.g_limit"), g2(d.load_limit_neg, d.load_limit_pos)),
      kv(t("fm.k.g_empty"), g2((lf.empty || [])[0], (lf.empty || [])[1])),
      kv(t("fm.k.g_full"), g2((lf.full || [])[0], (lf.full || [])[1])),
    ])));

    // ⑤ 几何
    cards.push(card(t("fm.card.geometry"), kvs([
      kv(t("fm.k.wingspan"), typeof d.wingspan === "number" ? `${fmt(d.wingspan)} m` : "—"),
      kv(t("fm.k.wing_area"), typeof d.wing_area === "number" ? `${fmt(d.wing_area, 2)} m²` : "—"),
      kv(t("fm.k.aspect_ratio"), fmt(d.aspect_ratio, 2)),
    ])));

    // ⑥ 气动特性：每部件一张卡片 + 一张「参数 | 光洁 | 全襟翼」小表
    const parts = d.parts || {};
    const partDefs = [
      [t("fm.part.wing"), parts.wing],
      [t("fm.part.fuselage"), parts.fuselage],
      [t("fm.part.hor_stab"), parts.hor_stab],
    ];
    let partCards = 0;
    for (const [partName, part] of partDefs) {
      if (!part) continue;
      partCards += 1;
      const b = part.base || {};
      const fl = part.flaps || null;
      const pair = (fn) => (fl ? fn(fl) : "—");
      const aoaR = (p) =>
        Array.isArray(p.aoa_crit) ? `${fmt(p.aoa_crit[0])}° ~ ${fmt(p.aoa_crit[1])}°` : "—";
      const clR = (p) =>
        Array.isArray(p.cl_crit) ? `${fmt(p.cl_crit[0], 3)} ~ ${fmt(p.cl_crit[1], 3)}` : "—";
      const bestLd = (p) =>
        Array.isArray(p.best_ld) ? `${fmt(p.best_ld[3])} @ ${fmt(p.best_ld[0])}°` : "—";
      const crit = (p) =>
        Array.isArray(p.crit_neg) && Array.isArray(p.crit_pos)
          ? `${fmt(p.crit_neg[0])}° ~ ${fmt(p.crit_pos[0])}° · L/D ${fmt(p.crit_neg[3])}/${fmt(p.crit_pos[3])}`
          : "—";
      const tr = (k, a, bf) =>
        `<tr><td>${esc(k)}</td><td>${esc(a)}</td><td>${esc(bf)}</td></tr>`;
      const geo = kvs([
        kv(t("fm.k.incidence"), typeof part.angle === "number" ? `${fmt(part.angle)}°` : "—"),
        kv(t("fm.k.area"), typeof part.area === "number" ? `${fmt(part.area, 2)} m²` : "—"),
        kv(t("fm.k.ar"), fmt(part.ar, 2)),
      ]);
      const table = `<table class="fm-table">
        <thead><tr><th>${esc(t("fm.th.param"))}</th><th>${esc(t("fm.th.clean"))}</th><th>${esc(t("fm.th.full"))}</th></tr></thead>
        <tbody>
        ${tr("CdMin", fmt(b.cd_min, 4), pair((p) => fmt(p.cd_min, 4)))}
        ${tr("Cl0", fmt(b.cl0, 3), pair((p) => fmt(p.cl0, 3)))}
        ${tr(t("fm.tr.aoa_crit"), aoaR(b), pair(aoaR))}
        ${tr(t("fm.tr.cl_crit"), clR(b), pair(clR))}
        ${tr(t("fm.tr.cl_slope"), fmt(b.cl_slope, 3), pair((p) => fmt(p.cl_slope, 3)))}
        ${tr(t("fm.tr.oswald"), fmt(b.oswalds, 3), pair((p) => fmt(p.oswalds, 3)))}
        ${tr(t("fm.tr.best_ld"), bestLd(b), pair(bestLd))}
        ${tr(t("fm.tr.stall_margin"), crit(b), pair(crit))}
        </tbody></table>`;
      cards.push(card(t("fm.card.aero", { part: partName }), geo + table));
    }
    if (!partCards) {
      cards.push(card(t("fm.card.aero_generic"), `<div class="muted">${esc(t("fm.parts_none"))}</div>`));
    }

    box.innerHTML = cards.join("");
    return true;
  } catch (e) {
    box.innerHTML = `<div class="msg err">${esc(t("fm.err.load_x", { err: e }))}</div>`;
    notify(t("fm.err.info_x", { err: e }), false);
    return false;
  }
}

const fmPinned = {};   // 「加入对比」固定的机型（切换选择不丢失）
let fmCurrentName = null;

async function addFmCompare(name, replaceAll) {
  try {
    const c = await api(`/api/fm/${encodeURIComponent(name)}/curves?${fmQuery()}`);
    if (replaceAll) {
      // 渲染集 = 固定对比(fmPinned) + 当前机型（与 fmAutoLoad 同一规则）。
      // 不能"全清"：拖一下油量/额外重量就会把已加入对比的机型清掉。
      for (const k of Object.keys(fmCompare)) {
        if (k !== name && !(k in fmPinned)) delete fmCompare[k];
      }
    }
    fmCompare[name] = c;
    renderCurves();
    return true;
  } catch (e) {
    report(t("fm.err.curves_x", { err: e }), false);
    return false;
  }
}

/** 按当前参数（燃油比例 / 额外重量）重取「已固定对比」机型的曲线。
 *  fmPinned 存的是 pin 那一刻的 JSON 快照，刷新时只重算当前机型的话，同一张图上
 *  A 是 50% 油、B 还是 80% 油的旧曲线 —— 图例与单位都不体现，对比结论静默失真。
 *  重取失败的机型直接移出对比并回报：留着旧曲线等于继续展示错参数。 */
async function refetchFmPinned() {
  const failed = [];
  for (const n of Object.keys(fmPinned)) {
    try {
      fmPinned[n] = await api(`/api/fm/${encodeURIComponent(n)}/curves?${fmQuery()}`);
      fmCompare[n] = fmPinned[n];
    } catch (e) {
      delete fmPinned[n];
      delete fmCompare[n];
      failed.push(`${n}: ${e}`);
    }
  }
  return failed;
}

async function fmAutoLoad(name) {
  if (!name) return;
  const info = document.getElementById("fm-info");
  if (info) chartPlaceholder(info, t("fm.loading_named", { name }), true);
  fmEachChart((node) => chartPlaceholder(node, t("fm.loading"), true));
  await loadFmInfo(name);
  try {
    const c = await api(`/api/fm/${encodeURIComponent(name)}/curves?${fmQuery()}`);
    fmCurrentName = name;
    // 渲染集 = 固定对比(fmPinned) + 当前机型；切换机型不清空已固定的对比
    for (const k of Object.keys(fmCompare)) {
      if (!(k in fmPinned)) delete fmCompare[k];
    }
    fmCompare[name] = c;
    renderCurves();
  } catch (e) {
    report(t("fm.err.curves_x", { err: e }), false);
    fmEachChart((node) => chartPlaceholder(node, t("fm.err.curves")));
  }
}
$("#fm-compare").addEventListener("click", () => {
  const name = fmCurrentName || currentFmName();
  if (!name || !(name in fmCompare)) { report(t("fm.pick_aircraft"), false); return; }
  fmPinned[name] = fmCompare[name];
  report(t("fm.compare_added", { list: Object.keys(fmPinned).join(", ") }), true, "fm-compare");
});
$("#fm-compare-clear").addEventListener("click", () => {
  for (const k of Object.keys(fmPinned)) delete fmPinned[k];
  for (const k of Object.keys(fmCompare)) {
    if (k !== fmCurrentName) delete fmCompare[k];
  }
  renderCurves();
  report(t("fm.compare_cleared"), true, "fm-compare");
});

/** 语言切换后重画本页由 JS 生成的文案（机型信息卡片 + 图表轴名/图例/空图提示 + 版本行）。
 *
 *  配置器那一半由 `app.js` 的 `setUiLang()` 重渲染 `renderForm()`；FM 页签这一半在这里：
 *  （切语言后卡片与轴名一直停在旧语言）。`app.js` 用 `typeof fmOnLangChange === "function"`
 *  的可选全局方式调它 —— 两个 classic script 之间不引入硬依赖。 */
function fmOnLangChange() {
  const name = fmCurrentName || currentFmName();
  if (name) loadFmInfo(name);                              // 卡片是异步的，不阻塞切语言
  if (Object.keys(fmCompare).length) {
    try { renderCurves(); } catch (e) { /* 图表可能还没 init（页签没打开过），忽略 */ }
  }
  renderFmChoices();                                       // 下拉的 title（"共 N 个机型"）
  loadFmVersion();
  // 更新面板也在飞行模型页签里，文案同样是 JS 生成的（`data-up` 不是 `data-i18n`）——
  // 不刷新的话要等下一次轮询（≤1.5s）才跟上语言。
  if (typeof upOnLangChange === "function") upOnLangChange();
}
