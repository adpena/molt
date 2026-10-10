"""Locate the Rust-owned WebAssembly host executable."""

from __future__ import annotations

import os
from pathlib import Path


def molt_wasm_host_exe_name() -> str:
    return "molt-wasm-host.exe" if os.name == "nt" else "molt-wasm-host"


def resolve_molt_wasm_host_binary(
    root: Path,
    *,
    cargo_profile: str,
    target_dir: Path | None = None,
) -> str | None:
    """Resolve the host built with the same Cargo profile as the runtime.

    ``MOLT_WASM_HOST_BIN`` is the explicit deployment authority.  Otherwise
    select the caller's target directory, CARGO_TARGET_DIR, or the repository
    target directory, in that order. A missing selected binary never falls
    through to another build. Relative Cargo target directories are relative to
    the source root, where callers run Cargo.
    """
    requested = os.environ.get("MOLT_WASM_HOST_BIN", "").strip()
    if requested:
        path = Path(requested).expanduser()
        return os.fspath(path.absolute()) if path.is_file() else None

    root = root.absolute()
    if target_dir is None:
        configured = os.environ.get("CARGO_TARGET_DIR", "").strip()
        target_dir = Path(configured) if configured else root / "target"
    target_dir = target_dir.expanduser()
    if not target_dir.is_absolute():
        target_dir = root / target_dir
    candidate = target_dir / cargo_profile / molt_wasm_host_exe_name()
    return os.fspath(candidate.absolute()) if candidate.is_file() else None
