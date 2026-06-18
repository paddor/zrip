#!/usr/bin/env python3
"""Generate summary SVG chart: single-panel corpus geomean, Level 1 only.

All codec implementations shown. Geometric mean of per-file pipeline
segment times (compress, transfer@100MB/s, decompress) in seconds/GB.

Reads ~/.cache/zrip/L1/{codec}.jsonl, writes summary.svg.

Usage:
    python3 scripts/plot_summary.py [output_dir]
"""
import json
import math
import os
import platform
import sys


CODEC_ORDER = ["C zstd", "zrip", "structured-zstd", "ruzstd"]

COLORS = {
    "C zstd":          ("#60a5fa", "#4680c4"),
    "zrip":            ("#f87171", "#c45050"),
    "structured-zstd": ("#f59e0b", "#c47d08"),
    "ruzstd":          ("#4ade80", "#3aaf60"),
}

LABELS = {
    "C zstd":          "C zstd 1.5.7 (libzstd)",
    "zrip":            "zrip (safe API, Rust)",
    "structured-zstd": "structured-zstd 0.0.41 (unsafe Rust)",
    "ruzstd":          "ruzstd 0.8.2 (safe Rust)",
}

LEVEL = 1
MIN_FILE_SIZE = 10_000
TRANSFER_RATE = 100e6  # 100 MB/s


def detect_hardware():
    try:
        for line in open("/proc/cpuinfo"):
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                cpu = cpu.replace("(R)", "").replace("(TM)", "").replace("CPU ", "")
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


def nice_step(max_val, target_lines):
    raw = max_val / target_lines
    mag = 10 ** int(f"{raw:.0e}".split("e")[1])
    for s in [1, 2, 5, 10]:
        step = s * mag
        if max_val / step <= target_lines + 1:
            return step
    return mag * 10


def load_all_data():
    cache_dir = os.path.join(os.environ.get("HOME", "."), ".cache", "zrip")
    data = {}
    if not os.path.isdir(cache_dir):
        return data
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
                    if r.get("input_size", 0) < MIN_FILE_SIZE:
                        continue
                    data[codec][(r["input"], r["level"])] = r
    return {codec: list(seen.values()) for codec, seen in data.items()}


def compute_geomean(data):
    results = {}
    for codec in CODEC_ORDER:
        rows = data.get(codec, [])
        level_rows = [r for r in rows if r["level"] == LEVEL]

        comp_log = []
        xfer_log = []
        decomp_log = []

        for r in level_rows:
            cns = r.get("compress_ns", 0)
            if cns is None or (isinstance(cns, float) and math.isnan(cns)):
                continue
            sz = r["input_size"]
            per_gb = 1e9 / sz
            comp_log.append(math.log(cns / 1e9 * per_gb))
            xfer_log.append(math.log((r["compressed_size"] / sz) * (1e9 / TRANSFER_RATE)))
            decomp_log.append(math.log(r["decompress_ns"] / 1e9 * per_gb))

        if comp_log:
            n = len(comp_log)
            results[codec] = (
                math.exp(sum(comp_log) / n),
                math.exp(sum(xfer_log) / n),
                math.exp(sum(decomp_log) / n),
            )

    return results


