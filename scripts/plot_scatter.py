#!/usr/bin/env python3
"""Generate encode speed vs compression ratio scatter plot from cached bench data.

Reads ~/.cache/zrip/{codec}.jsonl, writes scatter.svg.
Three panels stacked vertically: High / Medium / Low compressibility,
each with its own Y-axis scale. X-axis: encode throughput (MB/s, log scale).
Y-axis: compression ratio. Connected points per codec, each point a level.

Usage:
    python3 scripts/plot_scatter.py [output_dir]
"""
import json
import math
import os
import platform
import sys


CODEC_ORDER = ["C zstd", "zrip", "structured-zstd", "zrip paranoid", "ruzstd", "lz4rip"]

COLORS = {
    "C zstd":          "#60a5fa",
    "zrip":            "#f87171",
    "zrip paranoid":   "#f472b6",
    "structured-zstd": "#f59e0b",
    "ruzstd":          "#4ade80",
    "lz4rip":          "#c084fc",
}

LABELS = {
    "C zstd":          "C zstd 1.5.7 (libzstd)",
    "zrip":            "zrip (encapsulated unsafe, Rust)",
    "zrip paranoid":   "zrip paranoid (safe Rust, no unsafe)",
    "structured-zstd": "structured-zstd 0.0.42 (unsafe Rust)",
    "ruzstd":          "ruzstd 0.8.2 (safe Rust)",
    "lz4rip":          "lz4rip 0.8.5 (encapsulated unsafe, Rust, LZ4)",
}

# ruzstd only compresses at L1; other levels output raw (1.00x ratio).
LEVEL_FILTER = {
    "ruzstd": {1},
}

HIGH_COMPRESSIBILITY = {
    "hdfs.json", "nci", "xml_collection.xml", "compression_66k_JSON.txt",
    "samba",
}
MEDIUM_COMPRESSIBILITY = {
    "reymont.pdf", "webster", "osdb", "mr", "mozilla", "dickens.txt",
    "compression_34k.txt", "compression_65k.txt",
}
LOW_COMPRESSIBILITY = {"sao", "x-ray"}

GROUPS = [
    ("High compressibility", HIGH_COMPRESSIBILITY),
    ("Medium compressibility", MEDIUM_COMPRESSIBILITY),
    ("Low compressibility", LOW_COMPRESSIBILITY),
]

MIN_FILE_SIZE = 10_000

# Fixed axis ranges so the chart doesn't shift when data changes.
FIXED_LOG_X_MIN = 1.477  # 10^1.477 ≈ 30 MB/s
FIXED_LOG_X_MAX = 3.903  # 10^3.903 ≈ 8000 MB/s
FIXED_Y_RANGES = {
    "High compressibility":   (2.5, 10.0),
    "Medium compressibility": (1.4, 3.2),
    "Low compressibility":    (0.95, 1.55),
}


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


def compute_points(data):
    """For each (codec, level, group), compute geomean encode MB/s (log scale) and ratio."""
    groups = GROUPS
    points = {}

    for codec in CODEC_ORDER:
        rows = data.get(codec, [])
        allowed = LEVEL_FILTER.get(codec)
        levels = sorted(set(r["level"] for r in rows))
        for level in levels:
            if allowed is not None and level not in allowed:
                continue
            level_rows = [r for r in rows if r["level"] == level]
            for group_name, file_set in groups:
                enc_logs = []
                ratio_logs = []
                for r in level_rows:
                    if r["input"] not in file_set:
                        continue
                    cns = r.get("compress_ns", 0)
                    if cns is None or (isinstance(cns, float) and math.isnan(cns)):
                        continue
                    if cns == 0:
                        continue
                    enc_mbs = r["input_size"] / cns * 1000
                    ratio = r["input_size"] / r["compressed_size"]
                    enc_logs.append(math.log(enc_mbs))
                    ratio_logs.append(math.log(ratio))

                if enc_logs:
                    enc = math.exp(sum(enc_logs) / len(enc_logs))
                    rat = math.exp(sum(ratio_logs) / len(ratio_logs))
                    points[(codec, level, group_name)] = (enc, rat)

    return points


def nice_ratio_step(span):
    raw = span / 5
    mag = 10 ** math.floor(math.log10(max(raw, 1e-9)))
    for s in [1, 2, 5, 10]:
        step = s * mag
        if span / step <= 7:
            return step
    return mag * 10


