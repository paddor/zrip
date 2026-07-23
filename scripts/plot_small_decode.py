#!/usr/bin/env python3
"""Generate decode speed vs input size chart for small inputs (<256 KB).

All decoders decompress the same C zstd L3 bitstream (--decode-only mode).
Reads ~/.cache/zrip/small/decode_cmp/L3/{codec}.jsonl.
One panel per source, X-axis: input size (log), Y-axis: decode MB/s,
one line per codec.

Usage:
    python3 scripts/plot_small_decode.py [output_dir]
"""
import json
import math
import os
import platform
import sys


CODEC_ORDER = ["C zstd", "zrip", "zrip paranoid", "structured-zstd", "ruzstd"]

COLORS = {
    "C zstd":          "#60a5fa",
    "zrip":            "#f87171",
    "zrip paranoid":   "#f472b6",
    "structured-zstd": "#f59e0b",
    "ruzstd":          "#4ade80",
}

LABELS = {
    "C zstd":          "libzstd v1.5.7 (C)",
    "zrip":            "zrip (safe SIMD + encapsulated unsafe)",
    "zrip paranoid":   "zrip paranoid (safe SIMD, no unsafe)",
    "structured-zstd": "structured-zstd v0.0.49 (unsafe)",
    "ruzstd":          "ruzstd v0.8.3 (safe)",
}

SMALL_PREFIXES = ["dickens", "nci", "xml", "x-ray"]
SMALL_SUFFIXES = ["_512", "_1k", "_2k", "_4k", "_8k", "_16k", "_32k", "_64k", "_128k"]
SMALL_SIZES = [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072]
SIZE_LABELS = ["512", "1K", "2K", "4K", "8K", "16K", "32K", "64K", "128K"]

LEVEL = 3


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


def load_data():
    from profiles import cache_target
    base_dir = os.path.join(
        os.environ.get("HOME", "."), ".cache", "zrip", cache_target())
    decode_cmp_dir = os.path.join(base_dir, "small", "decode_cmp")
    small_dir = os.path.join(base_dir, "small")
    small_names = set()
    for prefix in SMALL_PREFIXES:
        for suffix in SMALL_SUFFIXES:
            small_names.add(prefix + suffix)
    data = {}
    has_decode_cmp = os.path.isdir(decode_cmp_dir)
    level_dir_name = f"L{LEVEL}"
    search_dirs = [decode_cmp_dir] if has_decode_cmp else [small_dir, base_dir]
    for cache_dir in search_dirs:
        level_dir = os.path.join(cache_dir, level_dir_name)
        if not os.path.isdir(level_dir):
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
                    if r["level"] == LEVEL:
                        data[codec][r["input"]] = r
    return {codec: list(seen.values()) for codec, seen in data.items()}, has_decode_cmp


def get_decode_mbs(rows, name):
    matches = [r for r in rows if r["input"] == name]
    if matches:
        r = matches[0]
        if r.get("decompress_ns") and r["decompress_ns"] > 0:
            return r["input_size"] / r["decompress_ns"] * 1000
    return None


