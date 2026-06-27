#!/usr/bin/env python3
"""Generate encode speed vs compression ratio chart for small inputs (<256 KB).

Reads ~/.cache/zrip/L*/{codec}.jsonl, filters to small corpus files,
writes small.svg. One panel per source (dickens, hdfs, xml_collection),
X-axis: input size (log), Y-axis: encode MB/s.

Two shaded bands per codec: Fast (L-7..L2) and DFast (L3..L4).
Boundary lines labeled at the right edge.

Usage:
    python3 scripts/plot_small.py [output_dir]
"""
import json
import math
import os
import platform
import sys


CODEC_ORDER = ["C zstd", "zrip", "structured-zstd"]

COLORS = {
    "C zstd":          "#60a5fa",
    "zrip":            "#f87171",
    "structured-zstd": "#f59e0b",
}

LABELS = {
    "C zstd":          "C zstd (libzstd)",
    "zrip":            "zrip (Rust)",
    "structured-zstd": "structured-zstd 0.0.45 (Rust)",
}

SMALL_PREFIXES = ["dickens", "hdfs", "xml_collection"]
SMALL_SUFFIXES = ["_2k", "_8k", "_32k", "_128k"]
SMALL_SIZES = [2048, 8192, 32768, 131072]
SIZE_LABELS = ["2K", "8K", "32K", "128K"]

BAND_LEVELS = list(range(-7, 5))    # -7..4
INTERIOR_LEVELS = list(range(-6, 4))  # -6..3 (faint lines inside band)
LABEL_LEVEL = -1


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


def load_small_data():
    from profiles import cache_target
    cache_dir = os.path.join(
        os.environ.get("HOME", "."), ".cache", "zrip", cache_target())
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


def get_mbs(rows, name, level):
    matches = [r for r in rows if r["input"] == name and r["level"] == level]
    if matches:
        r = matches[0]
        return r["input_size"] / r["compress_ns"] * 1000
    return None


def band_envelope(rows, prefix, levels):
    """Return (lo_pts, hi_pts) for the min/max MB/s across levels at each size."""
    lo_pts = []
    hi_pts = []
    for si, suffix in enumerate(SMALL_SUFFIXES):
        name = prefix + suffix
        vals = []
        for level in levels:
            mbs = get_mbs(rows, name, level)
            if mbs is not None:
                vals.append(mbs)
        if vals:
            lo_pts.append((SMALL_SIZES[si], min(vals)))
            hi_pts.append((SMALL_SIZES[si], max(vals)))
    return lo_pts, hi_pts


