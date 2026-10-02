#!/usr/bin/env python3
"""
Plot flight model curves for War Thunder aircraft.
- Piston engine: Power vs Altitude
- Jet engine: Thrust vs Velocity at different altitudes
"""

import argparse
import sys
import os

try:
    import matplotlib
    matplotlib.use('Agg')  # Use non-interactive backend
    import matplotlib.pyplot as plt
    import numpy as np
except ImportError:
    print("Error: matplotlib and numpy are required.")
    print("Install with: pip install matplotlib numpy")
    sys.exit(1)

# 中文字形支持：使用项目自带 Sarasa 字体（缺失则退回 matplotlib 自带的 DejaVu，与本项目 HUD 的字体逻辑无关）
# 本脚本在 flightmodel/scripts/ 下 → 回退两级到仓库根，再进 resource/fonts/
# （回退一级会指向不存在的 flightmodel/fonts/，于是永远静默用 DejaVu）
_cjk_font = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                         '..', '..', 'resource', 'fonts', 'SarasaMonoSC-Regular.ttf')
if os.path.exists(_cjk_font):
    from matplotlib import font_manager
    font_manager.fontManager.addfont(_cjk_font)
    plt.rcParams['font.family'] = ['Sarasa Mono SC', 'DejaVu Sans']
    _cjk_font_prop = font_manager.FontProperties(family='Sarasa Mono SC')
else:
    _cjk_font_prop = None


def parse_power_data(filepath):
    """Parse piston engine power-altitude data. Returns (altitudes, powers, wep_powers)."""
    altitudes = []
    powers = []
    wep_powers = []
    
    with open(filepath, 'r') as f:
        lines = f.readlines()
    
    # Check if WEP data is present
    has_wep = 'wep' in lines[0].lower() if lines else False
    
    for line in lines[1:]:  # Skip header
        parts = line.strip().split('\t')
        if len(parts) >= 2:
            try:
                alt = float(parts[0])
                power = float(parts[1])
                altitudes.append(alt)
                powers.append(power)
                
                if has_wep and len(parts) >= 3:
                    try:
                        wep_powers.append(float(parts[2]))
                    except ValueError:
                        wep_powers.append(0.0)
                else:
                    wep_powers.append(0.0)
            except ValueError:
                continue
    
    return altitudes, powers, wep_powers


def parse_thrust_data(filepath):
    """Parse jet engine thrust-altitude-velocity data. Returns (velocities, altitudes, thrust_data, wep_data)."""
    velocities = []
    altitudes = []
    thrust_data = []
    wep_data = []
    
    with open(filepath, 'r') as f:
        lines = f.readlines()
    
    if not lines:
        return [], [], [], []
    
    # Parse header (first line)
    header_parts = lines[0].strip().split('\t')
    # Filter out WEP columns (those with 'wep' in name)
    alt_headers = []
    wep_headers = []
    for h in header_parts[1:]:
        if 'wep' in h.lower():
            wep_headers.append(h)
        else:
            alt_headers.append(h)
    
    altitudes = [float(alt.replace('m', '').replace('_wep', '')) for alt in alt_headers]
    num_altitudes = len(altitudes)
    
    # Parse data lines
    for line in lines[1:]:
        parts = line.strip().split('\t')
        if len(parts) >= 2:
            try:
                vel = float(parts[0])
                velocities.append(vel)
                
                # Split thrust data into normal and WEP
                all_thrusts = []
                for thrust_str in parts[1:]:
                    try:
                        all_thrusts.append(float(thrust_str))
                    except ValueError:
                        all_thrusts.append(0.0)
                
                # First half is normal thrust, second half is WEP
                thrust_data.append(all_thrusts[:num_altitudes])
                wep_data.append(all_thrusts[num_altitudes:2*num_altitudes] if len(all_thrusts) > num_altitudes else [])
            except ValueError:
                continue
    
    return velocities, altitudes, thrust_data, wep_data


