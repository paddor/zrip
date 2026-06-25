"""Codec profiles for chart scripts.

Each profile defines CODEC_ORDER, COLORS, LABELS, LEVEL_FILTER, and
a cache target subdirectory. Scripts call `apply_profile()` at import
time to override module-level constants when --profile is passed.

The cache layout is ~/.cache/zrip/{target}/L{level}/{codec}.jsonl.
Native profiles use platform.machine() as target. Custom profiles
(e.g. wasm32) specify the target explicitly.
"""
import platform


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
            "structured-zstd": "structured-zstd 0.0.44",
            "zrip paranoid":   "zrip paranoid (no SIMD)",
        },
        "LEVEL_FILTER": {},
        "hw_label": "wasmtime (wasm32-wasip1)",
    },
}

_ACTIVE = {}


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
