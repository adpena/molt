"""Hidden ``internal-backend-build``: prewarm the backend compiler builds admit.

A bare ``cargo build`` of ``molt-backend`` neither selects the binary ``molt
build`` dispatches (host compiler profile, feature lane, session target root)
nor publishes the source/content receipts backend admission requires, so the
next build still runs Cargo. This command selects exactly like ``molt build``,
runs the same backend admission, and fails closed unless the receipts that
admission leaves behind bind exactly the compiler it admitted: the source
identity it computed and the selected binary's bytes.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path
from typing import Any

from molt.cli import backend_binary as _backend_binary
from molt.cli import backend_compile as _backend_compile
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.cli.build_inputs import (
    _apply_native_arch_perf_policy,
    _resolve_backend_compiler_profile,
)
from molt.cli.command_runtime import _resolve_timeout_env
from molt.cli.output import emit_json, fail, json_payload
from molt.cli.project_roots import (
    _find_molt_root,
    _find_project_root,
    _require_molt_root,
)
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
    _runtime_artifact_fingerprint_matches,
)
from molt.compiler_distribution import installed_compiler
from molt.exact_json import read_exact
from molt.toolchain_identity import executable_content_identity

_COMMAND = "internal-backend-build"


def _fail_prebuild(
    message: str, json_output: bool, *, data: dict[str, Any] | None = None
) -> int:
    # JSON mode owns stdout framing, not error suppression: CI logs and
    # operators still get the precise admission diagnostic on stderr.
    if json_output:
        print(message, file=sys.stderr)
    return fail(message, json_output, command=_COMMAND, data=data)


def _admitted_probe_target(
    probe_path: Path,
    selection: _backend_compile._BackendSelection,
    source_fingerprint: dict[str, Any],
) -> str | None:
    try:
        probe = read_exact(
            probe_path,
            max_bytes=_backend_binary._BACKEND_PROBE_VALIDATION_MAX_BYTES,
            label="backend probe validation",
        )
    except (OSError, ValueError):
        return None
    probe_target = probe.get("probe_target") if isinstance(probe, dict) else None
    if not isinstance(probe_target, str):
        return None
    # The receipt names the probe admission ran; its binary identity, feature
    # lane, and source fingerprint must all match the selected compiler.
    if not _backend_binary._backend_probe_validation_matches(
        probe_path,
        binary_path=selection.binary,
        probe_target=probe_target,
        backend_features=selection.features,
        fingerprint=source_fingerprint,
    ):
        return None
    return probe_target


def _verified_backend_receipts(
    molt_root: Path,
    selection: _backend_compile._BackendSelection,
    identity: dict[str, str | int],
    admitted_fingerprint: str | None,
) -> tuple[dict[str, Any] | None, str | None]:
    """Return the receipts binding exactly the compiler admission admitted.

    ``identity`` is the selected binary's content identity, read under the
    backend admission lock the caller holds for every read here.
    """
    source_path = _backend_binary._backend_fingerprint_path(
        molt_root, selection.binary, selection.cargo_profile
    )
    missing = (
        "Backend admission left no source/content receipt binding "
        f"{selection.binary} at {source_path}; the next build would run "
        "Cargo again."
    )
    source = _read_runtime_fingerprint(source_path)
    if source is None:
        return None, missing
    # Agreeing with itself proves nothing about this admission: the receipt
    # must name the source identity admission computed and these bytes must be
    # the ones it admitted, projected exactly as builds key their caches.
    receipt_fingerprint = _backend_binary._backend_compiler_cache_fingerprint(
        source, identity
    )
    if receipt_fingerprint != admitted_fingerprint:
        return None, (
            f"Backend receipt {source_path} and the bytes of {selection.binary} "
            "do not bind the compiler admission admitted (receipt fingerprint "
            f"{receipt_fingerprint}, admitted {admitted_fingerprint}); the next "
            "build would not reuse this compiler."
        )
    if not _runtime_artifact_fingerprint_matches(
        selection.binary, source, source_path, require_artifact_digest=True
    ):
        return None, missing
    probe_path = _backend_binary._backend_probe_validation_path(
        molt_root, selection.binary, selection.cargo_profile
    )
    probe_target = _admitted_probe_target(probe_path, selection, source)
    if probe_target is None:
        return None, (
            "Backend admission left no feature-probe receipt for the current "
            f"bytes of {selection.binary} at {probe_path}."
        )
    receipts = {
        "source_content": {
            "path": os.fspath(source_path),
            "hash": source["hash"],
            "rustc": source.get("rustc"),
            "inputs_digest": source.get("inputs_digest"),
            "meta_digest": source.get("meta_digest"),
            "artifact_content_identity": source.get("artifact_content_identity"),
        },
        "feature_probe": {
            "path": os.fspath(probe_path),
            "probe_target": probe_target,
        },
    }
    return receipts, None


def _prebuild_backend_binary(
    *,
    project_root: Path,
    target: str,
    json_output: bool,
    cargo_timeout: float | None,
    verbose: bool = False,
) -> int:
    if target == "mlir":
        # `build_pipeline` routes MLIR builds around the backend pipeline.
        return _fail_prebuild(
            "Target 'mlir' builds through molt-backend-mlir and never admits "
            "the molt-backend compiler; there is no backend compiler to prewarm.",
            json_output,
        )
    # Same lane classification as `_resolve_build_output_layout`.
    is_wasm = target in {"wasm", "wasm-freestanding"}
    is_luau_transpile = target == "luau"
    is_rust_transpile = target in {"rust", "luau"}
    try:
        # A target `molt build` rejects must not prewarm some other lane.
        resolve_backend_artifact_contract(
            target=target, emit_mode="wasm" if is_wasm else "bin"
        )
    except ValueError as exc:
        return _fail_prebuild(
            f"Unsupported build target {target!r}: {exc}", json_output
        )
    if cargo_timeout is None:
        cargo_timeout, timeout_error = _resolve_timeout_env("MOLT_CARGO_TIMEOUT")
        if timeout_error:
            return _fail_prebuild(timeout_error, json_output)
    elif cargo_timeout <= 0:
        return _fail_prebuild(
            f"--cargo-timeout must be greater than zero, not {cargo_timeout}.",
            json_output,
        )
    molt_root = _find_molt_root(project_root, _find_project_root(Path.cwd()))
    root_error = _require_molt_root(molt_root, json_output, _COMMAND)
    if root_error is not None:
        return root_error
    backend_profile, backend_cargo_profile, profile_error = (
        _resolve_backend_compiler_profile()
    )
    if profile_error:
        return _fail_prebuild(profile_error, json_output)
    warnings: list[str] = []
    # RUSTFLAGS are a backend fingerprint input: apply the build's policy.
    _apply_native_arch_perf_policy(target, warnings)
    selection = _backend_compile._select_backend_binary(
        molt_root=molt_root,
        backend_cargo_profile=backend_cargo_profile,
        is_wasm=is_wasm,
        is_luau_transpile=is_luau_transpile,
        is_rust_transpile=is_rust_transpile,
    )
    compiler: dict[str, Any] = {
        "path": os.fspath(selection.binary),
        "backend_profile": backend_profile,
        "cargo_profile": selection.cargo_profile,
        "features": list(selection.features),
    }
    if verbose and not json_output:
        print(
            f"Prebuilding backend compiler {selection.binary} "
            f"({selection.cargo_profile}; {','.join(selection.features)})",
            file=sys.stderr,
        )
    stage_timings_ms: dict[str, float] = {}
    result = _backend_compile._ensure_selected_backend_binary(
        selection,
        molt_root=molt_root,
        cargo_timeout=cargo_timeout,
        json_output=json_output,
        stage_timings_ms=stage_timings_ms,
    )
    if not result:
        return _fail_prebuild(
            result.message,
            json_output,
            data={
                "compiler": compiler,
                "failure": {
                    "phase": result.phase,
                    "returncode": result.returncode,
                    "command": list(result.command),
                },
                "stage_timings_ms": stage_timings_ms,
            },
        )
    receipts: dict[str, Any] | None = None
    try:
        # Admission releases its lock before returning. Reacquire it while
        # reading bytes and receipts so another feature lane cannot interleave.
        with _backend_binary._backend_admission_lock(
            molt_root, selection.cargo_profile, cargo_timeout=cargo_timeout
        ):
            identity = executable_content_identity(
                selection.binary, label="prebuilt backend compiler"
            )
            installed_manifest = installed_compiler(molt_root)
            installed = installed_manifest is not None
            if installed_manifest is not None:
                fingerprint = _backend_binary._installed_compiler_cache_fingerprint(
                    installed_manifest,
                    selection.binary,
                    selection.features,
                    selection.cargo_profile,
                )
                if fingerprint != result.cache_compiler_fingerprint:
                    raise ValueError("Installed compiler changed after admission")
            else:
                receipts, receipt_error = _verified_backend_receipts(
                    molt_root, selection, identity, result.cache_compiler_fingerprint
                )
                if receipt_error is not None:
                    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
                        receipt_error += (
                            " MOLT_SKIP_RUNTIME_REBUILD=1 bypasses backend admission;"
                            " unset it to publish receipts."
                        )
                    raise ValueError(receipt_error)
    except (OSError, ValueError, _backend_binary._BackendAdmissionLockError) as exc:
        return _fail_prebuild(
            f"Prebuilt backend compiler identity failed: {exc}",
            json_output,
            data={
                "compiler": compiler,
                "failure": {
                    "phase": (
                        "backend_receipt_lock"
                        if isinstance(exc, _backend_binary._BackendAdmissionLockError)
                        else "backend_receipt_identity"
                    ),
                },
                "stage_timings_ms": stage_timings_ms,
            },
        )
    compiler.update(identity)
    compiler["fingerprint"] = result.cache_compiler_fingerprint
    data = {
        "target": target,
        "admission": "installed" if installed else "receipts",
        "compiler": compiler,
        "receipts": receipts,
        "stage_timings_ms": stage_timings_ms,
    }
    if json_output:
        emit_json(json_payload(_COMMAND, "ok", data=data, warnings=warnings), True)
        return 0
    for warning in warnings:
        print(warning, file=sys.stderr)
    if verbose:
        print(f"Backend compiler: {selection.binary}", file=sys.stderr)
        for label, receipt in (receipts or {}).items():
            print(f"Backend {label} receipt: {receipt['path']}", file=sys.stderr)
    return 0
