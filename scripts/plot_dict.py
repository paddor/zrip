#!/usr/bin/env python3
"""Generate dictionary compression performance chart.

Reads ~/.cache/zrip/L*/{codec}.jsonl for zrip+dict and C_zstd+dict codecs.
Shows encode throughput and compression ratio vs input size for dict mode.

Usage:
    python3 scripts/plot_dict.py [output_dir]
"""
import json
import math
import os
import platform
import sys


CODEC_ORDER = ["C zstd+dict", "zrip+dict", "C zstd", "zrip"]

COLORS = {
    "C zstd+dict": "#60a5fa",
    "zrip+dict":   "#f87171",
    "C zstd":      "#60a5fa",
    "zrip":        "#f87171",
}

LABELS = {
    "C zstd+dict": "C zstd + dict",
    "zrip+dict":   "zrip + dict",
    "C zstd":      "C zstd (no dict)",
    "zrip":        "zrip (no dict)",
}

SMALL_PREFIXES = ["dickens", "hdfs", "xml_collection"]
SMALL_SUFFIXES = ["_2k", "_8k", "_32k", "_128k"]
SMALL_SIZES = [2048, 8192, 32768, 131072]
SIZE_LABELS = ["2K", "8K", "32K", "128K"]

DISPLAY_LEVELS = [1, 3]


def detect_hardware():
    try:
        cpu = os.environ.get("ZRIP_CPU")
        if not cpu:
            for line in open("/proc/cpuinfo"):
                if line.startswith("model name"):
                    cpu = line.split(":", 1)[1].strip()
                    cpu = cpu.replace("(R)", "").replace("(TM)", "").replace("CPU ", "")
                    break
        if cpu:
            extras = []
            try:
                gov = open("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").read().strip()
                if gov == "performance":
                    extras.append("performance governor")
            except OSError:
                pass
            for path, off_val in [
                ("/sys/devices/system/cpu/intel_pstate/no_turbo", "1"),
                ("/sys/devices/system/cpu/cpufreq/boost", "0"),
            ]:
                try:
                    if open(path).read().strip() == off_val:
                        extras.append("turbo off")
                    break
                except OSError:
                    continue
            if not extras:
                hw = os.environ.get("ZRIP_HW_EXTRAS")
                if hw:
                    extras.extend(hw.split(","))
            if extras:
                cpu += ", " + ", ".join(extras)
            return cpu
    except OSError:
        pass
    return None


def load_dict_data():
    cache_dir = os.path.join(os.environ.get("HOME", "."), ".cache", "zrip")
    data = {}
    if not os.path.isdir(cache_dir):
        return data
    small_names = set()
    for prefix in SMALL_PREFIXES:
        for suffix in SMALL_SUFFIXES:
            small_names.add(prefix + suffix)
    for entry in os.listdir(cache_dir):
        level_dir = os.path.join(cache_dir, entry)
        if not os.path.isdir(level_dir) or not entry.startswith("L"):
            continue
        for codec in CODEC_ORDER:
            fname = codec.replace(" ", "_") + ".jsonl"
            path = os.path.join(level_dir, fname)
            if not os.path.exists(path):
                continue
            if codec not in data:
                data[codec] = {}
            with open(path) as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue
                    r = json.loads(line)
                    if r["input"] not in small_names:
                        continue
                    data[codec][(r["input"], r["level"])] = r
    return {codec: list(seen.values()) for codec, seen in data.items()}


def nice_y_step(max_val):
    raw = max_val / 5
    mag = 10 ** math.floor(math.log10(max(raw, 1e-9)))
    for s in [1, 2, 5, 10]:
        step = s * mag
        if max_val / step <= 7:
            return step
    return mag * 10


