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
    search the selected target directory and the repository target directory;
    callers that isolate a test target pass it explicitly rather than growing a
    second locator.
    """
    requested = os.environ.get("MOLT_WASM_HOST_BIN", "").strip()
    if requested:
        path = Path(requested).expanduser()
        return os.fspath(path) if path.is_file() else None

    target_dirs: list[Path] = []
    if target_dir is not None:
        target_dirs.append(target_dir)
    configured = os.environ.get("CARGO_TARGET_DIR", "").strip()
    if configured:
        target_dirs.append(Path(configured).expanduser())
    target_dirs.append(root / "target")

    seen: set[str] = set()
    exe_name = molt_wasm_host_exe_name()
    for candidate_dir in target_dirs:
        key = os.path.normcase(os.fspath(candidate_dir.resolve(strict=False)))
        if key in seen:
            continue
        seen.add(key)
        candidate = candidate_dir / cargo_profile / exe_name
        if candidate.is_file():
            return os.fspath(candidate)
    return None