def plot_piston_power(altitudes, powers, wep_powers, output_path, aircraft_name):
    """Plot power vs altitude for piston engine."""
    plt.figure(figsize=(10, 6))
    
    plt.plot(powers, altitudes, 'b-', linewidth=2, label='Normal Power')
    plt.fill_betweenx(altitudes, 0, powers, alpha=0.3, color='blue')
    
    # Plot WEP if available
    has_wep = any(w > 0 for w in wep_powers)
    if has_wep:
        plt.plot(wep_powers, altitudes, 'r-', linewidth=2, label='WEP (Emergency)')
        plt.fill_betweenx(altitudes, 0, wep_powers, alpha=0.2, color='red')
    
    plt.xlabel('Power (hp)', fontsize=12)
    plt.ylabel('Altitude (m)', fontsize=12)
    plt.title(f'{aircraft_name} - Power vs Altitude (Piston Engine)', fontsize=14)
    plt.grid(True, alpha=0.3)
    plt.xlim(left=0)
    plt.legend(loc='upper right')
    
    # Add annotations
    max_power = max(powers) if powers else 0
    max_alt = altitudes[powers.index(max_power)] if powers else 0
    plt.annotate(f'Normal: {max_power:.0f} hp at {max_alt:.0f}m', 
                 xy=(max_power, max_alt),
                 xytext=(max_power * 0.7, max_alt + 2000),
                 arrowprops=dict(arrowstyle='->', color='blue'),
                 fontsize=10, color='blue')
    
    if has_wep:
        max_wep = max(wep_powers) if wep_powers else 0
        max_wep_alt = altitudes[wep_powers.index(max_wep)] if wep_powers else 0
        plt.annotate(f'WEP: {max_wep:.0f} hp at {max_wep_alt:.0f}m', 
                     xy=(max_wep, max_wep_alt),
                     xytext=(max_wep * 0.7, max_wep_alt + 1500),
                     arrowprops=dict(arrowstyle='->', color='red'),
                     fontsize=10, color='red')
    
    plt.tight_layout()
    plt.savefig(output_path, dpi=150)
    print(f"Plot saved to: {output_path}")
    plt.close()


def plot_jet_thrust(velocities, altitudes, thrust_data, wep_data, output_path, aircraft_name):
    """Plot thrust vs velocity at different altitudes for jet engine."""
    plt.figure(figsize=(12, 8))
    
    # Select a subset of altitudes to display (avoid too many lines)
    num_altitudes = len(altitudes)
    if num_altitudes > 5:
        step = num_altitudes // 5
        indices = list(range(0, num_altitudes, step))
    else:
        indices = list(range(num_altitudes))
    
    has_wep = any(len(row) > 0 and any(w > 0 for w in row) for row in wep_data)
    
    colors = plt.cm.viridis(np.linspace(0, 1, len(indices)))
    
    # Plot normal thrust (solid lines)
    for i, alt_idx in enumerate(indices):
        thrusts = [row[alt_idx] if alt_idx < len(row) else 0 for row in thrust_data]
        plt.plot(velocities, thrusts, color=colors[i], linewidth=2, linestyle='-',
                 label=f'{altitudes[alt_idx]:.0f}m')
    
    # Plot WEP thrust (dashed lines) if available
    if has_wep:
        for i, alt_idx in enumerate(indices):
            wep_thrusts = [row[alt_idx] if alt_idx < len(row) else 0 for row in wep_data]
            if any(w > 0 for w in wep_thrusts):
                plt.plot(velocities, wep_thrusts, color=colors[i], linewidth=2, linestyle='--',
                         label=f'{altitudes[alt_idx]:.0f}m WEP')
    
    plt.xlabel('Velocity (km/h)', fontsize=12)
    plt.ylabel('Thrust (kgf)', fontsize=12)
    plt.title(f'{aircraft_name} - Thrust vs Velocity (Jet Engine)', fontsize=14)
    plt.legend(title='Altitude', loc='upper right')
    plt.grid(True, alpha=0.3)
    plt.xlim(left=0)
    plt.ylim(bottom=0)
    
    plt.tight_layout()
    plt.savefig(output_path, dpi=150)
    print(f"Plot saved to: {output_path}")
    plt.close()


def plot_jet_thrust_3d(velocities, altitudes, thrust_data, output_path, aircraft_name):
    """Plot 3D surface of thrust vs velocity and altitude for jet engine."""
    from mpl_toolkits.mplot3d import Axes3D
    
    fig = plt.figure(figsize=(12, 9))
    ax = fig.add_subplot(111, projection='3d')
    
    # Create meshgrid
    V, A = np.meshgrid(velocities, altitudes)
    Z = np.array(thrust_data).T
    
    surf = ax.plot_surface(V, A, Z, cmap='viridis', alpha=0.8, edgecolor='none')
    
    ax.set_xlabel('Velocity (km/h)', fontsize=10)
    ax.set_ylabel('Altitude (m)', fontsize=10)
    ax.set_zlabel('Thrust (kgf)', fontsize=10)
    ax.set_title(f'{aircraft_name} - Thrust vs Velocity & Altitude', fontsize=12)
    
    fig.colorbar(surf, shrink=0.5, aspect=10, label='Thrust (kgf)')
    
    plt.tight_layout()
    plt.savefig(output_path, dpi=150)
    print(f"3D Plot saved to: {output_path}")
    plt.close()


