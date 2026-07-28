"""Dependency-free WASI sysroot identity and layout authority.

This module deliberately lives below :mod:`molt.cli`: bootstrap, installer, and
CI verification paths must be able to validate a sysroot before the optional
Python CLI dependency set is installed.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re


WASI_TARGET_INCLUDE_DIRS = ("wasm32-wasip1", "wasm32-wasi")
WASI_TARGET_LIB_DIRS = ("wasm32-wasip1", "wasm32-wasi")


@dataclass(frozen=True, slots=True)
class WasiSysrootLayout:
    """Compiler root plus the exact target content roots beneath it."""

    root: Path
    include_roots: tuple[tuple[str, Path], ...]
    library_roots: tuple[tuple[str, Path], ...]
    version_file: Path | None

    def content_roots(self) -> tuple[tuple[str, Path], ...]:
        version = (
            ()
            if self.version_file is None
            else (("VERSION", self.version_file),)
        )
        return (*self.include_roots, *self.library_roots, *version)


def _root_from_target_include_path(candidate: Path) -> Path | None:
    if candidate.name not in WASI_TARGET_INCLUDE_DIRS:
        return None
    if candidate.parent.name != "include":
        return None
    if not (candidate / "errno.h").exists():
        return None
    return candidate.parent.parent.resolve(strict=False)


def _layout_for_root(root: Path) -> WasiSysrootLayout | None:
    resolved = root.resolve(strict=False)
    include = resolved / "include"
    target_includes = tuple(
        (f"include/{target}", include / target)
        for target in WASI_TARGET_INCLUDE_DIRS
        if (include / target / "errno.h").exists()
    )
    if target_includes:
        include_roots = target_includes
    elif (include / "errno.h").exists():
        include_roots = (("include", include),)
    else:
        return None
    library_roots = tuple(
        (f"lib/{target}", resolved / "lib" / target)
        for target in WASI_TARGET_LIB_DIRS
        if (resolved / "lib" / target).is_dir()
    )
    version = resolved / "VERSION"
    return WasiSysrootLayout(
        root=resolved,
        include_roots=include_roots,
        library_roots=library_roots,
        version_file=version if version.is_file() else None,
    )


def resolve_wasi_sysroot_layout(
    path: str | Path | None,
) -> WasiSysrootLayout | None:
    """Resolve every supported SDK/distro shape without widening its content."""

    if path is None:
        return None
    candidate = Path(path).expanduser()
    target_root = _root_from_target_include_path(candidate)
    if target_root is not None:
        return _layout_for_root(target_root)
    roots = [candidate]
    if candidate.name == "include":
        roots.append(candidate.parent)
    for root in roots:
        if layout := _layout_for_root(root):
            return layout
    return None


def normalize_wasi_sysroot(path: str | Path | None) -> Path | None:
    """Return the canonical sysroot root for every supported WASI layout."""

    layout = resolve_wasi_sysroot_layout(path)
    return None if layout is None else layout.root


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
