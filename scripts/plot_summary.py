#!/usr/bin/env python3
"""Generate summary SVG chart from cached zrip_bench JSONL data.

Reads ~/.cache/zrip/{codec}.jsonl, writes summary.svg.
Style mirrors lz4rip/benches/plot_bench.py (stacked pipeline bars,
seconds/GB, GitHub dark theme).

Usage:
    python3 scripts/plot_summary.py [output_dir]
"""
import json
import math
import os
import platform
import sys


CODEC_ORDER = ["C zstd", "zrip", "structured-zstd", "ruzstd", "lz4rip"]

COLORS = {
    "C zstd":          ("#60a5fa", "#4680c4"),
    "zrip":            ("#f87171", "#c45050"),
    "structured-zstd": ("#f59e0b", "#c47d08"),
    "ruzstd":          ("#4ade80", "#3aaf60"),
    "lz4rip":          ("#c084fc", "#9966cc"),
}

LABELS = {
    "C zstd":          "C zstd 1.5.7 (libzstd)",
    "zrip":            "zrip (safe Rust)",
    "structured-zstd": "structured-zstd 0.0.37 (unsafe Rust)",
    "ruzstd":          "ruzstd 0.8.2 (safe Rust)",
    "lz4rip":          "lz4rip 0.3.1 (safe Rust, LZ4)",
}

LEVELS = [3, 1, -1]
LEVEL_LABELS = {-1: "Level −1", 1: "Level 1", 3: "Level 3"}

COMPRESSIBLE = {
    "dickens.txt", "hdfs.json", "nci", "xml_collection.xml", "webster",
    "samba", "reymont.pdf", "mozilla", "compression_34k.txt",
    "compression_65k.txt", "compression_66k_JSON.txt", "osdb",
}
INCOMPRESSIBLE = {"sao", "x-ray", "mr"}

MIN_FILE_SIZE = 10_000
TRANSFER_RATE = 1e9  # 1 GB/s


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
    for codec in CODEC_ORDER:
        fname = codec.replace(" ", "_") + ".jsonl"
        path = os.path.join(cache_dir, fname)
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
                seen[(r["input"], r["level"])] = r
        data[codec] = list(seen.values())
    return data


def compute_stacks(data):
    """Aggregate pipeline times: compress + transfer@1GB/s + decompress."""
    groups = [("Compressible", COMPRESSIBLE), ("Incompressible", INCOMPRESSIBLE)]

    stacks = {}
    for codec in CODEC_ORDER:
        rows = data.get(codec, [])
        for level in LEVELS:
            if codec == "ruzstd" and level != 1:
                continue
            if codec == "lz4rip" and level != -1:
                continue
            level_rows = [r for r in rows if r["level"] == level]
            for group_name, file_set in groups:
                total_input = 0
                total_compressed = 0
                total_compress_ns = 0.0
                total_decompress_ns = 0.0
                for r in level_rows:
                    if r["input"] not in file_set:
                        continue
                    cns = r.get("compress_ns", 0)
                    if cns is None or (isinstance(cns, float) and math.isnan(cns)):
                        continue
                    total_input += r["input_size"]
                    total_compressed += r["compressed_size"]
                    total_compress_ns += cns
                    total_decompress_ns += r["decompress_ns"]

                if total_input > 0:
                    per_gb = 1e9 / total_input
                    comp = total_compress_ns / 1e9 * per_gb
                    transfer = (total_compressed / total_input) * (1e9 / TRANSFER_RATE)
                    decomp = total_decompress_ns / 1e9 * per_gb
                    stacks[(codec, level, group_name)] = (comp, transfer, decomp)

    return stacks