def parse_em_grid(filepath):
    """Parse EM grid TSV -> (dict[alt], meta).
    Columns: alt_m, V_ias_kmh, V_tas_kmh, omega_dps, Ps_wep_mps, Ps_mil_mps."""
    grid = {}
    meta = {'weight': None}
    with open(filepath, 'r') as f:
        for line in f:
            line = line.strip()
            if line.startswith('#'):
                if 'weight=' in line:
                    try:
                        meta['weight'] = float(line.split('weight=')[1].split('kg')[0].strip())
                    except (ValueError, IndexError):
                        pass
                continue
            if not line or line.startswith('alt_m'):
                continue
            p = line.split('\t')
            alt = float(p[0])
            g = grid.setdefault(alt, {'w': [], 'v': [], 'tas': [], 'ps': [], 'ps_mil': []})
            g['v'].append(float(p[1]))      # 表速
            g['tas'].append(float(p[2]))    # 真速（求 ρ 比例用）
            g['w'].append(float(p[3]))
            g['ps'].append(float(p[4]))
            g['ps_mil'].append(float(p[5]))
    return grid, meta


def parse_em_curves(filepath):
    """Columns: alt_m, V_ias_kmh, V_tas_kmh, n_av, n_sus_wep, omega_inst_dps,
    omega_sus_wep_dps, omega_sus_mil_dps, Ps_level_wep_mps."""
    curves = {}
    with open(filepath, 'r') as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith('#') or line.startswith('alt_m'):
                continue
            p = line.split('\t')
            alt = float(p[0])
            c = curves.setdefault(alt, {'V': [], 'n_av': [], 'n_sus': [], 'w_inst': [],
                                        'w_sus': [], 'w_sus_mil': []})
            c['V'].append(float(p[1]))       # 表速
            c['n_av'].append(float(p[3]))
            c['n_sus'].append(float(p[4]))
            c['w_inst'].append(float(p[5]))
            c['w_sus'].append(float(p[6]))
            c['w_sus_mil'].append(float(p[7]))
    return curves


def parse_em_summary(filepath):
    """Columns: alt_m, corner_ias_kmh, corner_tas_kmh, max_instant_dps, max_sus_wep_dps,
    max_level_ias_kmh_wep, max_level_tas_kmh_wep, max_level_tas_kmh_mil, vne_ias_kmh."""
    summary = {}
    with open(filepath, 'r') as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith('#') or line.startswith('alt_m'):
                continue
            p = line.split('\t')
            summary[float(p[0])] = {'corner_ias': float(p[1]), 'vne': float(p[8]),
                                    'max_tas_wep': float(p[6]), 'max_tas_mil': float(p[7])}
    return summary


