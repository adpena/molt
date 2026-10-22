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

from molt.compilation_target import target_name

from molt.compilation_target import Target, TargetKind, is_wasm_target

import os
import sys
from pathlib import Path
from typing import Any

from molt.backend_executable_names import CodegenBackend
from molt.cli import backend_binary as _backend_binary
from molt.cli import backend_compile as _backend_compile
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.cli.build_inputs import _resolve_backend_compiler_profile
from molt.cli.command_runtime import _resolve_timeout_env
from molt.cli.output import emit_json, fail, json_payload
from molt.cli.project_roots import (
    _require_molt_root,
)
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
)
from molt.cli.compiler_identity import installed_compiler_admission
from molt.cli.rust_cpu_policy import resolve_rust_cpu_policy
from molt.exact_json import read_exact
from molt.source_root import compiler_source_root
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
    binary: Path,
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
        binary_path=binary,
        probe_target=probe_target,
        backend_features=selection.features,
        fingerprint=source_fingerprint,
    ):
        return None
    return probe_target


def _verified_backend_receipts(
    molt_root: Path,
    selection: _backend_compile._BackendSelection,
    binary: Path,
    identity: dict[str, str | int],
    admitted_fingerprint: str | None,
) -> tuple[dict[str, Any] | None, str | None]:
    """Return the store receipts binding exactly the compiler admission admitted.

    ``binary`` is the stored generation admission admitted and ``identity`` its
    content identity. The receipts belong to the product store variant that
    selects it under this checkout's identity and observed environment.
    """
    build_admission = _backend_binary.backend_build_admission(
        molt_root,
        selection.features,
        selection.cargo_profile,
        os.environ,
        rust_cpu=selection.rust_cpu,
    )
    build_admission.verify()
    current = _backend_binary._current_backend_fingerprint(
        molt_root,
        cargo_profile=selection.cargo_profile,
        backend_features=selection.features,
        build_admission=build_admission,
    )
    store = _backend_binary.host_tool_store(molt_root)
    key = _backend_binary.host_tool_identity(selection.binary, current)
    # Renewed before the read, so no eviction races it (design 56, 2.8).
    store.record_use(key)
    admission = next(
        (
            stored
            for stored in _backend_binary.stored_host_tools(
                store,
                key,
                selection.binary,
                current,
                environment=build_admission.plan.environment,
            )
            if stored.generation.resolve() == binary.resolve()
        ),
        None,
    )
    if admission is None:
        return None, (
            f"Backend admission left no store receipt selecting {binary} in "
            f"{store.entry(key)}; the next build would run Cargo again."
        )
    source_path = admission.stored.receipt
    source = _read_runtime_fingerprint(source_path)
    if source is None:
        return None, f"Backend store receipt {source_path} does not verify."
    # Agreeing with itself proves nothing about this admission: the receipt
    # must name the source identity admission computed and these bytes must be
    # the ones it admitted, projected exactly as builds key their caches.
    receipt_fingerprint = _backend_binary._backend_compiler_cache_fingerprint(
        source, identity
    )
    if receipt_fingerprint != admitted_fingerprint:
        return None, (
            f"Backend receipt {source_path} and the bytes of {binary} "
            "do not bind the compiler admission admitted (receipt fingerprint "
            f"{receipt_fingerprint}, admitted {admitted_fingerprint}); the next "
            "build would not reuse this compiler."
        )
    probe_path = admission.stored.probe_validation
    probe_target = _admitted_probe_target(probe_path, selection, binary, source)
    if probe_target is None:
        return None, (
            "Backend admission left no feature-probe receipt for the current "
            f"bytes of {binary} at {probe_path}."
        )
    receipts = {
        "source_content": {
            "path": os.fspath(source_path),
            "hash": source["hash"],
            "rustc": source.get("rustc"),
            "meta_digest": source.get("meta_digest"),
            "artifact_content_identity": source.get("artifact_content_identity"),
            "cargo_environment": source.get("cargo_environment"),
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
    target: Target,
    codegen_backend: CodegenBackend,
    json_output: bool,
    cargo_timeout: float | None,
    verbose: bool = False,
) -> int:
    if target is TargetKind.MLIR:
        # `build_pipeline` routes MLIR builds around the backend pipeline.
        return _fail_prebuild(
            "Target 'mlir' builds through molt-backend-mlir and never admits "
            "the molt-backend compiler; there is no backend compiler to prewarm.",
            json_output,
        )
    # Same lane classification as `_resolve_build_output_layout`.
    is_wasm = is_wasm_target(target)
    is_luau_transpile = target is TargetKind.LUAU
    is_rust_transpile = target in {TargetKind.RUST, TargetKind.LUAU}
    try:
        # A target `molt build` rejects must not prewarm some other lane.
        resolve_backend_artifact_contract(
            target=target, emit_mode="wasm" if is_wasm else "bin"
        )
    except ValueError as exc:
        return _fail_prebuild(
            f"Unsupported build target {target_name(target)!r}: {exc}", json_output
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
    # Match public build: compiler inputs are independent of the user project.
    molt_root = compiler_source_root()
    root_error = _require_molt_root(molt_root, json_output, _COMMAND)
    if root_error is not None:
        return root_error
    backend_profile, backend_cargo_profile, profile_error = (
        _resolve_backend_compiler_profile()
    )
    if profile_error:
        return _fail_prebuild(profile_error, json_output)
    warnings: list[str] = []
    # The Rust CPU policy is a compiler identity input: resolve it from the
    # same request that `molt build` resolves it from.
    try:
        rust_cpu, rust_cpu_warning = resolve_rust_cpu_policy(target, os.environ)
    except ValueError as exc:
        return _fail_prebuild(str(exc), json_output)
    if rust_cpu_warning is not None:
        warnings.append(rust_cpu_warning)
    selection = _backend_compile._select_backend_binary(
        molt_root=molt_root,
        backend_cargo_profile=backend_cargo_profile,
        is_wasm=is_wasm,
        is_luau_transpile=is_luau_transpile,
        is_rust_transpile=is_rust_transpile,
        codegen_backend=codegen_backend,
        rust_cpu=rust_cpu,
    )
    compiler: dict[str, Any] = {
        "coordinate": os.fspath(selection.binary),
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

    binary = result.binary
    assert binary is not None
    compiler["path"] = os.fspath(binary)

    def admitted_identity() -> tuple[dict[str, str | int], bool, dict[str, Any] | None]:
        identity = executable_content_identity(
            binary, label="prebuilt backend compiler"
        )
        installed_manifest = installed_compiler_admission(
            molt_root, selection.features, selection.cargo_profile, fresh=True
        )
        if installed_manifest is not None:
            if installed_manifest.compiler.binary != binary:
                raise ValueError(
                    "Selected compiler differs from the installed compiler"
                )
            if installed_manifest.fingerprint != result.cache_compiler_fingerprint:
                raise ValueError("Installed compiler changed after admission")
            return identity, True, None
        receipts, receipt_error = _verified_backend_receipts(
            molt_root, selection, binary, identity, result.cache_compiler_fingerprint
        )
        if receipt_error is not None:
            if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
                receipt_error += (
                    " MOLT_SKIP_RUNTIME_REBUILD=1 bypasses backend admission;"
                    " unset it to publish receipts."
                )
            raise ValueError(receipt_error)
        return identity, False, receipts

    try:
        try:
            # The admitted generation is immutable and the receipts bind its
            # bytes, so a read without the lock can only fail while another
            # lane rewrites a receipt, never pass for other bytes. Only a
            # failed read waits for the lock and reads again.
            identity, installed, receipts = admitted_identity()
        except (OSError, ValueError):
            with _backend_binary._backend_admission_lock(
                molt_root,
                selection.binary,
                selection.cargo_profile,
            ):
                identity, installed, receipts = admitted_identity()
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
        "target": target_name(target),
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
        print(f"Backend compiler: {binary}", file=sys.stderr)
        for label, receipt in (receipts or {}).items():
            print(f"Backend {label} receipt: {receipt['path']}", file=sys.stderr)
    return 0