def generate_svg(data):
    hw_label = detect_hardware()

    svg_w = 970
    n_rows = 2
    n_cols = len(SMALL_PREFIXES)
    panel_w = 220
    panel_h = 200
    top_margin = 55 if hw_label else 45
    left_margin = 90
    row_gap = 80
    col_gap = 70
    bottom_margin = 70
    svg_h = top_margin + n_rows * panel_h + (n_rows - 1) * row_gap + bottom_margin

    mid_x = svg_w / 2
    x_start = left_margin

    log_min = math.log10(1500)
    log_max = math.log10(200000)

    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')
    L.append(
        f'  <text x="{mid_x}" y="18" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'Dictionary Compression: Throughput and Ratio vs Input Size'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="34" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    row_labels = ["encode MB/s", "compression ratio"]

    for row in range(n_rows):
        row_top = top_margin + row * (panel_h + row_gap)

        y_mid = row_top + panel_h / 2
        L.append(
            f'  <text x="20" y="{y_mid}" text-anchor="middle" fill="#e6edf3"'
            f' font-size="11" font-weight="600"'
            f' transform="rotate(-90,20,{y_mid})">{row_labels[row]}</text>'
        )

        for ci, prefix in enumerate(SMALL_PREFIXES):
            xl = x_start + ci * (panel_w + col_gap)
            xr = xl + panel_w
            p_top = row_top
            p_bot = p_top + panel_h
            pw = xr - xl
            pcx = (xl + xr) / 2

            L.append(
                f'  <rect x="{xl}" y="{p_top}" width="{pw}" height="{panel_h}"'
                f' fill="#161b22" rx="4"/>'
            )

            if row == 0:
                title = prefix.replace("_", " ")
                L.append(
                    f'  <text x="{pcx}" y="{p_top - 8}" text-anchor="middle" fill="#e6edf3"'
                    f' font-size="12" font-weight="600">{title}</text>'
                )

            all_vals = []
            for codec in CODEC_ORDER:
                rows = data.get(codec, [])
                for level in DISPLAY_LEVELS:
                    for si, suffix in enumerate(SMALL_SUFFIXES):
                        name = prefix + suffix
                        matches = [r for r in rows if r["input"] == name and r["level"] == level]
                        if matches:
                            r = matches[0]
                            if row == 0:
                                val = r["input_size"] / r["compress_ns"] * 1000
                            else:
                                val = r["input_size"] / r["compressed_size"]
                            all_vals.append(val)

            if not all_vals:
                continue

            y_max = max(all_vals) * 1.15
            y_min = 0 if row == 0 else min(all_vals) * 0.9

            def map_x(size):
                frac = (math.log10(size) - log_min) / (log_max - log_min)
                return xl + frac * pw

            def map_y(v):
                frac = (v - y_min) / (y_max - y_min)
                return p_bot - frac * (p_bot - p_top)

            for si, (size, label) in enumerate(zip(SMALL_SIZES, SIZE_LABELS)):
                xx = map_x(size)
                if xl + 5 < xx < xr - 5:
                    L.append(
                        f'  <line x1="{xx:.1f}" y1="{p_top}" x2="{xx:.1f}" y2="{p_bot}"'
                        f' stroke="#21262d" stroke-width="1"/>'
                    )
                    L.append(
                        f'  <text x="{xx:.1f}" y="{p_bot + 14}" text-anchor="middle"'
                        f' fill="#7d8590" font-size="9">{label}</text>'
                    )

            y_step = nice_y_step(y_max - y_min)
            v = math.ceil(y_min / y_step) * y_step
            while v < y_max:
                yy = map_y(v)
                if p_top + 5 < yy < p_bot - 5:
                    L.append(
                        f'  <line x1="{xl}" y1="{yy:.1f}" x2="{xr}" y2="{yy:.1f}"'
                        f' stroke="#21262d" stroke-width="1"/>'
                    )
                    fmt = f"{int(v)}" if row == 0 else f"{v:.2f}x"
                    L.append(
                        f'  <text x="{xl - 8}" y="{yy:.1f}" text-anchor="end"'
                        f' dominant-baseline="middle" fill="#7d8590" font-size="9">'
                        f'{fmt}</text>'
                    )
                v += y_step

            for codec in CODEC_ORDER:
                rows = data.get(codec, [])
                color = COLORS[codec]
                is_dict = "+dict" in codec
                for level in DISPLAY_LEVELS:
                    pts = []
                    for si, suffix in enumerate(SMALL_SUFFIXES):
                        name = prefix + suffix
                        matches = [r for r in rows if r["input"] == name and r["level"] == level]
                        if matches:
                            r = matches[0]
                            if row == 0:
                                val = r["input_size"] / r["compress_ns"] * 1000
                            else:
                                val = r["input_size"] / r["compressed_size"]
                            pts.append((SMALL_SIZES[si], val))

                    if not pts:
                        continue

                    dash = ""
                    if not is_dict:
                        dash = ' stroke-dasharray="4,3"'
                    elif level == 3:
                        dash = ' stroke-dasharray="8,4"'

                    opacity = "1" if is_dict else "0.4"

                    if len(pts) > 1:
                        path_parts = []
                        for i, (sz, val) in enumerate(pts):
                            xx = map_x(sz)
                            yy = map_y(val)
                            cmd = "M" if i == 0 else "L"
                            path_parts.append(f"{cmd}{xx:.1f},{yy:.1f}")
                        L.append(
                            f'  <path d="{"".join(path_parts)}" fill="none"'
                            f' stroke="{color}" stroke-width="1.5"{dash}'
                            f' stroke-opacity="{opacity}"/>'
                        )

                    r_size = "3" if is_dict else "2"
                    for sz, val in pts:
                        xx = map_x(sz)
                        yy = map_y(val)
                        L.append(
                            f'  <circle cx="{xx:.1f}" cy="{yy:.1f}" r="{r_size}"'
                            f' fill="{color}" stroke="#0d1117" stroke-width="1"'
                            f' opacity="{opacity}"/>'
                        )

                    if is_dict and row == 0:
                        last_sz, last_val = pts[-1]
                        xx = map_x(last_sz)
                        yy = map_y(last_val)
                        lbl = f"L{level}"
                        L.append(
                            f'  <text x="{xx + 6:.1f}" y="{yy + 3:.1f}" text-anchor="start"'
                            f' fill="{color}" font-size="8" font-weight="600">{lbl}</text>'
                        )

    # Legend
    leg_y = svg_h - bottom_margin + 40
    legend_items = [(c, LABELS[c]) for c in CODEC_ORDER if c in data]

    rw = sum(len(lb) * 6.2 + 24 for _, lb in legend_items) + 12 * (len(legend_items) - 1)
    if rw > svg_w - 40 and len(legend_items) > 2:
        split = (len(legend_items) + 1) // 2
        rows_of_items = [legend_items[:split], legend_items[split:]]
    else:
        rows_of_items = [legend_items]

    for ri, row_items in enumerate(rows_of_items):
        rw = sum(len(lb) * 6.2 + 24 for _, lb in row_items) + 12 * (len(row_items) - 1)
        lx = mid_x - rw / 2
        ry = leg_y + ri * 18
        for key, label in row_items:
            color = COLORS[key]
            is_dict = "+dict" in key
            if is_dict:
                L.append(
                    f'  <circle cx="{lx + 5:.0f}" cy="{ry}" r="4" fill="{color}"/>'
                )
            else:
                L.append(
                    f'  <circle cx="{lx + 5:.0f}" cy="{ry}" r="3" fill="{color}"'
                    f' opacity="0.4"/>'
                )
            L.append(
                f'  <text x="{lx + 13:.0f}" y="{ry + 3.5}" fill="#e6edf3"'
                f' font-size="10" font-weight="500">{label}</text>'
            )
            lx += len(label) * 6.2 + 24 + 12

    L.append("</svg>")
    return "\n".join(L) + "\n"


def main():
    data = load_dict_data()
    if not data:
        print("No dict data in ~/.cache/zrip/", file=sys.stderr)
        sys.exit(1)

    svg = generate_svg(data)

    arch = platform.machine() or "x86_64"
    output_dir = sys.argv[1] if len(sys.argv) > 1 else f"doc/charts/{arch}"
    os.makedirs(output_dir, exist_ok=True)
    path = os.path.join(output_dir, "dict.svg")
    with open(path, "w") as f:
        f.write(svg)
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