def plot_em(grid, curves, summary, output_base, aircraft_name, weight_kg=None):
    """EM 能量机动图：X=表速(km/h)（右边界=最大允许表速 VNE），Y=盘旋率(deg/s)，
    Ps 平滑色带 + 瞬时/持续盘旋包线 + 瞬盘/稳盘角点标注（含过载）+ 等过载线。"""
    from matplotlib.colors import LinearSegmentedColormap
    # 柔和配色：负 Ps 暖红 → 0 近白 → 正 Ps 冷绿
    ps_cmap = LinearSegmentedColormap.from_list(
        'ps', ['#8b1a1a', '#c0392b', '#e8a87c', '#f3ddc2', '#f7f7f5',
               '#b9e3a8', '#6fbf73', '#2e8b57', '#145a32'])

    for alt in sorted(grid):
        g = grid[alt]
        fig, ax = plt.subplots(figsize=(10, 7))
        vm = 400.0
        tcf = ax.tripcolor(g['v'], g['w'], g['ps'], shading='gouraud',
                           cmap=ps_cmap, vmin=-vm, vmax=vm)
        fig.colorbar(tcf, ax=ax, label='Ps WEP (m/s)')

        if alt in curves:
            c = curves[alt]
            ax.plot(c['V'], c['w_inst'], '-', color='#1f4e9c', lw=2, label='instant (max)')
            ax.plot(c['V'], c['w_sus'], '--', color='#ff8c00', lw=2.2, label='sustained WEP')
            ax.plot(c['V'], c['w_sus_mil'], ':', color='#8e44ad', lw=2, label='sustained mil')

            # 瞬盘/稳盘角点速度：曲线峰值处用文字标签标出（含过载）
            def mark_peak(xs, ys, ns, color, name, dy):
                if not ys:
                    return
                i = max(range(len(ys)), key=lambda k: ys[k])
                ax.plot([xs[i]], [ys[i]], 'o', ms=5, color=color, zorder=5)
                label = f'{name} {xs[i]:.0f} km/h\n{ys[i]:.1f}°/s  {ns[i]:.1f}G'
                print(f"LABEL@{int(alt)}: [{label.replace(chr(10), ' | ')}]")
                ax.annotate(label,
                            (xs[i], ys[i]), textcoords='offset points',
                            xytext=(8, dy), fontsize=9, color=color,
                            fontproperties=_cjk_font_prop,
                            bbox=dict(facecolor='white', alpha=0.85, edgecolor=color, lw=0.8))

            mark_peak(c['V'], c['w_inst'], c['n_av'], '#1f4e9c', 'INST', -44)
            mark_peak(c['V'], c['w_sus'], c['n_sus'], '#e07b00', 'SUS', -48)

        # 等过载线（常值 n）：ω = g·√(n²−1)/V_tas；横轴为表速 → 需乘 √(ρ/ρ0)
        if g['tas'] and g['tas'][0] > 0:
            ias_per_tas = g['v'][0] / g['tas'][0]
        else:
            ias_per_tas = 1.0
        x_left = min(g['v']) if g['v'] else 0.0
        vne = summary.get(alt, {}).get('vne') if summary else None
        data_max = max(g['v']) if g['v'] else 3000.0
        # 右边界 = min(VNE, 数据范围)：不越过最大允许表速，也不留空白
        x_right = min(vne, data_max) if vne else data_max
        vs_line = np.linspace(max(x_left, 50.0), x_right, 240)
        for n in (2, 3, 4, 6, 8, 12):
            w_line = np.degrees(9.81 * np.sqrt(n * n - 1.0) * ias_per_tas / (vs_line / 3.6))
            ax.plot(vs_line, w_line, color='dimgray', lw=0.8, ls=(0, (2, 2)), alpha=0.7,
                    zorder=1)
            for i in range(len(vs_line)):
                if w_line[i] <= 28.5:
                    ax.annotate(f'{n}G', (vs_line[i], w_line[i]),
                                textcoords='offset points', xytext=(4, 4),
                                fontsize=8, color='dimgray')
                    break

        # 右侧边界 = 最大允许表速 (VNE)
        if vne and vne <= x_right + 1e-6:
            ax.axvline(vne, color='#333333', lw=1.6, alpha=0.85)
            ax.annotate(f'VNE {vne:.0f}', (vne, 31), textcoords='offset points',
                        xytext=(-6, -12), ha='right', fontsize=9, color='#333333')

        ax.set_xlim(x_left, x_right)
        ax.set_xlabel('IAS (km/h)')
        ax.set_ylabel('Turn rate (deg/s)')
        ax.set_ylim(0, 33)   # 与 Ps 网格采样范围一致，边界内无空白
        if weight_kg:
            ax.set_title(f'{aircraft_name} - EM diagram @ {alt:.0f} m ({weight_kg:.0f} kg, 50% fuel)')
        else:
            ax.set_title(f'{aircraft_name} - EM diagram @ {alt:.0f} m (50% fuel)')
        ax.grid(True, color='k', alpha=0.12, lw=0.5, zorder=6)
        ax.legend(loc='upper right')
        out = f"{output_base}_{int(alt)}m.png"
        fig.savefig(out, dpi=150, bbox_inches='tight')
        print(f"VERIFY alt={int(alt)}: xlabel=[{ax.get_xlabel()}] title=[{ax.get_title()}] xlim=[{ax.get_xlim()}] ylim=[{ax.get_ylim()}]")
        plt.close(fig)
        print(f"EM plot saved: {out}")


