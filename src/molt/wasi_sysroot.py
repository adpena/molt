"""Dependency-free WASI sysroot identity and layout authority.

This module deliberately lives below :mod:`molt.cli`: bootstrap, installer, and
CI verification paths must be able to validate a sysroot before the optional
Python CLI dependency set is installed.
"""

from __future__ import annotations

from pathlib import Path
import re


WASI_TARGET_INCLUDE_DIRS = ("wasm32-wasip1", "wasm32-wasi")


def _normalize_target_include_path(candidate: Path) -> Path | None:
    if candidate.name not in WASI_TARGET_INCLUDE_DIRS:
        return None
    if candidate.parent.name != "include":
        return None
    if not (candidate / "errno.h").exists():
        return None
    return candidate.parent.parent.resolve(strict=False)


def normalize_wasi_sysroot(path: str | Path | None) -> Path | None:
    """Return the canonical sysroot root for every supported WASI layout."""

    if path is None:
        return None
    candidate = Path(path).expanduser()
    target_include_root = _normalize_target_include_path(candidate)
    if target_include_root is not None:
        return target_include_root
    roots = [candidate]
    if candidate.name == "include":
        roots.append(candidate.parent)
    for root in roots:
        for target in WASI_TARGET_INCLUDE_DIRS:
            if (root / "include" / target / "errno.h").exists():
                return root.resolve(strict=False)
        if (root / "include" / "errno.h").exists():
            return root.resolve(strict=False)
    return None


def wasi_sysroot_llvm_version(sysroot: Path) -> str | None:
    """Read the LLVM producer version attested by a WASI SDK sysroot."""

    version_file = sysroot / "VERSION"
    if not version_file.is_file():
        return None
    try:
        text = version_file.read_text(encoding="utf-8")
    except OSError:
        return None
    match = re.search(r"^llvm-version:\s*(\d+\.\d+(?:\.\d+)?)\s*$", text, re.MULTILINE)
    return match.group(1) if match is not None else None