def generate_svg(data):
    stacks = compute_geomean(data)
    hw_label = detect_hardware()

    codecs = [c for c in CODEC_ORDER if c in stacks]
    n = len(codecs)
    if n == 0:
        return None

    svg_w = 700
    x_left, x_right = 70, 680
    plot_w = x_right - x_left

    top_margin = 55 if hw_label else 45
    plot_h = 250
    p_top = top_margin
    p_bot = p_top + plot_h

    y_max = max(sum(stacks[c]) for c in codecs) * 1.15

    def y(v):
        return p_bot - (v / y_max) * plot_h

    bar_w = min(plot_w * 0.7 / n, 80)
    gap = bar_w * 0.4
    total_bars_w = n * bar_w + (n - 1) * gap
    bar_start = x_left + (plot_w - total_bars_w) / 2

    mid_x = svg_w / 2

    legend_items = [(k, LABELS[k]) for k in CODEC_ORDER if k in stacks]
    leg_row_h = 18
    leg_cols = 2
    leg_rows = math.ceil(len(legend_items) / leg_cols)

    svg_h = int(p_bot + 40 + leg_rows * leg_row_h + 30)

    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')

    L.append(
        f'  <text x="{mid_x}" y="22" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'zstd Pipeline @100 MB/s: Corpus geomean, Level 1 (lower is better)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="38" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    y_mid = (p_top + p_bot) / 2
    L.append(
        f'  <text x="22" y="{y_mid}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="11" font-weight="600"'
        f' transform="rotate(-90,22,{y_mid})">seconds / GB</text>'
    )

    step = nice_step(y_max, 5)
    v = step
    while v <= y_max:
        yy = y(v)
        L.append(
            f'  <line x1="{x_left}" y1="{yy:.1f}" x2="{x_right}" y2="{yy:.1f}"'
            f' stroke="#21262d" stroke-width="1"/>'
        )
        L.append(
            f'  <text x="{x_left - 8}" y="{yy:.1f}" text-anchor="end"'
            f' dominant-baseline="middle" fill="#7d8590" font-size="10">'
            f'{v:.1f}</text>'
        )
        v += step

    L.append(
        f'  <line x1="{x_left}" y1="{p_bot}" x2="{x_right}" y2="{p_bot}"'
        f' stroke="#30363d" stroke-width="1.5"/>'
    )

    for i, codec in enumerate(codecs):
        comp, transfer, decomp = stacks[codec]
        main_c, xfer_c = COLORS[codec]
        bx = bar_start + i * (bar_w + gap)

        h_comp = (comp / y_max) * plot_h
        L.append(
            f'  <rect x="{bx:.1f}" y="{y(comp):.1f}"'
            f' width="{bar_w:.1f}" height="{h_comp:.1f}"'
            f' fill="{main_c}" rx="1"/>'
        )
        h_transfer = (transfer / y_max) * plot_h
        L.append(
            f'  <rect x="{bx:.1f}" y="{y(comp + transfer):.1f}"'
            f' width="{bar_w:.1f}" height="{h_transfer:.1f}"'
            f' fill="{xfer_c}" rx="1"/>'
        )
        h_decomp = (decomp / y_max) * plot_h
        L.append(
            f'  <rect x="{bx:.1f}" y="{y(comp + transfer + decomp):.1f}"'
            f' width="{bar_w:.1f}" height="{h_decomp:.1f}"'
            f' fill="{main_c}" rx="1"/>'
        )

        cx = bx + bar_w / 2
        L.append(
            f'  <text x="{cx:.1f}" y="{p_bot + 16}" text-anchor="middle"'
            f' fill="#e6edf3" font-size="10" font-weight="600">{codec}</text>'
        )

    leg_y = p_bot + 35
    leg_col_x = [mid_x - 200, mid_x + 10]
    for i, (key, label) in enumerate(legend_items):
        col = i // leg_rows
        row = i % leg_rows
        if col >= leg_cols:
            break
        lx = leg_col_x[col]
        ly = leg_y + row * leg_row_h
        main_c, _ = COLORS[key]
        L.append(
            f'  <rect x="{lx:.0f}" y="{ly - 5}" width="12" height="12"'
            f' fill="{main_c}" rx="2"/>'
        )
        L.append(
            f'  <text x="{lx + 18:.0f}" y="{ly + 5}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{label}</text>'
        )

    seg_y = leg_y + leg_rows * leg_row_h + 8
    seg_items = [
        ("bright = compress + decompress", "#e6edf3"),
        ("dim = transfer @100 MB/s", "#7d8590"),
    ]
    seg_total = 420
    seg_start = mid_x - seg_total / 2
    for i, (label, fill) in enumerate(seg_items):
        sx = seg_start + i * 240
        L.append(
            f'  <text x="{sx:.0f}" y="{seg_y + 4}" fill="{fill}"'
            f' font-size="9">{label}</text>'
        )

    L.append("</svg>")
    return "\n".join(L) + "\n"


def main():
    data = load_all_data()
    if not data:
        print("No cached data in ~/.cache/zrip/", file=sys.stderr)
        sys.exit(1)

    svg = generate_svg(data)
    if svg is None:
        print("No Level 1 data found", file=sys.stderr)
        sys.exit(1)

    arch = platform.machine() or "x86_64"
    output_dir = sys.argv[1] if len(sys.argv) > 1 else f"doc/charts/{arch}"
    os.makedirs(output_dir, exist_ok=True)
    path = os.path.join(output_dir, "summary.svg")
    with open(path, "w") as f:
        f.write(svg)
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