def generate_svg(data):
    hw_label = detect_hardware()

    svg_w = 950
    n_panels = len(SMALL_PREFIXES)
    panel_w = 230
    panel_h = 400
    top_margin = 55 if hw_label else 45
    left_margin = 90
    panel_gap = 70
    bottom_margin = 70
    svg_h = top_margin + panel_h + bottom_margin

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
        f'Encode Throughput vs Input Size (small inputs)'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="34" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    x_start = left_margin

    log_min = math.log10(1500)
    log_max = math.log10(200000)

    for pi, prefix in enumerate(SMALL_PREFIXES):
        xl = x_start + pi * (panel_w + panel_gap)
        xr = xl + panel_w
        p_top = top_margin
        p_bot = p_top + panel_h
        pw = xr - xl
        pcx = (xl + xr) / 2

        panel_max = 0
        for codec in CODEC_ORDER:
            rows = data.get(codec, [])
            for level in BAND_LEVELS:
                for suffix in SMALL_SUFFIXES:
                    name = prefix + suffix
                    mbs = get_mbs(rows, name, level)
                    if mbs is not None and mbs > panel_max:
                        panel_max = mbs
        y_max_mbs = panel_max * 1.15

        L.append(
            f'  <rect x="{xl}" y="{p_top}" width="{pw}" height="{panel_h}"'
            f' fill="#161b22" rx="4"/>'
        )

        title = prefix.replace("_", " ")
        L.append(
            f'  <text x="{pcx}" y="{p_top - 8}" text-anchor="middle" fill="#e6edf3"'
            f' font-size="12" font-weight="600">{title}</text>'
        )

        def map_x(size):
            frac = (math.log10(size) - log_min) / (log_max - log_min)
            return xl + frac * pw

        def map_y(mbs):
            frac = mbs / y_max_mbs
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

        y_step = nice_y_step(y_max_mbs)
        v = y_step
        while v < y_max_mbs:
            yy = map_y(v)
            if p_top + 5 < yy < p_bot - 5:
                L.append(
                    f'  <line x1="{xl}" y1="{yy:.1f}" x2="{xr}" y2="{yy:.1f}"'
                    f' stroke="#21262d" stroke-width="1"/>'
                )
                L.append(
                    f'  <text x="{xl - 8}" y="{yy:.1f}" text-anchor="end"'
                    f' dominant-baseline="middle" fill="#7d8590" font-size="9">'
                    f'{int(v)}</text>'
                )
            v += y_step

        for codec in CODEC_ORDER:
            rows = data.get(codec, [])
            if not rows:
                continue
            color = COLORS[codec]

            lo_pts, hi_pts = band_envelope(rows, prefix, BAND_LEVELS)
            if not lo_pts:
                continue

            # Shaded fill between lo and hi
            fwd = [f"{map_x(sz):.1f},{map_y(mbs):.1f}" for sz, mbs in hi_pts]
            rev = [f"{map_x(sz):.1f},{map_y(mbs):.1f}" for sz, mbs in reversed(lo_pts)]
            poly = "M" + "L".join(fwd) + "L" + "L".join(rev) + "Z"
            L.append(
                f'  <path d="{poly}" fill="{color}" fill-opacity="0.15"/>'
            )

            # Faint interior lines (L-6..L1), label L-1
            for level in INTERIOR_LEVELS:
                pts = []
                for si, suffix in enumerate(SMALL_SUFFIXES):
                    name = prefix + suffix
                    mbs = get_mbs(rows, name, level)
                    if mbs is not None:
                        pts.append((SMALL_SIZES[si], mbs))
                if len(pts) > 1:
                    parts = []
                    for i, (sz, mbs) in enumerate(pts):
                        cmd = "M" if i == 0 else "L"
                        parts.append(f"{cmd}{map_x(sz):.1f},{map_y(mbs):.1f}")
                    L.append(
                        f'  <path d="{"".join(parts)}" fill="none"'
                        f' stroke="{color}" stroke-width="0.7" stroke-opacity="0.35"/>'
                    )
                if level == LABEL_LEVEL and pts:
                    last_sz, last_mbs = pts[-1]
                    xx = map_x(last_sz)
                    yy = map_y(last_mbs)
                    lbl = f"L{LABEL_LEVEL}" if LABEL_LEVEL >= 0 else f"L−{abs(LABEL_LEVEL)}"
                    L.append(
                        f'  <text x="{xx + 6:.1f}" y="{yy + 3:.1f}" text-anchor="start"'
                        f' fill="{color}" font-size="7" font-weight="600"'
                        f' fill-opacity="0.5">{lbl}</text>'
                    )

            # Boundary lines: L-7 (solid, top) and L4 (dashed, bottom)
            for pts, dash, label in [
                (hi_pts, "", "L−7"),
                (lo_pts, ' stroke-dasharray="4,3"', "L4"),
            ]:
                if len(pts) > 1:
                    parts = []
                    for i, (sz, mbs) in enumerate(pts):
                        cmd = "M" if i == 0 else "L"
                        parts.append(f"{cmd}{map_x(sz):.1f},{map_y(mbs):.1f}")
                    L.append(
                        f'  <path d="{"".join(parts)}" fill="none"'
                        f' stroke="{color}" stroke-width="1.5"{dash}/>'
                    )

                for sz, mbs in pts:
                    L.append(
                        f'  <circle cx="{map_x(sz):.1f}" cy="{map_y(mbs):.1f}" r="2.5"'
                        f' fill="{color}" stroke="#0d1117" stroke-width="0.8"/>'
                    )

                last_sz, last_mbs = pts[-1]
                xx = map_x(last_sz)
                yy = map_y(last_mbs)
                L.append(
                    f'  <text x="{xx + 6:.1f}" y="{yy + 3:.1f}" text-anchor="start"'
                    f' fill="{color}" font-size="7" font-weight="600">{label}</text>'
                )

    # Shared Y-axis label
    y_mid = top_margin + panel_h / 2
    L.append(
        f'  <text x="20" y="{y_mid}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="11" font-weight="600"'
        f' transform="rotate(-90,20,{y_mid})">encode MB/s</text>'
    )

    # Legend
    leg_y = top_margin + panel_h + 40
    legend_items = []
    for codec in CODEC_ORDER:
        if codec in data:
            legend_items.append(("codec", codec, LABELS[codec]))
    legend_items.append(("band", "fast", "L−7..L4 range"))
    legend_items.append(("line", "solid", "solid = L−7"))
    legend_items.append(("line", "dash", "dashed = L4"))

    rw = sum(len(lb) * 6.2 + 24 for _, _, lb in legend_items) + 12 * (len(legend_items) - 1)
    lx = mid_x - rw / 2
    for kind, key, label in legend_items:
        if kind == "codec":
            color = COLORS[key]
            L.append(
                f'  <circle cx="{lx + 5:.0f}" cy="{leg_y}" r="4" fill="{color}"/>'
            )
            L.append(
                f'  <text x="{lx + 13:.0f}" y="{leg_y + 3.5}" fill="#e6edf3"'
                f' font-size="10" font-weight="500">{label}</text>'
            )
        elif kind == "band":
            L.append(
                f'  <rect x="{lx:.0f}" y="{leg_y - 5}" width="14" height="10"'
                f' fill="#7d8590" fill-opacity="0.25" rx="2"/>'
            )
            L.append(
                f'  <text x="{lx + 18:.0f}" y="{leg_y + 3.5}" fill="#7d8590"'
                f' font-size="10">{label}</text>'
            )
        elif kind == "line":
            dash = "" if key == "solid" else " stroke-dasharray='4,3'"
            L.append(
                f'  <line x1="{lx:.0f}" y1="{leg_y}" x2="{lx + 14:.0f}" y2="{leg_y}"'
                f' stroke="#7d8590" stroke-width="1.5"{dash}/>'
            )
            L.append(
                f'  <text x="{lx + 18:.0f}" y="{leg_y + 3.5}" fill="#7d8590"'
                f' font-size="10">{label}</text>'
            )
        lx += len(label) * 6.2 + 24 + 12

    L.append("</svg>")
    return "\n".join(L) + "\n"


def nice_y_step(max_val):
    raw = max_val / 5
    mag = 10 ** math.floor(math.log10(max(raw, 1e-9)))
    for s in [1, 2, 5, 10]:
        step = s * mag
        if max_val / step <= 7:
            return step
    return mag * 10


def main():
    data = load_small_data()
    if not data:
        print("No small-input data in ~/.cache/zrip/", file=sys.stderr)
        sys.exit(1)

    svg = generate_svg(data)

    arch = platform.machine() or "x86_64"
    output_dir = sys.argv[1] if len(sys.argv) > 1 else f"doc/charts/{arch}"
    os.makedirs(output_dir, exist_ok=True)
    path = os.path.join(output_dir, "small.svg")
    with open(path, "w") as f:
        f.write(svg)
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