def render_panel(L, points, data, group_name, xl, xr, p_top, p_bot,
                 log_min, log_max, ratio_min, ratio_max):
    pw = xr - xl
    panel_h = p_bot - p_top

    def map_x(enc_mbs):
        frac = (math.log10(enc_mbs) - log_min) / (log_max - log_min)
        return xl + frac * pw

    def map_y(ratio):
        frac = (ratio - ratio_min) / (ratio_max - ratio_min)
        return p_bot - frac * panel_h

    # Panel background
    L.append(
        f'  <rect x="{xl}" y="{p_top}" width="{pw}" height="{panel_h}"'
        f' fill="#161b22" rx="4"/>'
    )

    # Panel title
    pcx = (xl + xr) / 2
    L.append(
        f'  <text x="{pcx}" y="{p_top - 8}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="12" font-weight="600">{group_name}</text>'
    )

    # Y gridlines
    ystep = nice_ratio_step(ratio_max - ratio_min)
    v = math.ceil(ratio_min / ystep) * ystep
    while v <= ratio_max:
        yy = map_y(v)
        if p_top + 5 < yy < p_bot - 5:
            L.append(
                f'  <line x1="{xl}" y1="{yy:.1f}" x2="{xr}" y2="{yy:.1f}"'
                f' stroke="#21262d" stroke-width="1"/>'
            )
            L.append(
                f'  <text x="{xl - 6}" y="{yy:.1f}" text-anchor="end"'
                f' dominant-baseline="middle" fill="#7d8590" font-size="9">'
                f'{v:.2f}x</text>'
            )
        v += ystep

    # X gridlines
    for decade in range(0, 5):
        for mult in [1, 2, 5]:
            tick = mult * (10 ** decade)
            if 10 ** log_min <= tick <= 10 ** log_max:
                xx = map_x(tick)
                if xl + 5 < xx < xr - 5:
                    L.append(
                        f'  <line x1="{xx:.1f}" y1="{p_top}" x2="{xx:.1f}" y2="{p_bot}"'
                        f' stroke="#21262d" stroke-width="1"/>'
                    )
                    L.append(
                        f'  <text x="{xx:.1f}" y="{p_bot + 14}" text-anchor="middle"'
                        f' fill="#7d8590" font-size="9">{tick}</text>'
                    )

    # X-axis label
    L.append(
        f'  <text x="{pcx}" y="{p_bot + 28}" text-anchor="middle"'
        f' fill="#7d8590" font-size="10">encode MB/s (log scale)</text>'
    )

    # Plot each codec
    for codec in CODEC_ORDER:
        color = COLORS[codec]
        rows = data.get(codec, [])
        levels = sorted(set(r["level"] for r in rows))

        codec_points = []
        for level in levels:
            key = (codec, level, group_name)
            if key in points:
                enc, rat = points[key]
                codec_points.append((level, enc, rat))

        if not codec_points:
            continue

        # Collapse codecs with no meaningful level variation (lz4rip)
        # into a single averaged point.
        if len(codec_points) > 1:
            lo = codec_points[0]
            hi = codec_points[-1]
            if (abs(hi[1] - lo[1]) / lo[1] < 0.05
                    and abs(hi[2] - lo[2]) / lo[2] < 0.05):
                avg_enc = math.exp(
                    sum(math.log(e) for _, e, _ in codec_points) / len(codec_points))
                avg_rat = math.exp(
                    sum(math.log(r) for _, _, r in codec_points) / len(codec_points))
                codec_points = [(codec_points[0][0], avg_enc, avg_rat)]

        # Draw connecting line
        if len(codec_points) > 1:
            path_parts = []
            for i, (_, enc, rat) in enumerate(codec_points):
                xx = map_x(enc)
                yy = map_y(rat)
                cmd = "M" if i == 0 else "L"
                path_parts.append(f"{cmd}{xx:.1f},{yy:.1f}")
            L.append(
                f'  <path d="{"".join(path_parts)}" fill="none"'
                f' stroke="{color}" stroke-width="1.5" stroke-opacity="0.6"/>'
            )

        # Label strategy: for dense sequences (>5 points), label endpoints
        # and every 3rd intermediate point.
        label_indices = set()
        n = len(codec_points)
        if n <= 5:
            label_indices = set(range(n))
        else:
            label_indices.add(0)
            label_indices.add(n - 1)
            for i in range(0, n, 3):
                label_indices.add(i)

        placed = []

        for idx, (level, enc, rat) in enumerate(codec_points):
            xx = map_x(enc)
            yy = map_y(rat)
            L.append(
                f'  <circle cx="{xx:.1f}" cy="{yy:.1f}" r="3.5"'
                f' fill="{color}" stroke="#0d1117" stroke-width="1"/>'
            )

            if idx not in label_indices:
                continue

            if codec == "lz4rip":
                lbl = "LZ4"
            elif codec == "ruzstd":
                lbl = "L1"
            else:
                lbl = str(level)
            lbl_w = len(lbl) * 5.5 + 4
            lbl_h = 10

            candidates = [
                (xx + 6, yy - 3, "start"),
                (xx - 6, yy - 3, "end"),
                (xx, yy - 10, "middle"),
                (xx, yy + 12, "middle"),
            ]

            best = candidates[0]
            for cx, cy, anchor in candidates:
                if anchor == "start":
                    bx = cx
                elif anchor == "end":
                    bx = cx - lbl_w
                else:
                    bx = cx - lbl_w / 2
                by = cy - lbl_h / 2
                bbox = (bx, by, bx + lbl_w, by + lbl_h)

                collides = False
                for pb in placed:
                    if (bbox[0] < pb[2] and bbox[2] > pb[0]
                            and bbox[1] < pb[3] and bbox[3] > pb[1]):
                        collides = True
                        break
                if not collides:
                    best = (cx, cy, anchor)
                    break

            tx, ty, anchor = best
            if anchor == "start":
                bx = tx
            elif anchor == "end":
                bx = tx - lbl_w
            else:
                bx = tx - lbl_w / 2
            by = ty - lbl_h / 2
            placed.append((bx, by, bx + lbl_w, by + lbl_h))

            L.append(
                f'  <text x="{tx:.1f}" y="{ty + 3:.1f}" text-anchor="{anchor}"'
                f' fill="{color}" font-size="8" font-weight="600">{lbl}</text>'
            )


