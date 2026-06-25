from __future__ import annotations

import hashlib
import json
import os
import subprocess
from pathlib import Path

from molt.cli.atomic_io import _atomic_write_text
from molt.cli.backend_cache import (
    _nm_candidate_binaries,
    _normalize_native_symbol_name,
)
from molt.cli.command_runtime import _run_completed_command
from molt.cli.models import _RuntimeArtifactState
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail
from molt.cli.runtime_build import _ensure_native_runtime_lib_ready_before_link


def _runtime_intrinsic_symbols_file(
    runtime_lib: Path,
) -> tuple[Path | None, str | None]:
    """Materialize the `molt_*` intrinsic symbols defined by a runtime staticlib.

    The per-app intrinsic resolver takes the address of every intrinsic the app
    reaches by name. Those addresses are resolved against this staticlib at link
    time, so the resolver must only reference intrinsics the staticlib actually
    defines; the native ``micro`` and ``full`` profiles intentionally differ.

    Extraction accepts the first ``nm`` candidate that exits cleanly and yields a
    non-empty ``molt_*`` text-symbol set. That lets LLVM ``nm`` win when a system
    ``nm`` cannot parse the staticlib's LTO bitcode.
    """
    try:
        stat = runtime_lib.stat()
    except OSError as exc:
        return None, f"runtime staticlib unreadable: {runtime_lib} ({exc})"
    cache_path = runtime_lib.with_name(
        f"{runtime_lib.name}.intrinsic_symbols.{stat.st_size}.{int(stat.st_mtime)}.txt"
    )
    if cache_path.exists():
        return cache_path, None
    # Use an absolute path so cwd=runtime_lib.parent cannot break path
    # resolution. Keep the timeout generous: the native staticlib can be large.
    runtime_lib_abs = runtime_lib.resolve()
    failures: list[str] = []
    symbols: set[str] = set()
    for nm_bin in _nm_candidate_binaries():
        try:
            result = _run_completed_command(
                [nm_bin, "--defined-only", str(runtime_lib_abs)],
                capture_output=True,
                timeout=120,
                env=None,
                cwd=runtime_lib_abs.parent,
                memory_guard_prefix="MOLT_BUILD",
            )
        except (OSError, subprocess.SubprocessError) as exc:
            failures.append(f"{nm_bin}: {exc}")
            continue
        if result.returncode != 0:
            stderr_lines = (result.stderr or "").strip().splitlines()
            detail = stderr_lines[-1] if stderr_lines else "no stderr"
            failures.append(f"{nm_bin}: exit {result.returncode} ({detail})")
            continue
        candidate_symbols: set[str] = set()
        for raw_line in result.stdout.splitlines():
            parts = raw_line.split()
            if len(parts) < 2:
                continue
            kind = parts[-2]
            name = _normalize_native_symbol_name(parts[-1])
            if kind in ("T", "t") and name.startswith("molt_"):
                candidate_symbols.add(name)
        if not candidate_symbols:
            failures.append(f"{nm_bin}: produced no molt_* text symbols")
            continue
        symbols = candidate_symbols
        break
    if not symbols:
        return None, (
            "no available nm could extract the staticlib's molt_* symbols - "
            + "; ".join(failures or ["no nm candidates found"])
        )
    try:
        _atomic_write_text(cache_path, "\n".join(sorted(symbols)) + "\n")
    except OSError as exc:
        return None, f"failed to write symbol cache {cache_path}: {exc}"
    return cache_path, None


def _runtime_intrinsic_symbols_digest(symbols_file: Path | None) -> str:
    if symbols_file is None:
        return ""
    try:
        symbols = sorted(
            {
                line.strip()
                for line in symbols_file.read_text(encoding="utf-8").splitlines()
                if line.strip()
            }
        )
    except OSError:
        return ""
    if not symbols:
        return ""
    payload = json.dumps(
        {
            "schema": "runtime-intrinsic-symbols-v1",
            "symbols": symbols,
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def _stage_runtime_intrinsic_symbols_for_native_codegen(
    runtime_state: _RuntimeArtifactState,
    *,
    target_triple: str | None,
    json_output: bool,
    runtime_cargo_profile: str,
    molt_root: Path,
    cargo_timeout: float | None,
    stdlib_profile: str | None = "micro",
    resolved_modules: set[str] | frozenset[str] | None = None,
    is_wasm_freestanding: bool = False,
) -> tuple[str, _CliFailure | None]:
    runtime_lib = runtime_state.runtime_lib
    os.environ.pop("MOLT_RUNTIME_INTRINSIC_SYMBOLS", None)
    if runtime_lib is None or is_wasm_freestanding:
        return "", None
    runtime_ready = _ensure_native_runtime_lib_ready_before_link(
        runtime_state,
        target_triple=target_triple,
        json_output=json_output,
        runtime_cargo_profile=runtime_cargo_profile,
        molt_root=molt_root,
        cargo_timeout=cargo_timeout,
        diagnostics_enabled=False,
        phase_starts={},
        stdlib_profile=stdlib_profile,
        resolved_modules=resolved_modules,
    )
    if not runtime_ready or not runtime_lib.exists():
        return "", _fail(
            "native runtime staticlib build failed or produced no artifact "
            f"({runtime_lib}); cannot stage the intrinsic-symbol set native "
            "codegen requires. See the cargo output above for the build error.",
            json_output,
            command="build",
        )
    symbols_file, symbols_failure = _runtime_intrinsic_symbols_file(runtime_lib)
    if symbols_file is None:
        return "", _fail(
            "failed to extract the runtime staticlib's molt_* intrinsic "
            f"symbols from {runtime_lib}: {symbols_failure}. Native codegen "
            "requires this set (the per-app resolver must not reference "
            "symbols the linker cannot satisfy). Remediation: install an "
            "LLVM matching your Rust toolchain (`brew install llvm` or "
            "`rustup component add llvm-tools`), or point MOLT_NM at a "
            "bitcode-capable llvm-nm.",
            json_output,
            command="build",
        )
    digest = _runtime_intrinsic_symbols_digest(symbols_file)
    if not digest:
        return "", _fail(
            "failed to digest the runtime staticlib intrinsic-symbol set "
            f"from {symbols_file}; native backend cache identity requires "
            "the exact resolver symbol authority.",
            json_output,
            command="build",
        )
    os.environ["MOLT_RUNTIME_INTRINSIC_SYMBOLS"] = str(symbols_file)
    return digest, None