def generate_svg(data, common_bitstream=False):
    hw_label = detect_hardware()

    n_panels = len(SMALL_PREFIXES)
    panel_w = 700
    panel_h = 220
    top_margin = 55 if hw_label else 45
    left_margin = 90
    row_gap = 50
    bottom_margin = 65
    svg_w = left_margin + panel_w + 40
    svg_h = top_margin + n_panels * panel_h + (n_panels - 1) * row_gap + bottom_margin

    mid_x = svg_w / 2

    subtitle = f"C zstd L{LEVEL} bitstream" if common_bitstream else f"L{LEVEL}"
    title = f"Decode Throughput vs Input Size ({subtitle})"

    L = []
    L.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {svg_w} {svg_h}"'
        f' font-family="system-ui, -apple-system, sans-serif">'
    )
    L.append(f'  <rect width="{svg_w}" height="{svg_h}" fill="#0d1117"/>')
    L.append(
        f'  <text x="{mid_x}" y="18" text-anchor="middle" fill="#e6edf3"'
        f' font-size="14" font-weight="700">'
        f'{title}'
        f'</text>'
    )
    if hw_label:
        L.append(
            f'  <text x="{mid_x}" y="34" text-anchor="middle" fill="#7d8590"'
            f' font-size="10">{hw_label}</text>'
        )

    log_min = math.log10(400)
    log_max = math.log10(200000)

    for pi, prefix in enumerate(SMALL_PREFIXES):
        xl = left_margin
        xr = xl + panel_w
        p_top = top_margin + pi * (panel_h + row_gap)
        p_bot = p_top + panel_h
        pw = xr - xl
        pcx = (xl + xr) / 2

        panel_max = 0
        for codec in CODEC_ORDER:
            rows = data.get(codec, [])
            for suffix in SMALL_SUFFIXES:
                name = prefix + suffix
                mbs = get_decode_mbs(rows, name)
                if mbs is not None and mbs > panel_max:
                    panel_max = mbs
        if panel_max == 0:
            continue
        y_max_mbs = panel_max * 1.15

        L.append(
            f'  <rect x="{xl}" y="{p_top}" width="{pw}" height="{panel_h}"'
            f' fill="#161b22" rx="4"/>'
        )

        panel_title = prefix.replace("_", " ")
        L.append(
            f'  <text x="{pcx}" y="{p_top - 8}" text-anchor="middle" fill="#e6edf3"'
            f' font-size="12" font-weight="600">{panel_title}</text>'
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

            pts = []
            for si, suffix in enumerate(SMALL_SUFFIXES):
                name = prefix + suffix
                mbs = get_decode_mbs(rows, name)
                if mbs is not None:
                    pts.append((SMALL_SIZES[si], mbs))
            if len(pts) < 2:
                continue

            parts = []
            for i, (sz, mbs) in enumerate(pts):
                cmd = "M" if i == 0 else "L"
                parts.append(f"{cmd}{map_x(sz):.1f},{map_y(mbs):.1f}")
            L.append(
                f'  <path d="{"".join(parts)}" fill="none"'
                f' stroke="{color}" stroke-width="1.8"/>'
            )

            for sz, mbs in pts:
                L.append(
                    f'  <circle cx="{map_x(sz):.1f}" cy="{map_y(mbs):.1f}" r="2.5"'
                    f' fill="{color}" stroke="#0d1117" stroke-width="0.8"/>'
                )

    total_h = n_panels * panel_h + (n_panels - 1) * row_gap
    y_mid = top_margin + total_h / 2
    L.append(
        f'  <text x="20" y="{y_mid}" text-anchor="middle" fill="#e6edf3"'
        f' font-size="11" font-weight="600"'
        f' transform="rotate(-90,20,{y_mid})">decode MB/s</text>'
    )

    leg_y = top_margin + total_h + 35
    legend_items = []
    for codec in CODEC_ORDER:
        if codec in data:
            legend_items.append((codec, LABELS[codec]))

    leg_gap = 12
    leg_pad = 24
    char_w = 6.2
    max_row_w = svg_w - 40

    rows = []
    cur_row = []
    cur_w = 0
    for key, label in legend_items:
        item_w = len(label) * char_w + leg_pad
        if cur_row and cur_w + leg_gap + item_w > max_row_w:
            rows.append(cur_row)
            cur_row = []
            cur_w = 0
        if cur_row:
            cur_w += leg_gap
        cur_row.append((key, label))
        cur_w += item_w
    if cur_row:
        rows.append(cur_row)

    for ri, row in enumerate(rows):
        ry = leg_y + ri * 18
        rw = sum(len(lb) * char_w + leg_pad for _, lb in row)
        rw += leg_gap * (len(row) - 1)
        lx = mid_x - rw / 2
        for key, label in row:
            color = COLORS[key]
            L.append(
                f'  <circle cx="{lx + 5:.0f}" cy="{ry}" r="4" fill="{color}"/>'
            )
            L.append(
                f'  <text x="{lx + 13:.0f}" y="{ry + 3.5}" fill="#e6edf3"'
                f' font-size="10" font-weight="500">{label}</text>'
            )
            lx += len(label) * char_w + leg_pad + leg_gap

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
    data, common_bitstream = load_data()
    if not data:
        print("No small-input decode data in ~/.cache/zrip/", file=sys.stderr)
        sys.exit(1)

    svg = generate_svg(data, common_bitstream)

    arch = platform.machine() or "x86_64"
    output_dir = sys.argv[1] if len(sys.argv) > 1 else f"doc/charts/{arch}"
    os.makedirs(output_dir, exist_ok=True)
    path = os.path.join(output_dir, "small_decode.svg")
    with open(path, "w") as f:
        f.write(svg)
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
