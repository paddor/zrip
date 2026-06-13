#!/usr/bin/env python3
"""
Generate dark-theme SVG charts from zrip_bench JSON output.

Usage:
    cargo run --example zrip_bench --release > bench.json
    python3 scripts/plot_bench.py bench.json
"""
import json
import sys
import os

BG = "#1e1e2e"
FG = "#cdd6f4"
GRID = "#45475a"
ZRIP_COLOR = "#89b4fa"
ZSTD_COLOR = "#f38ba8"
BAR_HEIGHT = 20
BAR_GAP = 6
CHART_WIDTH = 700
LEFT_MARGIN = 180
RIGHT_MARGIN = 40


def read_cpuinfo():
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if line.startswith("model name"):
                    return line.split(":", 1)[1].strip()
    except FileNotFoundError:
        pass
    return "unknown CPU"


def svg_header(width, height, title):
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"
     viewBox="0 0 {width} {height}" style="background:{BG}">
<style>
  text {{ font-family: monospace; fill: {FG}; font-size: 12px; }}
  .title {{ font-size: 14px; font-weight: bold; }}
  .label {{ font-size: 11px; text-anchor: end; }}
  .value {{ font-size: 10px; }}
</style>
<text x="{width//2}" y="24" text-anchor="middle" class="title">{title}</text>
"""


def svg_footer():
    return "</svg>\n"


def throughput_mbps(input_size, ns):
    if ns == 0:
        return 0.0
    return (input_size / 1e6) / (ns / 1e9)


def render_compress_chart(data, output_dir):
    levels = sorted(set(r["level"] for r in data))
    corpora = []
    seen = set()
    for r in data:
        if r["corpus"] not in seen:
            corpora.append(r["corpus"])
            seen.add(r["corpus"])

    for level in levels:
        rows = [r for r in data if r["level"] == level]
        n = len(rows)
        height = 60 + n * (BAR_HEIGHT * 2 + BAR_GAP * 2)
        width = CHART_WIDTH

        svg = svg_header(width, height, f"Compress throughput (level {level})")

        max_tp = 0
        for r in rows:
            tp_zrip = throughput_mbps(r["input_size"], r["zrip_compress_ns"])
            tp_zstd = throughput_mbps(r["input_size"], r["zstd_compress_ns"])
            max_tp = max(max_tp, tp_zrip, tp_zstd)

        bar_width = width - LEFT_MARGIN - RIGHT_MARGIN
        y = 50

        for r in rows:
            tp_zrip = throughput_mbps(r["input_size"], r["zrip_compress_ns"])
            tp_zstd = throughput_mbps(r["input_size"], r["zstd_compress_ns"])

            svg += f'<text x="{LEFT_MARGIN - 8}" y="{y + BAR_HEIGHT - 4}" class="label">{r["corpus"]}</text>\n'

            w_zrip = (tp_zrip / max_tp) * bar_width if max_tp > 0 else 0
            w_zstd = (tp_zstd / max_tp) * bar_width if max_tp > 0 else 0

            svg += f'<rect x="{LEFT_MARGIN}" y="{y}" width="{w_zrip:.1f}" height="{BAR_HEIGHT}" fill="{ZRIP_COLOR}" rx="2"/>\n'
            svg += f'<text x="{LEFT_MARGIN + w_zrip + 4}" y="{y + BAR_HEIGHT - 5}" class="value">zrip {tp_zrip:.0f} MB/s</text>\n'
            y += BAR_HEIGHT + BAR_GAP

            svg += f'<rect x="{LEFT_MARGIN}" y="{y}" width="{w_zstd:.1f}" height="{BAR_HEIGHT}" fill="{ZSTD_COLOR}" rx="2"/>\n'
            svg += f'<text x="{LEFT_MARGIN + w_zstd + 4}" y="{y + BAR_HEIGHT - 5}" class="value">zstd {tp_zstd:.0f} MB/s</text>\n'
            y += BAR_HEIGHT + BAR_GAP

        svg += svg_footer()
        path = os.path.join(output_dir, f"compress_level_{level}.svg")
        with open(path, "w") as f:
            f.write(svg)
        print(f"  wrote {path}")


def render_decompress_chart(data, output_dir):
    corpora = []
    seen = set()
    for r in data:
        if r["corpus"] not in seen:
            corpora.append(r["corpus"])
            seen.add(r["corpus"])

    rows = [r for r in data if r["level"] == 1]
    n = len(rows)
    height = 60 + n * (BAR_HEIGHT * 2 + BAR_GAP * 2)
    width = CHART_WIDTH

    svg = svg_header(width, height, "Decompress throughput (level 1 input)")

    max_tp = 0
    for r in rows:
        tp_zrip = throughput_mbps(r["input_size"], r["zrip_decompress_ns"])
        tp_zstd = throughput_mbps(r["input_size"], r["zstd_decompress_ns"])
        max_tp = max(max_tp, tp_zrip, tp_zstd)

    bar_width = width - LEFT_MARGIN - RIGHT_MARGIN
    y = 50

    for r in rows:
        tp_zrip = throughput_mbps(r["input_size"], r["zrip_decompress_ns"])
        tp_zstd = throughput_mbps(r["input_size"], r["zstd_decompress_ns"])

        svg += f'<text x="{LEFT_MARGIN - 8}" y="{y + BAR_HEIGHT - 4}" class="label">{r["corpus"]}</text>\n'

        w_zrip = (tp_zrip / max_tp) * bar_width if max_tp > 0 else 0
        w_zstd = (tp_zstd / max_tp) * bar_width if max_tp > 0 else 0

        svg += f'<rect x="{LEFT_MARGIN}" y="{y}" width="{w_zrip:.1f}" height="{BAR_HEIGHT}" fill="{ZRIP_COLOR}" rx="2"/>\n'
        svg += f'<text x="{LEFT_MARGIN + w_zrip + 4}" y="{y + BAR_HEIGHT - 5}" class="value">zrip {tp_zrip:.0f} MB/s</text>\n'
        y += BAR_HEIGHT + BAR_GAP

        svg += f'<rect x="{LEFT_MARGIN}" y="{y}" width="{w_zstd:.1f}" height="{BAR_HEIGHT}" fill="{ZSTD_COLOR}" rx="2"/>\n'
        svg += f'<text x="{LEFT_MARGIN + w_zstd + 4}" y="{y + BAR_HEIGHT - 5}" class="value">zstd {tp_zstd:.0f} MB/s</text>\n'
        y += BAR_HEIGHT + BAR_GAP

    svg += svg_footer()
    path = os.path.join(output_dir, "decompress.svg")
    with open(path, "w") as f:
        f.write(svg)
    print(f"  wrote {path}")


def render_ratio_chart(data, output_dir):
    rows = [r for r in data if r["level"] == 1]
    n = len(rows)
    height = 60 + n * (BAR_HEIGHT * 2 + BAR_GAP * 2)
    width = CHART_WIDTH

    svg = svg_header(width, height, "Compression ratio (level 1)")

    max_ratio = 0
    for r in rows:
        ratio_zrip = r["input_size"] / max(r["zrip_compressed_size"], 1)
        ratio_zstd = r["input_size"] / max(r["zstd_compressed_size"], 1)
        max_ratio = max(max_ratio, ratio_zrip, ratio_zstd)

    bar_width = width - LEFT_MARGIN - RIGHT_MARGIN
    y = 50

    for r in rows:
        ratio_zrip = r["input_size"] / max(r["zrip_compressed_size"], 1)
        ratio_zstd = r["input_size"] / max(r["zstd_compressed_size"], 1)

        svg += f'<text x="{LEFT_MARGIN - 8}" y="{y + BAR_HEIGHT - 4}" class="label">{r["corpus"]}</text>\n'

        w_zrip = (ratio_zrip / max_ratio) * bar_width if max_ratio > 0 else 0
        w_zstd = (ratio_zstd / max_ratio) * bar_width if max_ratio > 0 else 0

        svg += f'<rect x="{LEFT_MARGIN}" y="{y}" width="{w_zrip:.1f}" height="{BAR_HEIGHT}" fill="{ZRIP_COLOR}" rx="2"/>\n'
        svg += f'<text x="{LEFT_MARGIN + w_zrip + 4}" y="{y + BAR_HEIGHT - 5}" class="value">zrip {ratio_zrip:.2f}x</text>\n'
        y += BAR_HEIGHT + BAR_GAP

        svg += f'<rect x="{LEFT_MARGIN}" y="{y}" width="{w_zstd:.1f}" height="{BAR_HEIGHT}" fill="{ZSTD_COLOR}" rx="2"/>\n'
        svg += f'<text x="{LEFT_MARGIN + w_zstd + 4}" y="{y + BAR_HEIGHT - 5}" class="value">zstd {ratio_zstd:.2f}x</text>\n'
        y += BAR_HEIGHT + BAR_GAP

    svg += svg_footer()
    path = os.path.join(output_dir, "ratio.svg")
    with open(path, "w") as f:
        f.write(svg)
    print(f"  wrote {path}")


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <bench.json> [output_dir]")
        sys.exit(1)

    with open(sys.argv[1]) as f:
        data = json.load(f)

    output_dir = sys.argv[2] if len(sys.argv) > 2 else "."
    os.makedirs(output_dir, exist_ok=True)

    cpu = read_cpuinfo()
    print(f"CPU: {cpu}")
    print(f"Generating charts from {len(data)} measurements...")

    render_compress_chart(data, output_dir)
    render_decompress_chart(data, output_dir)
    render_ratio_chart(data, output_dir)

    print("Done.")


if __name__ == "__main__":
    main()
