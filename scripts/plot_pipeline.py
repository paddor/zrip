#!/usr/bin/env python3
"""Generate per-file pipeline SVG chart from cached bench data.

Stacked bars: compress + transfer@100MB/s + decompress, seconds/GB.
Two panels splitting files in half. Level 1 only.

Reads ~/.cache/zrip/L1/{codec}.jsonl, writes pipeline.svg.

Usage:
    python3 scripts/plot_pipeline.py [output_dir]
"""
import json
import math
import os
import platform
import sys


CODEC_ORDER = ["C zstd", "zrip", "structured-zstd", "zrip paranoid", "ruzstd"]

COLORS = {
    "C zstd":          ("#60a5fa", "#4680c4"),
    "zrip":            ("#f87171", "#c45050"),
    "zrip paranoid":   ("#f472b6", "#c05a92"),
    "structured-zstd": ("#f59e0b", "#c47d08"),
    "ruzstd":          ("#4ade80", "#3aaf60"),
}

LABELS = {
    "C zstd":          "C zstd 1.5.7 (libzstd)",
    "zrip":            "zrip (encapsulated unsafe, Rust)",
    "zrip paranoid":   "zrip paranoid (safe Rust, no unsafe)",
    "structured-zstd": "structured-zstd 0.0.44 (unsafe Rust)",
    "ruzstd":          "ruzstd 0.8.2 (safe Rust)",
}

LEVEL = 1
MIN_FILE_SIZE = 1_000_000
TRANSFER_RATE = 100e6  # 100 MB/s


def _apply_profile():
    from profiles import apply_profile
    p = apply_profile(sys.argv)
    if p is None:
        return
    global CODEC_ORDER, COLORS, LABELS
    CODEC_ORDER = p["CODEC_ORDER"]
    COLORS = p.get("COLORS_TUPLE", p["COLORS"])
    LABELS = p["LABELS"]


_apply_profile()


def human_size(n):
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f} MB"
    if n >= 1_000:
        return f"{n / 1_000:.0f} KB"
    return f"{n} B"


def nice_step(max_val, target_lines):
    raw = max_val / target_lines
    mag = 10 ** int(f"{raw:.0e}".split("e")[1])
    for s in [1, 2, 5, 10]:
        step = s * mag
        if max_val / step <= target_lines + 1:
            return step
    return mag * 10


