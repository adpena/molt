from __future__ import annotations

import hashlib
import json
import os
import time
from pathlib import Path

from molt._wasm_abi_generated import WASM_NON_RUNTIME_CALLABLE_INTRINSICS
from molt.cli.atomic_io import _atomic_write_text
from molt.cli import native_symbol_inspection
from molt.cli.config_resolution import DEFAULT_RUNTIME_STDLIB_PROFILE
from molt.cli.models import _RuntimeArtifactState
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail
from molt.cli.runtime_native_build import _ensure_native_runtime_lib_ready_before_link


def _record_runtime_callable_stage_ms(
    stage_timings_ms: dict[str, float] | None,
    name: str,
    started_at: float,
) -> None:
    if stage_timings_ms is None:
        return
    stage_timings_ms[name] = round(
        max(0.0, (time.perf_counter() - started_at) * 1000.0),
        6,
    )


def _runtime_callable_symbols_file(
    runtime_lib: Path,
    *,
    target_triple: str | None = None,
) -> tuple[Path | None, str | None]:
    """Project runtime callables from the shared, generation-bound archive facts.

    The reader owns candidate selection, target decoration, typed failures and
    artifact/reader identity. This stage owns only the callable projection and
    its materialized input to native codegen.
    """
    try:
        identity = native_symbol_inspection._native_symbol_artifact_identity(
            runtime_lib
        )
        facts = native_symbol_inspection._native_archive_global_symbol_facts(
            runtime_lib,
            target_triple=target_triple,
            identity=identity,
            requirement=native_symbol_inspection.NativeSymbolRequirement(
                function_prefix="molt_",
                excluded_functions=WASM_NON_RUNTIME_CALLABLE_INTRINSICS,
            ),
        )
        symbols = sorted(
            name
            for name in facts.defined_functions
            if name.startswith("molt_")
            and name not in WASM_NON_RUNTIME_CALLABLE_INTRINSICS
        )
        if not symbols:
            return None, "runtime staticlib defines no molt_* callable symbols"
        content = "\n".join(symbols) + "\n"
        projection_digest = hashlib.sha256(content.encode("utf-8")).hexdigest()
        cache_path = runtime_lib.with_name(
            f"{runtime_lib.name}.callable_symbols.v3."
            f"{identity.sha256}.{projection_digest}.txt"
        )
        try:
            cached = cache_path.read_text(encoding="utf-8")
        except (OSError, UnicodeError):
            cached = None
        if cached != content:
            _atomic_write_text(cache_path, content)
        native_symbol_inspection._require_unchanged_symbol_artifact(
            runtime_lib, identity
        )
        return cache_path, None
    except OSError as exc:
        return None, f"runtime staticlib callable inspection failed: {exc}"


def _runtime_callable_symbols_digest(symbols_file: Path | None) -> str:
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
            "schema": "runtime-callable-symbols-v2",
            "non_runtime_callable_intrinsics": sorted(
                WASM_NON_RUNTIME_CALLABLE_INTRINSICS
            ),
            "symbols": symbols,
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def _stage_runtime_callable_symbols_for_native_codegen(
    runtime_state: _RuntimeArtifactState,
    *,
    target_triple: str | None,
    json_output: bool,
    runtime_cargo_profile: str,
    molt_root: Path,
    cargo_timeout: float | None,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    resolved_modules: set[str] | frozenset[str] | None = None,
    is_wasm_freestanding: bool = False,
    stage_timings_ms: dict[str, float] | None = None,
) -> tuple[str, _CliFailure | None]:
    runtime_lib = runtime_state.runtime_lib
    os.environ.pop("MOLT_RUNTIME_CALLABLE_SYMBOLS", None)
    if runtime_lib is None or is_wasm_freestanding:
        return "", None
    ensure_start = time.perf_counter()
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
        stage_timings_ms=stage_timings_ms,
    )
    _record_runtime_callable_stage_ms(
        stage_timings_ms,
        "runtime_callable_symbols_ensure_runtime_lib",
        ensure_start,
    )
    if not runtime_ready or not runtime_lib.exists():
        failure = runtime_state.native_runtime_build_failure
        failure_detail = ""
        failure_data: dict[str, object] | None = None
        if failure is not None:
            failure_detail = (
                f" stage={failure.stage}; first_error={failure.summary!r};"
                + (
                    f" evidence={failure.evidence_path};"
                    if failure.evidence_path is not None
                    else ""
                )
            )
            failure_data = {"runtime_build_failure": failure.json_payload()}
        return "", _fail(
            "native runtime staticlib build failed"
            f"{failure_detail} expected_artifact={runtime_lib}; cannot stage the "
            "callable-symbol set native codegen requires.",
            json_output,
            command="build",
            data=failure_data,
        )
    symbol_file_start = time.perf_counter()
    symbols_file, symbols_failure = _runtime_callable_symbols_file(
        runtime_lib, target_triple=target_triple
    )
    _record_runtime_callable_stage_ms(
        stage_timings_ms,
        "runtime_callable_symbols_file",
        symbol_file_start,
    )
    if symbols_file is None:
        return "", _fail(
            "failed to extract the runtime staticlib's molt_* callable "
            f"symbols from {runtime_lib}: {symbols_failure}. Native codegen "
            "requires this set (the per-app resolver must not reference "
            "symbols the linker cannot satisfy). Remediation: install an "
            "LLVM matching your Rust toolchain (`brew install llvm` or "
            "`rustup component add llvm-tools`) or repair the managed "
            "MOLT_TARGET_ROOT toolchain family so its llvm-nm can read "
            "the selected Rust bitcode.",
            json_output,
            command="build",
        )
    digest_start = time.perf_counter()
    digest = _runtime_callable_symbols_digest(symbols_file)
    _record_runtime_callable_stage_ms(
        stage_timings_ms,
        "runtime_callable_symbols_digest",
        digest_start,
    )
    if not digest:
        return "", _fail(
            "failed to digest the runtime staticlib callable-symbol set "
            f"from {symbols_file}; native backend cache identity requires "
            "the exact resolver symbol authority.",
            json_output,
            command="build",
        )
    os.environ["MOLT_RUNTIME_CALLABLE_SYMBOLS"] = str(symbols_file)
    return digest, None