def generate_svg(data):
    stacks = compute_stacks(data)
    hw_label = detect_hardware()

    n_levels = len(LEVELS)
    groups = ["Compressible", "Incompressible"]

    svg_w = 850
    x_left, x_right = 70, 830
    plot_w = x_right - x_left

    top_margin = 50 if hw_label else 40
    panel_h = 200
    panel_gap = 55
    panel_label_h = 20

    panel_tops = []
    y = top_margin
    for _ in range(n_levels):
        y += panel_label_h
        panel_tops.append(y)
        y += panel_h + panel_gap

    svg_h = y - panel_gap + 130

    # shared y-axis scale across all panels
    y_max = 0
    for v in stacks.values():
        y_max = max(y_max, sum(v))
    y_max *= 1.15

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
        f'zstd Pipeline @1 GB/s: Aggregate across corpus (lower is better)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="38" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    # y-axis label (centered across all panels)
    total_mid_y = (panel_tops[0] + panel_tops[-1] + panel_h) / 2
    L.append(
        f'  <text x="22" y="{total_mid_y}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="11" font-weight="600"'
        f' transform="rotate(-90,22,{total_mid_y})">seconds / GB</text>'
    )

    for pi, level in enumerate(LEVELS):
        p_top = panel_tops[pi]
        p_bot = p_top + panel_h

        # panel title
        L.append(
            f'  <text x="{mid_x}" y="{p_top - 6}" text-anchor="middle" fill="#e6edf3"'
            f' font-size="12" font-weight="600">{LEVEL_LABELS[level]}</text>'
        )

        # codecs present at this level
        codecs = [c for c in CODEC_ORDER
                  if any(stacks.get((c, level, g)) for g in groups)]
        n_codecs = len(codecs)
        if n_codecs == 0:
            continue

        def y(v, _bot=p_bot, _top=p_top):
            return _bot - (v / y_max) * (_bot - _top)

        # y gridlines + labels (only on leftmost panel row, reuse positions)
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

        # baseline
        L.append(
            f'  <line x1="{x_left}" y1="{p_bot}" x2="{x_right}" y2="{p_bot}"'
            f' stroke="#30363d" stroke-width="1.5"/>'
        )

        # bars
        n_groups = len(groups)
        group_w = plot_w / n_groups
        bar_w = min(group_w * 0.7 / n_codecs, 50)
        inner_gap = bar_w * 0.15

        for gi, group_name in enumerate(groups):
            group_x = x_left + gi * group_w + (group_w * 0.2) / 2

            for ci, codec in enumerate(codecs):
                key = (codec, level, group_name)
                if key not in stacks:
                    continue
                comp, transfer, decomp = stacks[key]
                main_c, xfer_c = COLORS[codec]

                bx = group_x + ci * (bar_w + inner_gap / n_codecs)
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

            # group label
            cx = group_x + (n_codecs * (bar_w + inner_gap / n_codecs)) / 2
            L.append(
                f'  <text x="{cx:.1f}" y="{p_bot + 18}" text-anchor="middle"'
                f' fill="#e6edf3" font-size="11" font-weight="600">{group_name}</text>'
            )

    # legend (2x2 grid)
    leg_y = panel_tops[-1] + panel_h + 35
    legend_items = [(k, LABELS[k]) for k in CODEC_ORDER if k in COLORS]
    row_h = 18
    leg_positions = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1)]
    leg_col_x = [mid_x - 200, mid_x + 10]
    n_leg_rows = max(row for _, row in leg_positions) + 1
    for i, (key, label) in enumerate(legend_items):
        if i >= len(leg_positions):
            break
        col, row = leg_positions[i]
        lx = leg_col_x[col]
        ly = leg_y + row * row_h
        main_c, _ = COLORS[key]
        L.append(
            f'  <rect x="{lx:.0f}" y="{ly - 5}" width="12" height="12"'
            f' fill="{main_c}" rx="2"/>'
        )
        L.append(
            f'  <text x="{lx + 18:.0f}" y="{ly + 5}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{label}</text>'
        )

    # bar segment legend
    seg_y = leg_y + n_leg_rows * row_h + 8
    seg_items = [
        ("bright = compress + decompress", "#e6edf3"),
        ("dim = transfer @1 GB/s", "#7d8590"),
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

    arch = platform.machine() or "x86_64"
    output_dir = sys.argv[1] if len(sys.argv) > 1 else f"doc/charts/{arch}"
    os.makedirs(output_dir, exist_ok=True)
    path = os.path.join(output_dir, "summary.svg")
    with open(path, "w") as f:
        f.write(svg)
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