def load_level_data():
    from profiles import cache_target
    cache_dir = os.path.join(
        os.environ.get("HOME", "."), ".cache", "zrip", cache_target())
    level_dir = os.path.join(cache_dir, f"L{LEVEL}")
    if not os.path.isdir(level_dir):
        return {}
    data = {}
    for codec in CODEC_ORDER:
        fname = codec.replace(" ", "_") + ".jsonl"
        path = os.path.join(level_dir, fname)
        if not os.path.exists(path):
            continue
        seen = {}
        with open(path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                r = json.loads(line)
                if r.get("input_size", 0) < MIN_FILE_SIZE:
                    continue
                cns = r.get("compress_ns", 0)
                if cns is None or (isinstance(cns, float) and math.isnan(cns)):
                    continue
                seen[r["input"]] = r
        if seen:
            data[codec] = list(seen.values())
    return data


def generate_svg(data):
    from profiles import detect_hardware
    hw_label = detect_hardware()

    codecs = [c for c in CODEC_ORDER if c in data]
    n_codecs = len(codecs)
    if n_codecs == 0:
        return None

    input_order = []
    seen = set()
    for codec in codecs:
        for r in data[codec]:
            if r["input"] not in seen:
                input_order.append(r["input"])
                seen.add(r["input"])
    input_order.sort()

    input_sizes = {}
    for codec in codecs:
        for r in data[codec]:
            input_sizes[r["input"]] = r["input_size"]

    mid = (len(input_order) + 1) // 2
    panels = [input_order[:mid], input_order[mid:]]

    stacks = {}
    y_max = 0
    for codec in codecs:
        for r in data[codec]:
            inp = r["input"]
            sz = r["input_size"]
            per_gb = 1e9 / sz
            comp = r["compress_ns"] / 1e9 * per_gb
            transfer = (r["compressed_size"] / sz) * (1e9 / TRANSFER_RATE)
            decomp = r["decompress_ns"] / 1e9 * per_gb
            stacks[(inp, codec)] = (comp, transfer, decomp)
            y_max = max(y_max, comp + transfer + decomp)

    y_max *= 1.1

    svg_w = 850
    x_left, x_right = 55, 830
    plot_w = x_right - x_left
    panel_h = 240
    panel_gap = 70
    top_margin = 50 if hw_label else 40

    panel_tops = [
        top_margin,
        top_margin + panel_h + panel_gap,
    ]
    svg_h = panel_tops[-1] + panel_h + 120

    mid_x = (x_left + x_right) / 2
    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')

    L.append(
        f'  <text x="{mid_x}" y="22" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'zstd Pipeline @100 MB/s: Per-file, Level 1 (lower is better)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="38" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    for pi, panel_inputs in enumerate(panels):
        n_inputs = len(panel_inputs)
        if n_inputs == 0:
            continue
        p_top = panel_tops[pi]
        p_bot = p_top + panel_h

        group_w = plot_w / n_inputs
        bar_w = group_w * 0.75 / n_codecs
        gap = group_w * 0.25

        def y(v, _bot=p_bot, _top=p_top):
            return _bot - (v / y_max) * (_bot - _top)

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
                f' dominant-baseline="middle" fill="#7d8590" font-size="10">{v:.0f}</text>'
            )
            v += step

        L.append(
            f'  <line x1="{x_left}" y1="{p_bot}" x2="{x_right}" y2="{p_bot}"'
            f' stroke="#30363d" stroke-width="1.5"/>'
        )

        if pi == 0:
            total_mid_y = (panel_tops[0] + panel_tops[1] + panel_h) / 2
            L.append(
                f'  <text x="22" y="{total_mid_y}" text-anchor="middle" fill="#e6edf3"'
                f' font-size="11" font-weight="600"'
                f' transform="rotate(-90,22,{total_mid_y})">seconds / GB</text>'
            )

        for gi, inp in enumerate(panel_inputs):
            group_x = x_left + gi * group_w + gap / 2

            for ci, codec in enumerate(codecs):
                if (inp, codec) not in stacks:
                    continue
                comp, transfer, decomp = stacks[(inp, codec)]
                main_c, xfer_c = COLORS[codec]

                bx = group_x + ci * bar_w
                h_comp = (comp / y_max) * (p_bot - p_top)
                L.append(
                    f'  <rect x="{bx:.1f}" y="{y(comp):.1f}"'
                    f' width="{bar_w:.1f}" height="{h_comp:.1f}"'
                    f' fill="{main_c}" rx="1"/>'
                )
                h_transfer = (transfer / y_max) * (p_bot - p_top)
                L.append(
                    f'  <rect x="{bx:.1f}" y="{y(comp + transfer):.1f}"'
                    f' width="{bar_w:.1f}" height="{h_transfer:.1f}"'
                    f' fill="{xfer_c}" rx="1"/>'
                )
                h_decomp = (decomp / y_max) * (p_bot - p_top)
                L.append(
                    f'  <rect x="{bx:.1f}" y="{y(comp + transfer + decomp):.1f}"'
                    f' width="{bar_w:.1f}" height="{h_decomp:.1f}"'
                    f' fill="{main_c}" rx="1"/>'
                )

            label = inp.replace(".txt", "").replace("compression_", "")
            size_label = human_size(input_sizes.get(inp, 0))
            cx = group_x + (n_codecs * bar_w) / 2
            L.append(
                f'  <text x="{cx:.1f}" y="{p_bot + 16}" text-anchor="middle"'
                f' fill="#e6edf3" font-size="10" font-weight="600">{label}</text>'
            )
            L.append(
                f'  <text x="{cx:.1f}" y="{p_bot + 28}" text-anchor="middle"'
                f' fill="#7d8590" font-size="9">{size_label}</text>'
            )

    leg_y = panel_tops[-1] + panel_h + 50
    legend_items = [(k, LABELS[k]) for k in codecs if k in COLORS]
    row_h = 18
    n_cols = 2
    n_leg_rows = (len(legend_items) + n_cols - 1) // n_cols
    leg_col_x = [mid_x - 220, mid_x + 30]
    for i, (key, label) in enumerate(legend_items):
        col = i // n_leg_rows
        row = i % n_leg_rows
        lx = leg_col_x[min(col, len(leg_col_x) - 1)]
        ly = leg_y + row * row_h
        main_c, xfer_c = COLORS[key]
        L.append(
            f'  <rect x="{lx:.0f}" y="{ly - 5}" width="12" height="12"'
            f' fill="{main_c}" rx="2"/>'
        )
        L.append(
            f'  <text x="{lx + 18:.0f}" y="{ly + 5}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{label}</text>'
        )

    seg_y = leg_y + n_leg_rows * row_h + 8
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
    data = load_level_data()
    if not data:
        print(f"No cached Level {LEVEL} data in ~/.cache/zrip/", file=sys.stderr)
        sys.exit(1)

    svg = generate_svg(data)
    if svg is None:
        print("No data to plot", file=sys.stderr)
        sys.exit(1)

    arch = platform.machine() or "x86_64"
    output_dir = sys.argv[1] if len(sys.argv) > 1 else f"doc/charts/{arch}"
    os.makedirs(output_dir, exist_ok=True)
    path = os.path.join(output_dir, "pipeline.svg")
    with open(path, "w") as f:
        f.write(svg)
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