def mark_series_max(ax, xs, ys, values, others, color, name, unit='', at_unit='',
                    dx=6, dy=-12, fontsize=8):
    """在曲线极大值点打标记，标签格式：`名称 数值单位 at 另一维坐标`。
    xs/ys 为点坐标，values 为求极大的量（None 表示断线段），others 为另一维坐标值。"""
    best_i = None
    best_v = None
    for i, v in enumerate(values):
        if v is not None and (best_v is None or v > best_v):
            best_v = v
            best_i = i
    if best_i is None:
        return
    x, y = xs[best_i], ys[best_i]
    at = others[best_i] if best_i < len(others) else 0.0
    ax.plot([x], [y], 'D', ms=6, color=color, zorder=6)
    ax.annotate(f'{name} {best_v:.0f}{unit} at {at:.0f}{at_unit}', (x, y),
                textcoords='offset points',
                xytext=(dx, dy), fontsize=fontsize, color=color,
                bbox=dict(facecolor='white', alpha=0.85, edgecolor=color, lw=0.7))
    print(f"MAX_MARK [{name}] = {best_v:.0f}{unit} at {at:.0f}{at_unit}")


def parse_thrust_grid(filepath):
    """Columns: alt_m, V_tas_kmh, V_ias_kmh, thrust_wep_kgf, thrust_mil_kgf
    -> dict[alt] = {tas, ias, wep, mil}."""
    data = {}
    with open(filepath, 'r') as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith('#') or line.startswith('alt_m'):
                continue
            p = line.split('\t')
            alt = float(p[0])
            d = data.setdefault(alt, {'tas': [], 'ias': [], 'wep': [], 'mil': []})
            d['tas'].append(float(p[1]))
            d['ias'].append(float(p[2]))
            d['wep'].append(float(p[3]))
            d['mil'].append(float(p[4]))
    return data


def parse_tas_alt(filepath):
    """Columns: alt_m, tas_wep_kmh, tas_mil_kmh, vne_tas_kmh, mne_tas_kmh,
    drag_wep_kgf, drag_mil_kgf -> list of rows."""
    rows = []
    with open(filepath, 'r') as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith('#') or line.startswith('alt_m'):
                continue
            p = line.split('\t')
            rows.append({'alt': float(p[0]), 'tas_wep': float(p[1]) or None,
                         'tas_mil': float(p[2]) or None, 'vne_tas': float(p[3]),
                         'mne_tas': float(p[4]) if len(p) > 4 else 0.0,
                         'drag_wep': float(p[5]) if len(p) > 5 else 0.0,
                         'drag_mil': float(p[6]) if len(p) > 6 else 0.0})
    return rows


def parse_limits(filepath):
    """Columns: alt_m, vne_tas_kmh, vne_ias_kmh, mach_tas_kmh, mach_ias_kmh
    -> dict[alt] = {...}（推力图速度裁剪用）"""
    limits = {}
    with open(filepath, 'r') as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith('#') or line.startswith('alt_m'):
                continue
            p = line.split('\t')
            limits[float(p[0])] = {'vne_tas': float(p[1]), 'vne_ias': float(p[2]),
                                   'mach_tas': float(p[3]), 'mach_ias': float(p[4])}
    return limits


