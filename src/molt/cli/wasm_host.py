"""Locate the Rust-owned WebAssembly host executable."""

from __future__ import annotations

import os
from pathlib import Path

from molt.dx import project_cargo_target_dir


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
    select the caller's target directory (relative to ``root``), else the
    project's Cargo target (`molt.dx.project_cargo_target_dir`). A missing
    selected binary never falls through to another build, and a selected
    binary older than its sources is not returned: on 2026-11-07 a host built
    on 11-01 failed a landing with an error its 11-06 fix had removed.
    """
    requested = os.environ.get("MOLT_WASM_HOST_BIN", "").strip()
    if requested:
        path = Path(requested).expanduser()
        return os.fspath(path.absolute()) if path.is_file() else None

    root = root.absolute()
    if target_dir is None:
        target_dir = project_cargo_target_dir(root, os.environ)
    target_dir = target_dir.expanduser()
    if not target_dir.is_absolute():
        target_dir = root / target_dir
    candidate = target_dir / cargo_profile / molt_wasm_host_exe_name()
    if not candidate.is_file():
        return None
    from molt.cli.cargo_build_reads import cargo_output_is_current

    if not cargo_output_is_current(candidate, dep_info_base=root):
        return None
    return os.fspath(candidate.absolute())
