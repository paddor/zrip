"""Codec profiles for chart scripts.

Each profile defines CODEC_ORDER, COLORS, LABELS, LEVEL_FILTER, and
a cache target subdirectory. Scripts call `apply_profile()` at import
time to override module-level constants when --profile is passed.

The cache layout is ~/.cache/zrip/{target}/L{level}/{codec}.jsonl.
Native profiles use platform.machine() as target. Custom profiles
(e.g. wasm32) specify the target explicitly.
"""
import os
import platform
import subprocess


def cache_target():
    """Return the cache subdirectory for the current profile."""
    return _ACTIVE.get("target", platform.machine() or "x86_64")


PROFILES = {
    "wasm32": {
        "target": "wasm32",
        "CODEC_ORDER": [
            "C zstd", "zrip", "structured-zstd",
            "zrip paranoid",
        ],
        "COLORS": {
            "C zstd":          "#60a5fa",
            "zrip":            "#f87171",
            "structured-zstd": "#f59e0b",
            "zrip paranoid":   "#f472b6",
        },
        "COLORS_TUPLE": {
            "C zstd":          ("#60a5fa", "#4680c4"),
            "zrip":            ("#f87171", "#c45050"),
            "structured-zstd": ("#f59e0b", "#c47d08"),
            "zrip paranoid":   ("#f472b6", "#c05a92"),
        },
        "LABELS": {
            "C zstd":          "C zstd 1.5.7 (libzstd wasm)",
            "zrip":            "zrip (SIMD128)",
            "structured-zstd": "structured-zstd 0.0.49",
            "zrip paranoid":   "zrip paranoid (safe Rust, no unsafe)",
        },
        "LEVEL_FILTER": {},
        "hw_label": "wasmtime (wasm32-wasip1), Linux VM on Intel Core i7-8700B @ 3.20GHz",
    },
}

_ACTIVE = {}


def detect_hardware():
    """Return a hardware subtitle string, or None."""
    if "hw_label" in _ACTIVE:
        return _ACTIVE["hw_label"]
    try:
        cpu = os.environ.get("ZRIP_CPU")
        if not cpu:
            if platform.system() == "Darwin":
                try:
                    cpu = subprocess.check_output(
                        ["sysctl", "-n", "machdep.cpu.brand_string"],
                        text=True,
                    ).strip()
                except Exception:
                    pass
            else:
                try:
                    for line in open("/proc/cpuinfo"):
                        if line.startswith("model name"):
                            cpu = line.split(":", 1)[1].strip()
                            cpu = cpu.replace("(R)", "").replace("(TM)", "").replace("CPU ", "")
                            break
                except OSError:
                    pass
        if not cpu:
            cpu = os.environ.get("ZRIP_HW_EXTRAS")
            return cpu
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


def get_profile(name):
    if name not in PROFILES:
        raise ValueError(f"unknown profile: {name!r} (available: {list(PROFILES)})")
    return PROFILES[name]


def apply_profile(argv):
    """If --profile <name> is in argv, activate that profile and remove
    the flag from argv. Returns the profile dict or None."""
    global _ACTIVE
    if "--profile" not in argv:
        return None
    idx = argv.index("--profile")
    p = get_profile(argv[idx + 1])
    del argv[idx:idx + 2]
    _ACTIVE = p
    return p