def plot_thrust_altitude(thrust, limits, output_base, aircraft_name, weight_kg=None, speed_key='tas'):
    """推力-速度-高度曲线：X=真速/表速(km/h)，Y=推力(kgf)；每高度一条，
    WEP 实线 / 军推虚线，线端标注高度。
    速度轴受限制裁剪：IAS 受 VNE 限制；TAS 受 min(VNE/MNE→TAS) 限制。"""
    palette = ['#1f4e9c', '#2e8b57', '#c0392b', '#8e44ad', '#e07b00', '#0e7490', '#6b4c9a']
    speed_name = 'TAS' if speed_key == 'tas' else 'IAS'
    fig, ax = plt.subplots(figsize=(10, 7))
    x_ends = []
    fence_x = []
    for idx, alt in enumerate(sorted(thrust)):
        d = thrust[alt]
        color = palette[idx % len(palette)]
        # 每高度速度限制：IAS → VNE；TAS → min(VNE/MNE 换算成 TAS)
        lim = limits.get(alt, {}) if limits else {}
        if speed_key == 'ias':
            x_max = lim.get('vne_ias')
        else:
            cands = [v for v in (lim.get('vne_tas'), lim.get('mach_tas')) if v and v > 0]
            x_max = min(cands) if cands else None

        def clip(xs, ys):
            ox, oy = [], []
            for x, y in zip(xs, ys):
                if x_max is not None and x > x_max:
                    break
                ox.append(x)
                oy.append(y)
            if x_max is not None and ox and ox[-1] < x_max <= xs[-1]:
                ox.append(x_max)
                oy.append(float(np.interp(x_max, xs, ys)))
            return ox, oy

        wx, wy = clip(d[speed_key], d['wep'])
        mx, my = clip(d[speed_key], d['mil'])
        ax.plot(wx, wy, '-', color=color, lw=2, label=f"WEP {alt / 1000:.0f} km")
        ax.plot(mx, my, '--', color=color, lw=1.5, alpha=0.75)
        if wx:
            x_ends.append(wx[-1])
            ax.annotate(f"{alt / 1000:.0f} km", (wx[-1], wy[-1]),
                        textcoords='offset points', xytext=(4, -6),
                        fontsize=8, color=color)
        # 每条曲线的极大值点（`名称 数值单位 at 速度坐标`）
        mark_series_max(ax, wx, wy, wy, wx, color, 'WEP', 'kgf', 'km/h', dx=6, dy=8, fontsize=7)
        mark_series_max(ax, mx, my, my, mx, color, 'MIL', 'kgf', 'km/h', dx=6, dy=-14, fontsize=7)

        # VNE/MNE 限制边界：竖虚线（灰=VNE、青=MNE），曲线止于两者较小值
        spd = d[speed_key]
        v_lim = lim.get('vne_tas' if speed_key == 'tas' else 'vne_ias') or 0.0
        m_lim = lim.get('mach_tas' if speed_key == 'tas' else 'mach_ias') or 0.0
        for xx, c, tag in ((v_lim, '#666666', 'VNE'), (m_lim, '#0e7490', 'MNE')):
            if xx <= 0 or not spd:
                continue
            x_end = min(xx, spd[-1])
            ytop = float(np.interp(x_end, spd, d['wep']))
            ax.plot([xx, xx], [0.0, ytop], ls=(0, (2, 2)), color=c, lw=1.1, alpha=0.9,
                    label=(f'{tag} limit' if idx == 0 else None))
            ax.annotate(tag, (xx, ytop), textcoords='offset points', xytext=(-2, 4),
                        fontsize=7, color=c)
            fence_x.append(xx)
        print(f"FENCE@{alt / 1000:.0f}km {speed_name}: VNE={v_lim:.0f} MNE={m_lim:.0f}")
    ax.set_xlabel(f'{speed_name} (km/h)')
    ax.set_ylabel('Thrust (kgf)')
    x_right = max(x_ends + fence_x) if (x_ends or fence_x) else None
    if speed_key == 'ias':
        # IAS 图右缘即 VNE（最大允许表速），不越过 VNE 画速度轴
        vne_vals = [(limits.get(a, {}) or {}).get('vne_ias') or 0.0 for a in sorted(thrust)]
        vne_max = max(vne_vals) if any(vne_vals) else 0.0
        if vne_max > 0:
            x_right = min(x_right, vne_max) if x_right else vne_max
    ax.set_xlim(left=0, right=x_right)  # 从 0 开始，右缘为限制边界
    ax.set_ylim(bottom=0) # 推力轴从 0 开始
    if weight_kg:
        ax.set_title(f'{aircraft_name} - Thrust vs {speed_name} ({weight_kg:.0f} kg, 50% fuel); solid=WEP dashed=mil')
    else:
        ax.set_title(f'{aircraft_name} - Thrust vs {speed_name}; solid=WEP dashed=mil')
    ax.grid(True, color='k', alpha=0.12, lw=0.5)
    # 图例放在坐标轴外侧（右侧）
    ax.legend(loc='upper left', bbox_to_anchor=(1.02, 1.0), borderaxespad=0, fontsize=8)
    out = f"{output_base}_thrust_{speed_key}.png"
    fig.savefig(out, dpi=150, bbox_inches='tight')
    print(f"VERIFY-{speed_name}: xlim={ax.get_xlim()} ylim={ax.get_ylim()}")
    plt.close(fig)
    print(f"Thrust plot saved: {out}")