def generate_svg(data):
    points = compute_points(data)
    hw_label = detect_hardware()

    svg_w = 850
    top_margin = 55 if hw_label else 45
    n_panels = len(GROUPS)
    panel_h = 250
    panel_gap = 65
    bottom_margin = 80
    svg_h = top_margin + panel_h * n_panels + panel_gap * (n_panels - 1) + bottom_margin

    xl = 70
    xr = 830

    if not points:
        return "<svg></svg>"
    log_min = FIXED_LOG_X_MIN
    log_max = FIXED_LOG_X_MAX

    mid_x = svg_w / 2
    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')

    L.append(
        f'  <text x="{mid_x}" y="18" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'Encode Speed vs Compression Ratio (geomean across corpus)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="34" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    last_bot = top_margin
    for i, (group_name, _file_set) in enumerate(GROUPS):
        p_top = top_margin + i * (panel_h + panel_gap)
        p_bot = p_top + panel_h
        y_lo, y_hi = FIXED_Y_RANGES[group_name]
        render_panel(L, points, data, group_name,
                     xl, xr, p_top, p_bot, log_min, log_max, y_lo, y_hi)
        last_bot = p_bot

    # Y-axis label (shared, centered across all panels)
    y_mid = (top_margin + last_bot) / 2
    L.append(
        f'  <text x="16" y="{y_mid}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="11" font-weight="600"'
        f' transform="rotate(-90,16,{y_mid})">compression ratio</text>'
    )

    # Legend
    leg_y = last_bot + 40
    legend_items = [(k, LABELS[k]) for k in CODEC_ORDER if k in data]
    item_widths = [len(label) * 6.2 + 24 for _, label in legend_items]
    gap = 12

    # Split into two balanced rows
    total_w = sum(item_widths) + gap * (len(legend_items) - 1)
    if total_w > svg_w - 40 and len(legend_items) > 2:
        split = (len(legend_items) + 1) // 2
        rows_of_items = [legend_items[:split], legend_items[split:]]
    else:
        rows_of_items = [legend_items]

    for ri, row_items in enumerate(rows_of_items):
        rw = (sum(len(lb) * 6.2 + 24 for _, lb in row_items)
              + gap * (len(row_items) - 1))
        lx = mid_x - rw / 2
        ry = leg_y + ri * 18
        for key, label in row_items:
            color = COLORS[key]
            L.append(
                f'  <circle cx="{lx + 5:.0f}" cy="{ry}" r="4"'
                f' fill="{color}"/>'
            )
            L.append(
                f'  <text x="{lx + 13:.0f}" y="{ry + 3.5}" fill="#e6edf3"'
                f' font-size="10" font-weight="500">{label}</text>'
            )
            lx += len(label) * 6.2 + 24 + gap

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
    path = os.path.join(output_dir, "scatter.svg")
    with open(path, "w") as f:
        f.write(svg)
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
