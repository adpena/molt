"""Backend variants and executable names for publication and discovery.

Names identify discovery candidates, never artifact provenance or process
ownership. Admission still requires the content-bound build receipt.
"""

from __future__ import annotations

import os
from collections.abc import Mapping, Sequence


_TARGET_FEATURES = {
    "native": ("native-backend",),
    "wasm": ("wasm-backend",),
    "luau": ("luau-backend",),
    "rust": ("rust-backend",),
}
DEFAULT_BACKEND_FEATURES = _TARGET_FEATURES["native"]


def backend_features_for_target(
    *,
    is_wasm: bool,
    is_luau_transpile: bool,
    is_rust_transpile: bool,
    env: Mapping[str, str] | None = None,
) -> tuple[str, ...]:
    """Resolve the same feature identity for builds, names and cache keys."""
    source = os.environ if env is None else env
    if is_luau_transpile:
        target = "luau"
    elif is_rust_transpile:
        target = "rust"
    elif is_wasm:
        target = "wasm"
    else:
        target = "native"
    features = _TARGET_FEATURES[target]
    return (*features, "llvm") if source.get("MOLT_BACKEND") == "llvm" else features


def backend_executable_name(
    *, os_name: str, features: Sequence[str] | None = None
) -> str:
    """Name the Cargo output (None), or an independently published variant."""
    stem = "molt-backend"
    if features is not None:
        tag = "_".join(sorted(features)).replace("-", "_") or "default"
        stem += f".{tag}"
    return stem + (".exe" if os_name == "nt" else "")


_BACKEND_IMAGES = frozenset(
    backend_executable_name(os_name=os_name, features=features)
    for os_name in ("nt", "posix")
    for features in (
        None,
        (),
        *_TARGET_FEATURES.values(),
        *((*base, "llvm") for base in _TARGET_FEATURES.values()),
    )
)


def is_backend_executable_name(name: str) -> bool:
    """Recognize a backend image basename for sampling and quiescence only."""
    return name.strip().casefold() in _BACKEND_IMAGES