def plot_tas_altitude(rows, output_base, aircraft_name, weight_kg=None):
    """真空速-高度曲线（最大平飞）：细线 + 每 2000m 一个速度标记；
    VNE（表速限制）换算成真空速曲线（随空气密度变化）。"""
    alts = [r['alt'] for r in rows]
    tas_wep = [r['tas_wep'] for r in rows]
    tas_mil = [r['tas_mil'] for r in rows]
    vne_tas = [r['vne_tas'] for r in rows]

    fig, ax = plt.subplots(figsize=(10, 7))
    # VNE/MNE 各自换算成真空速曲线；有效限制取两者较小值
    mne_tas = [r['mne_tas'] for r in rows]
    lim_tas = [min(v, m) for v, m in zip(vne_tas, mne_tas)]
    print(f"VERIFY-MNE-TAS: first={mne_tas[0]:.0f} last={mne_tas[-1]:.0f} "
          f"vne_first={vne_tas[0]:.0f} limit_first={lim_tas[0]:.0f}")
    ax.plot(vne_tas, alts, ':', color='#8a8a8a', lw=1.2, label='VNE (as TAS)')
    ax.plot(mne_tas, alts, ':', color='#0e7490', lw=1.2, label='MNE (as TAS)')
    ax.plot(lim_tas, alts, '-', color='#333333', lw=1.6, label='TAS limit = min(VNE, MNE)')
    ax.plot(tas_wep, alts, '-', color='#ff8c00', lw=1.4, label='max level TAS (WEP)')
    ax.plot(tas_mil, alts, '--', color='#1f4e9c', lw=1.4, label='max level TAS (mil)')

    # 速度标记：每 2000m 一个（数据为 100m 采样，仅在 2000m 整数高度标记）
    for r in rows:
        if r['alt'] % 2000.0 != 0.0:
            continue
        if r['tas_wep']:
            ax.plot([r['tas_wep']], [r['alt']], 'o', ms=4, color='#e07b00')
            ax.annotate(f"{r['tas_wep']:.0f}", (r['tas_wep'], r['alt']),
                        textcoords='offset points', xytext=(6, -10),
                        fontsize=8, color='#a05a00')
        if r['tas_mil']:
            ax.plot([r['tas_mil']], [r['alt']], 'o', ms=3, color='#1f4e9c')
            ax.annotate(f"{r['tas_mil']:.0f}", (r['tas_mil'], r['alt']),
                        textcoords='offset points', xytext=(-30, 4),
                        fontsize=8, color='#1f4e9c')
        # 高度 2000m 标记点同时标注阻力（最大平飞点 T=D）
        if r['alt'] == 2000.0:
            if r['tas_wep'] and r['drag_wep']:
                ax.annotate(f"D {r['drag_wep']:.0f}kgf", (r['tas_wep'], r['alt']),
                            textcoords='offset points', xytext=(6, -24),
                            fontsize=7, color='#7a4a00')
            if r['tas_mil'] and r['drag_mil']:
                ax.annotate(f"D {r['drag_mil']:.0f}kgf", (r['tas_mil'], r['alt']),
                            textcoords='offset points', xytext=(-56, -12),
                            fontsize=7, color='#1f4e9c')

    # 每条曲线的极大值点（`名称 数值单位 at 高度坐标`）
    mark_series_max(ax, tas_wep, alts, tas_wep, alts, '#c0392b', 'WEP', 'km/h', 'm', dx=-16, dy=10)
    mark_series_max(ax, tas_mil, alts, tas_mil, alts, '#1f4e9c', 'MIL', 'km/h', 'm', dx=10, dy=4)

    ax.set_xlabel('TAS (km/h)')
    ax.set_ylabel('Altitude (m)')
    ax.set_xlim(left=0)   # 速度轴从 0 开始，不允许负速度
    ax.set_ylim(bottom=0) # 高度轴从 0 开始，不允许负高度
    if weight_kg:
        ax.set_title(f'{aircraft_name} - Max level TAS vs Altitude ({weight_kg:.0f} kg, 50% fuel)')
    else:
        ax.set_title(f'{aircraft_name} - Max level TAS vs Altitude')
    ax.grid(True, color='k', alpha=0.12, lw=0.5)
    ax.legend(loc='lower right')
    out = f"{output_base}_tas_alt.png"
    fig.savefig(out, dpi=150, bbox_inches='tight')
    print(f"VERIFY-TASALT: xlim={ax.get_xlim()} ylim={ax.get_ylim()}")
    plt.close(fig)
    print(f"TAS-altitude plot saved: {out}")


