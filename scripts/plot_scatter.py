#!/usr/bin/env python3
"""Generate encode speed vs compression ratio scatter plot from cached bench data.

Reads ~/.cache/zrip/{codec}.jsonl, writes scatter.svg.
Two panels stacked vertically: Compressible (top), Incompressible (bottom),
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


CODEC_ORDER = ["C zstd", "zrip", "structured-zstd", "lz4rip"]

COLORS = {
    "C zstd":          "#60a5fa",
    "zrip":            "#f87171",
    "structured-zstd": "#f59e0b",
    "lz4rip":          "#c084fc",
}

LABELS = {
    "C zstd":          "C zstd 1.5.7 (libzstd)",
    "zrip":            "zrip (safe Rust)",
    "structured-zstd": "structured-zstd 0.0.37 (unsafe Rust)",
    "lz4rip":          "lz4rip 0.3.1 (safe Rust, LZ4)",
}

COMPRESSIBLE = {
    "dickens.txt", "hdfs.json", "nci", "xml_collection.xml", "webster",
    "samba", "reymont.pdf", "mozilla", "compression_34k.txt",
    "compression_65k.txt", "compression_66k_JSON.txt", "osdb",
}
INCOMPRESSIBLE = {"sao", "x-ray", "mr"}

MIN_FILE_SIZE = 10_000


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


def compute_points(data):
    """For each (codec, level, group), compute geomean encode MB/s and ratio."""
    groups = [("Compressible", COMPRESSIBLE), ("Incompressible", INCOMPRESSIBLE)]
    points = {}

    for codec in CODEC_ORDER:
        rows = data.get(codec, [])
        levels = sorted(set(r["level"] for r in rows))
        for level in levels:
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
                 log_min, log_max):
    pw = xr - xl
    panel_h = p_bot - p_top

    ratios = [v[1] for k, v in points.items() if k[2] == group_name]
    if not ratios:
        return
    rmin = min(ratios)
    rmax = max(ratios)
    margin = (rmax - rmin) * 0.15 if rmax > rmin else rmax * 0.1
    ratio_min = rmin - margin
    ratio_max = rmax + margin

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
        f' fill="#7d8590" font-size="10">encode MB/s</text>'
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

        # Deduplicate nearly-identical points (e.g. lz4rip across levels)
        deduped = [codec_points[0]]
        for cp in codec_points[1:]:
            prev = deduped[-1]
            if (abs(cp[1] - prev[1]) / prev[1] > 0.02
                    or abs(cp[2] - prev[2]) / prev[2] > 0.02):
                deduped.append(cp)
        if len(deduped) == 1 and len(codec_points) > 1:
            codec_points = deduped

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
    panel_h = 300
    panel_gap = 55
    bottom_margin = 60
    svg_h = top_margin + panel_h * 2 + panel_gap + bottom_margin

    xl = 70
    xr = 830

    panel1_top = top_margin
    panel1_bot = panel1_top + panel_h
    panel2_top = panel1_bot + panel_gap
    panel2_bot = panel2_top + panel_h

    # Shared X-axis (log encode MB/s)
    all_enc = [v[0] for v in points.values()]
    if not all_enc:
        return "<svg></svg>"
    enc_min = min(all_enc) * 0.85
    enc_max = max(all_enc) * 1.15
    log_min = math.log10(enc_min)
    log_max = math.log10(enc_max)

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

    render_panel(L, points, data, "Compressible",
                 xl, xr, panel1_top, panel1_bot, log_min, log_max)
    render_panel(L, points, data, "Incompressible",
                 xl, xr, panel2_top, panel2_bot, log_min, log_max)

    # Y-axis label (shared, centered between panels)
    y_mid = (panel1_top + panel2_bot) / 2
    L.append(
        f'  <text x="16" y="{y_mid}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="11" font-weight="600"'
        f' transform="rotate(-90,16,{y_mid})">compression ratio</text>'
    )

    # Legend
    leg_y = panel2_bot + 40
    legend_items = [(k, LABELS[k]) for k in CODEC_ORDER if k in data]
    total_w = (sum(len(label) * 6.2 + 24 for _, label in legend_items)
               + 12 * (len(legend_items) - 1))
    lx = mid_x - total_w / 2
    for key, label in legend_items:
        color = COLORS[key]
        L.append(
            f'  <circle cx="{lx + 5:.0f}" cy="{leg_y}" r="4"'
            f' fill="{color}"/>'
        )
        L.append(
            f'  <text x="{lx + 13:.0f}" y="{leg_y + 3.5}" fill="#e6edf3"'
            f' font-size="10" font-weight="500">{label}</text>'
        )
        lx += len(label) * 6.2 + 24 + 12

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