def main():
    parser = argparse.ArgumentParser(description='Plot flight model curves')
    parser.add_argument('--input', '-i', required=True, help='Input data file')
    parser.add_argument('--output', '-o', help='Output plot file (default: <input>.png)')
    parser.add_argument('--type', '-t', choices=['auto', 'piston', 'jet'], default='auto',
                        help='Data type (auto-detect from file content)')
    parser.add_argument('--name', '-n', help='Aircraft name for title')
    parser.add_argument('--3d', action='store_true', help='Generate 3D plot for jet engines')
    
    args = parser.parse_args()
    
    input_file = args.input
    if not os.path.exists(input_file):
        print(f"Error: Input file not found: {input_file}")
        sys.exit(1)
    
    # Determine output file
    if args.output:
        output_file = args.output
    else:
        base, _ = os.path.splitext(input_file)
        output_file = f"{base}.png"
    
    # Determine aircraft name
    aircraft_name = args.name or os.path.basename(base)
    
    # Parse data and determine type - check column header (skip '#' comments)
    with open(input_file, 'r') as f:
        header = ''
        for line in f:
            s = line.strip().lower()
            if s and not s.startswith('#'):
                header = s
                break
    
    if 'omega' in header:
        # EM 能量机动网格（需配套 *_curves.txt / *_summary.txt）
        grid, meta = parse_em_grid(input_file)
        base_name = os.path.splitext(input_file)[0]
        curves_path = f"{base_name}_curves.txt"
        summary_path = f"{base_name}_summary.txt"
        curves = parse_em_curves(curves_path) if os.path.exists(curves_path) else {}
        summary = parse_em_summary(summary_path) if os.path.exists(summary_path) else {}
        if grid:
            print(f"Detected EM grid data: {len(grid)} altitude(s), curves: {bool(curves)}")
            plot_em(grid, curves, summary, base_name, aircraft_name, meta.get('weight'))
            # 附带图表：推力-真速度-高度、推力-表速-高度、真空速-高度
            thrust_path = f"{base_name}_thrust.txt"
            limits_path = f"{base_name}_limits.txt"
            limits = parse_limits(limits_path) if os.path.exists(limits_path) else {}
            if os.path.exists(thrust_path):
                thrust = parse_thrust_grid(thrust_path)
                if thrust:
                    plot_thrust_altitude(thrust, limits, base_name, aircraft_name, meta.get('weight'), 'tas')
                    plot_thrust_altitude(thrust, limits, base_name, aircraft_name, meta.get('weight'), 'ias')
            tas_alt_path = f"{base_name}_tas_alt.txt"
            if os.path.exists(tas_alt_path):
                rows = parse_tas_alt(tas_alt_path)
                if rows:
                    plot_tas_altitude(rows, base_name, aircraft_name, meta.get('weight'))
        else:
            print("Error: Could not parse EM grid data")
            sys.exit(1)
    elif 'velocity' in header:
        # Jet data has velocity in first column
        data_type = 'jet'
        vel2, alt2, thrust2, wep2 = parse_thrust_data(input_file)
        if vel2 and alt2 and thrust2:
            has_wep = any(len(row) > 0 and any(w > 0 for w in row) for row in wep2)
            print(f"Detected jet engine data: {len(vel2)} velocities, {len(alt2)} altitudes, WEP: {has_wep}")
            plot_jet_thrust(vel2, alt2, thrust2, wep2, output_file, aircraft_name)
            if args.__dict__.get('3d', False):
                output_3d = output_file.replace('.png', '_3d.png')
                plot_jet_thrust_3d(vel2, alt2, thrust2, output_3d, aircraft_name)
        else:
            print("Error: Could not parse jet engine data")
            sys.exit(1)
    else:
        # Piston data
        data_type = 'piston'
        alt1, pwr1, wep1 = parse_power_data(input_file)
        if alt1 and pwr1:
            has_wep = any(w > 0 for w in wep1)
            print(f"Detected piston engine data: {len(alt1)} points, WEP: {has_wep}")
            plot_piston_power(alt1, pwr1, wep1, output_file, aircraft_name)
        else:
            print("Error: Could not parse piston engine data")
            sys.exit(1)


if __name__ == '__main__':
    main()
