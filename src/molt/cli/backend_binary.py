from __future__ import annotations

import contextlib
from dataclasses import dataclass
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Mapping, cast

from molt.cli.artifact_state import (
    _artifact_state_path,
    _artifact_state_path_for_build_state_root,
    _canonical_build_state_root,
    _canonical_target_root,
    _maybe_hydrate_artifact_from_canonical_target,
)
from molt.cli.atomic_io import _atomic_copy_file
from molt.cli.build_locks import _build_lock
from molt.cli.cache_fingerprints import _backend_source_paths
from molt.cli.cargo_execution import (
    _cargo_build_env,
    _maybe_enable_native_cpu,
    _maybe_enable_sccache,
    _run_cargo_with_sccache_retry,
)
from molt.cli.command_runtime import _run_subprocess_captured_to_tempfiles
from molt.cli.compiler_metadata import _compiler_clean_source_state, _rustc_version
from molt.file_hashing import _hash_source_tree_metadata, _hash_source_tree_paths
from molt.cli.json_cache import _read_cached_json_object, _write_cached_json_object
from molt.cli.native_toolchain import _codesign_binary
from molt.cli.runtime_fingerprints import (
    _artifact_needs_rebuild,
    _artifact_content_looks_valid,
    _read_runtime_fingerprint,
    _refresh_runtime_fingerprint_metadata,
    _stored_fingerprint_matches_source_metadata,
    _stored_fingerprint_matches_clean_source_state,
    _runtime_fingerprint_metadata_needs_refresh,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_paths import _cargo_profile_dir, _cargo_target_root
from molt.cli.setup_readiness import (
    _llvm_backend_unavailable_message,
)
from molt.llvm_toolchain import LlvmToolchainConfigError, required_llvm_backend_pin


_BACKEND_PROBE_VALIDATION_SCHEMA_VERSION = 1
_BACKEND_COMPILER_CACHE_FINGERPRINT_SCHEMA_VERSION = 1


@dataclass(frozen=True)
class _BackendBinaryEnsureResult:
    ok: bool
    detail: str | None = None
    returncode: int | None = None
    phase: str | None = None
    command: tuple[str, ...] = ()
    cache_compiler_fingerprint: str | None = None

    def __bool__(self) -> bool:
        return self.ok

    @property
    def message(self) -> str:
        return self.detail or "Backend build failed"


def _backend_compiler_cache_fingerprint(
    fingerprint: Mapping[str, Any] | None,
) -> str | None:
    if fingerprint is None:
        return None
    fingerprint_hash = fingerprint.get("hash")
    if not isinstance(fingerprint_hash, str) or not fingerprint_hash:
        return None
    payload = {
        "schema": _BACKEND_COMPILER_CACHE_FINGERPRINT_SCHEMA_VERSION,
        "hash": fingerprint_hash,
        "rustc": fingerprint.get("rustc"),
        "inputs_digest": fingerprint.get("inputs_digest"),
        "meta_digest": fingerprint.get("meta_digest"),
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _backend_ensure_success(
    *,
    fingerprint: Mapping[str, Any] | None = None,
) -> _BackendBinaryEnsureResult:
    return _BackendBinaryEnsureResult(
        ok=True,
        cache_compiler_fingerprint=_backend_compiler_cache_fingerprint(fingerprint),
    )


def _record_backend_binary_stage_ms(
    stage_timings_ms: dict[str, float] | None,
    name: str,
    started_at: float,
) -> None:
    if stage_timings_ms is None:
        return
    elapsed_ms = max(0.0, (time.perf_counter() - started_at) * 1000.0)
    stage_timings_ms[name] = round(
        stage_timings_ms.get(name, 0.0) + elapsed_ms,
        6,
    )


def _backend_ensure_failure(
    phase: str,
    detail: str,
    *,
    returncode: int | None = None,
    command: list[str] | tuple[str, ...] = (),
) -> _BackendBinaryEnsureResult:
    return _BackendBinaryEnsureResult(
        ok=False,
        detail=detail,
        returncode=returncode,
        phase=phase,
        command=tuple(command),
    )


def _process_text_tail(value: str | bytes | None, *, limit: int = 4000) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        text = value.decode("utf-8", errors="replace")
    else:
        text = value
    text = text.strip()
    if len(text) <= limit:
        return text
    return f"... <truncated to last {limit} chars>\n{text[-limit:]}"


def _completed_process_failure_detail(
    label: str,
    process: subprocess.CompletedProcess[str] | subprocess.CompletedProcess[bytes],
) -> str:
    rc = process.returncode
    body = _process_text_tail(process.stderr) or _process_text_tail(process.stdout)
    detail = f"{label} failed (exit {rc})"
    if body:
        detail = f"{detail}:\n{body}"
    return detail


def _backend_fingerprint_path(
    project_root: Path,
    artifact: Path,
    cargo_profile: str,
) -> Path:
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="backend_fingerprints",
        stem_suffix=f"{cargo_profile}",
        extension="fingerprint",
    )


def _backend_probe_validation_path(
    project_root: Path,
    artifact: Path,
    cargo_profile: str,
) -> Path:
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="backend_probe_validations",
        stem_suffix=f"{cargo_profile}",
        extension="json",
    )


def _backend_probe_binary_identity(binary_path: Path) -> dict[str, object] | None:
    try:
        resolved = binary_path.resolve()
    except OSError:
        resolved = binary_path
    try:
        stat = binary_path.stat()
    except OSError:
        return None
    return {
        "path": os.fspath(resolved),
        "mtime_ns": stat.st_mtime_ns,
        "size": stat.st_size,
    }


def _backend_probe_validation_payload(
    *,
    binary_path: Path,
    probe_target: str,
    backend_features: tuple[str, ...],
    fingerprint: dict[str, str | None] | None,
) -> dict[str, object] | None:
    if fingerprint is None:
        return None
    fingerprint_hash = fingerprint.get("hash")
    if not isinstance(fingerprint_hash, str) or not fingerprint_hash:
        return None
    binary_identity = _backend_probe_binary_identity(binary_path)
    if binary_identity is None:
        return None
    return {
        "schema": _BACKEND_PROBE_VALIDATION_SCHEMA_VERSION,
        "binary": binary_identity,
        "probe_target": probe_target,
        "backend_features": sorted(backend_features),
        "fingerprint": {
            "hash": fingerprint_hash,
            "rustc": fingerprint.get("rustc"),
            "inputs_digest": fingerprint.get("inputs_digest"),
            "meta_digest": fingerprint.get("meta_digest"),
        },
    }


def _backend_probe_validation_matches(
    path: Path,
    *,
    binary_path: Path,
    probe_target: str,
    backend_features: tuple[str, ...],
    fingerprint: dict[str, str | None] | None,
) -> bool:
    expected = _backend_probe_validation_payload(
        binary_path=binary_path,
        probe_target=probe_target,
        backend_features=backend_features,
        fingerprint=fingerprint,
    )
    if expected is None:
        return False
    return _read_cached_json_object(path) == expected


def _write_backend_probe_validation(
    path: Path,
    *,
    binary_path: Path,
    probe_target: str,
    backend_features: tuple[str, ...],
    fingerprint: dict[str, str | None] | None,
) -> None:
    payload = _backend_probe_validation_payload(
        binary_path=binary_path,
        probe_target=probe_target,
        backend_features=backend_features,
        fingerprint=fingerprint,
    )
    if payload is None:
        return
    _write_cached_json_object(path, payload)


def _backend_fingerprint(
    project_root: Path,
    *,
    cargo_profile: str,
    rustflags: str,
    backend_features: tuple[str, ...],
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, Any] | None:
    meta = f"profile:{cargo_profile}\n"
    meta += f"rustflags:{rustflags}\n"
    meta += f"features:{','.join(backend_features)}\n"
    meta_digest = hashlib.sha256(meta.encode("utf-8")).hexdigest()
    rustc_info = _rustc_version()
    source_state = _compiler_clean_source_state(project_root)
    if _stored_fingerprint_matches_clean_source_state(
        stored_fingerprint,
        source_state=source_state,
        rustc=rustc_info,
        meta_digest=meta_digest,
    ):
        assert stored_fingerprint is not None
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": rustc_info,
            "inputs_digest": stored_fingerprint.get("inputs_digest"),
            "meta_digest": meta_digest,
            "source_state": source_state,
        }
    source_paths = _backend_source_paths(project_root, backend_features)
    inputs_meta = _hash_source_tree_metadata(source_paths, project_root)
    inputs_digest = inputs_meta[0] if inputs_meta is not None else None
    if _stored_fingerprint_matches_source_metadata(
        stored_fingerprint,
        inputs_digest=inputs_digest,
        rustc=rustc_info,
        meta_digest=meta_digest,
    ):
        assert stored_fingerprint is not None
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": rustc_info,
            "inputs_digest": inputs_digest,
            "meta_digest": meta_digest,
            "source_state": source_state,
        }

    hasher = hashlib.sha256()
    hasher.update(meta.encode("utf-8"))
    try:
        _hash_source_tree_paths(source_paths, project_root, hasher)
    except OSError:
        return None
    return {
        "hash": hasher.hexdigest(),
        "rustc": rustc_info,
        "inputs_digest": inputs_digest,
        "meta_digest": meta_digest,
        "source_state": source_state,
    }


def _ensure_backend_binary(
    backend_bin: Path,
    *,
    cargo_timeout: float | None,
    json_output: bool,
    cargo_profile: str,
    project_root: Path,
    backend_features: tuple[str, ...],
    stage_timings_ms: dict[str, float] | None = None,
) -> _BackendBinaryEnsureResult:
    # MOLT_SKIP_RUNTIME_REBUILD=1 also skips the backend fingerprint check.
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
        if backend_bin.exists():
            return _backend_ensure_success()
    rustflags = os.environ.get("RUSTFLAGS", "")
    fingerprint_path = _backend_fingerprint_path(
        project_root, backend_bin, cargo_profile
    )
    probe_validation_path = _backend_probe_validation_path(
        project_root, backend_bin, cargo_profile
    )
    stage_start = time.perf_counter()
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    _record_backend_binary_stage_ms(
        stage_timings_ms,
        "backend_binary_read_fingerprint",
        stage_start,
    )
    stage_start = time.perf_counter()
    fingerprint = _backend_fingerprint(
        project_root,
        cargo_profile=cargo_profile,
        rustflags=rustflags,
        backend_features=backend_features,
        stored_fingerprint=stored_fingerprint,
    )
    _record_backend_binary_stage_ms(
        stage_timings_ms,
        "backend_binary_compute_fingerprint",
        stage_start,
    )
    features_tag = "_".join(sorted(backend_features)) if backend_features else "default"
    lock_name = f"backend.{cargo_profile}.{features_tag}"
    with _build_lock(project_root, lock_name):

        def _canonical_cargo_backend_output() -> Path:
            exe_suffix = ".exe" if os.name == "nt" else ""
            return backend_bin.parent / f"molt-backend{exe_suffix}"

        def _materialize_backend_binary_from(source: Path) -> bool:
            if not source.exists():
                return False
            if source != backend_bin:
                _atomic_copy_file(source, backend_bin, codesign=True)
            else:
                _codesign_binary(backend_bin)
            return backend_bin.exists()

        def _materialize_rebuilt_backend_binary() -> bool:
            return _materialize_backend_binary_from(_canonical_cargo_backend_output())

        def _backend_probe_target() -> str:
            if "wasm-backend" in backend_features:
                return "wasm"
            if "luau-backend" in backend_features:
                return "luau"
            if "rust-backend" in backend_features:
                return "rust"
            return "native"

        def _probe_backend_binary_support(
            probe_target: str,
            *,
            binary_path: Path | None = None,
        ) -> tuple[bool, str]:
            stage_start = time.perf_counter()
            probe_ir = json.dumps(
                {
                    "functions": [],
                    "module": "__probe__",
                    "entry": "main",
                    "metadata": {"target": probe_target, "deterministic": True},
                }
            ).encode()
            probe_suffix = ".o"
            if probe_target == "wasm":
                probe_suffix = ".wasm"
            elif probe_target == "luau":
                probe_suffix = ".luau"
            elif probe_target == "rust":
                probe_suffix = ".rs"
            probe_tmp = tempfile.NamedTemporaryFile(
                prefix="molt_backend_probe_",
                suffix=probe_suffix,
                delete=False,
            )
            probe_path = Path(probe_tmp.name)
            probe_tmp.close()
            probe_cmd = [str(binary_path or backend_bin), "--output", str(probe_path)]
            if probe_target == "wasm":
                probe_cmd.extend(["--target", "wasm"])
            elif probe_target == "luau":
                probe_cmd.extend(["--target", "luau"])
            elif probe_target == "rust":
                probe_cmd.extend(["--target", "rust"])
            try:
                probe = _run_subprocess_captured_to_tempfiles(
                    probe_cmd,
                    input=probe_ir,
                    cwd=project_root,
                    timeout=10,
                    memory_guard_prefix="MOLT_BUILD",
                )
            except (subprocess.TimeoutExpired, OSError) as exc:
                _record_backend_binary_stage_ms(
                    stage_timings_ms,
                    "backend_binary_probe",
                    stage_start,
                )
                return False, str(exc)
            finally:
                try:
                    probe_path.unlink()
                except OSError:
                    pass
            stderr = probe.stderr.decode(errors="replace")
            stdout = probe.stdout.decode(errors="replace")
            if probe.returncode == 0 and binary_path is None:
                _write_backend_probe_validation(
                    probe_validation_path,
                    binary_path=backend_bin,
                    probe_target=probe_target,
                    backend_features=backend_features,
                    fingerprint=fingerprint,
                )
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_probe",
                stage_start,
            )
            return probe.returncode == 0, (stderr or stdout).strip()

        def _refresh_feature_tagged_backend_alias(probe_target: str) -> None:
            cargo_output = _canonical_cargo_backend_output()
            if cargo_output == backend_bin or not cargo_output.exists():
                return
            try:
                cargo_mtime = cargo_output.stat().st_mtime_ns
            except OSError:
                return
            try:
                alias_mtime = backend_bin.stat().st_mtime_ns
            except OSError:
                alias_mtime = -1
            if alias_mtime >= cargo_mtime:
                return
            probe_ok, _probe_detail = _probe_backend_binary_support(
                probe_target,
                binary_path=cargo_output,
            )
            if probe_ok:
                _materialize_backend_binary_from(cargo_output)

        if stored_fingerprint is None:
            stage_start = time.perf_counter()
            stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_read_fingerprint",
                stage_start,
            )
        stage_start = time.perf_counter()
        if not _artifact_needs_rebuild(backend_bin, fingerprint, stored_fingerprint):
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_artifact_freshness",
                stage_start,
            )
            if fingerprint is not None and _runtime_fingerprint_metadata_needs_refresh(
                stored_fingerprint, fingerprint
            ):
                with contextlib.suppress(OSError):
                    _refresh_runtime_fingerprint_metadata(
                        fingerprint_path,
                        fingerprint,
                    )
            # Force a real compile-path probe. An empty stdin-only probe can
            # miss feature-lane poisoning because it never exercises output
            # emission for the requested target.
            _quick_target = _backend_probe_target()
            _refresh_feature_tagged_backend_alias(_quick_target)
            stage_start = time.perf_counter()
            if _backend_probe_validation_matches(
                probe_validation_path,
                binary_path=backend_bin,
                probe_target=_quick_target,
                backend_features=backend_features,
                fingerprint=fingerprint,
            ):
                _record_backend_binary_stage_ms(
                    stage_timings_ms,
                    "backend_binary_probe_validation",
                    stage_start,
                )
                return _backend_ensure_success(fingerprint=fingerprint)
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_probe_validation",
                stage_start,
            )
            _probe_ok, _probe_detail = _probe_backend_binary_support(_quick_target)
            if _probe_ok:
                return _backend_ensure_success(fingerprint=fingerprint)
        else:
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_artifact_freshness",
                stage_start,
            )
        canonical_target_root = _canonical_target_root(project_root)
        canonical_backend_bin = (
            canonical_target_root / _cargo_profile_dir(cargo_profile) / backend_bin.name
        )
        canonical_fingerprint_path = _artifact_state_path_for_build_state_root(
            _canonical_build_state_root(project_root),
            canonical_backend_bin,
            subdir="backend_fingerprints",
            stem_suffix=f"{cargo_profile}",
            extension="fingerprint",
        )
        stage_start = time.perf_counter()
        if _maybe_hydrate_artifact_from_canonical_target(
            artifact=backend_bin,
            fingerprint=fingerprint,
            fingerprint_path=fingerprint_path,
            candidate_artifact=canonical_backend_bin,
            candidate_fingerprint_path=canonical_fingerprint_path,
        ):
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_canonical_hydrate",
                stage_start,
            )
            _probe_target = _backend_probe_target()
            _probe_ok, _probe_detail = _probe_backend_binary_support(_probe_target)
            if _probe_ok:
                return _backend_ensure_success(fingerprint=fingerprint)
        else:
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_canonical_hydrate",
                stage_start,
            )
        # Cargo always writes the executable as `molt-backend`; Molt keeps
        # feature-specific aliases beside it so native/wasm/rust lanes cannot
        # poison each other.  When CI or a developer prebuilds the correct
        # feature lane with cargo, materialize the alias after probing the
        # canonical binary instead of rebuilding the backend.
        if _canonical_cargo_backend_output() != backend_bin:
            cargo_output = _canonical_cargo_backend_output()
            stage_start = time.perf_counter()
            if _artifact_newer_than_sources(
                cargo_output,
                _backend_source_paths(project_root, backend_features),
            ):
                _record_backend_binary_stage_ms(
                    stage_timings_ms,
                    "backend_binary_cargo_output_newer_than_sources",
                    stage_start,
                )
                _probe_target = _backend_probe_target()
                _probe_ok, _probe_detail = _probe_backend_binary_support(
                    _probe_target,
                    binary_path=cargo_output,
                )
                if _probe_ok and _materialize_backend_binary_from(cargo_output):
                    if fingerprint is not None:
                        try:
                            fingerprint_path.parent.mkdir(
                                parents=True,
                                exist_ok=True,
                            )
                            _write_runtime_fingerprint(fingerprint_path, fingerprint)
                        except OSError:
                            if not json_output:
                                print(
                                    "Warning: failed to write backend fingerprint metadata.",
                                    file=sys.stderr,
                                )
                    return _backend_ensure_success(fingerprint=fingerprint)
            else:
                _record_backend_binary_stage_ms(
                    stage_timings_ms,
                    "backend_binary_cargo_output_newer_than_sources",
                    stage_start,
                )
        # Fast path: if the backend binary exists and is newer than every
        # source file that contributes to the fingerprint, skip the expensive
        # cargo build and just update the stored fingerprint.  This handles
        # the common case of running `cargo build` manually before `molt build`.
        stage_start = time.perf_counter()
        if _artifact_newer_than_sources(
            backend_bin,
            _backend_source_paths(project_root, backend_features),
        ):
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_newer_than_sources",
                stage_start,
            )
            _probe_target = _backend_probe_target()
            _probe_ok, _probe_detail = _probe_backend_binary_support(_probe_target)
            if _probe_ok:
                assert fingerprint is not None
                _write_runtime_fingerprint(fingerprint_path, fingerprint)
                return _backend_ensure_success(fingerprint=fingerprint)
        else:
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_newer_than_sources",
                stage_start,
            )
        if not json_output:
            print("Backend sources changed; rebuilding backend...", file=sys.stderr)
        if "llvm" in backend_features:
            llvm_message = _llvm_backend_unavailable_message(project_root)
            if llvm_message is not None:
                return _backend_ensure_failure("backend_toolchain", llvm_message)
        # Cache entries include backend/tooling/runtime identity in their keys.
        # A backend rebuild therefore invalidates by selecting new keys, not by
        # deleting shared immutable cache artifacts that concurrent sessions may
        # still be reading. Size/age retention belongs to `molt clean`.
        cmd = [
            "cargo",
            "build",
            "--package",
            "molt-backend",
            "--bin",
            "molt-backend",
            "--profile",
            cargo_profile,
        ]
        if backend_features:
            cmd.append("--no-default-features")
            cmd.extend(["--features", ",".join(backend_features)])
        build_env = _cargo_build_env()
        # Per-session build isolation: route cargo output to
        # target/sessions/<id>/ under the canonical target root
        # when MOLT_SESSION_ID is active to prevent concurrent agents from
        # clobbering each other's backend artifacts.
        build_env["CARGO_TARGET_DIR"] = str(_cargo_target_root(project_root))
        # When building with the LLVM feature, ensure the pinned llvm-sys
        # prefix env var points at the matching Homebrew install so
        # inkwell/llvm-sys can link without extra shell setup.
        if "llvm" in backend_features:
            try:
                llvm_pin = required_llvm_backend_pin(project_root)
            except LlvmToolchainConfigError:
                llvm_pin = None
            if llvm_pin is not None and llvm_pin.env_var not in build_env:
                llvm_prefix = f"/opt/homebrew/opt/llvm@{llvm_pin.major}"
                if os.path.isdir(llvm_prefix):
                    build_env[llvm_pin.env_var] = llvm_prefix
        _maybe_enable_sccache(build_env)
        _maybe_enable_native_cpu(build_env)
        try:
            stage_start = time.perf_counter()
            build = _run_cargo_with_sccache_retry(
                cmd,
                cwd=project_root,
                env=build_env,
                timeout=cargo_timeout,
                json_output=json_output,
                label="Backend build",
            )
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_cargo_build",
                stage_start,
            )
        except subprocess.TimeoutExpired:
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_cargo_build",
                stage_start,
            )
            timeout_note = (
                f"Backend build timed out after {cargo_timeout:.1f}s."
                if cargo_timeout is not None
                else "Backend build timed out."
            )
            return _backend_ensure_failure(
                "backend_cargo_build",
                timeout_note,
                command=cmd,
            )
        if build.returncode != 0:
            return _backend_ensure_failure(
                "backend_cargo_build",
                _completed_process_failure_detail("Backend cargo build", build),
                returncode=build.returncode,
                command=cmd,
            )
        # Cargo always produces target/<profile>/molt-backend regardless of
        # features.  When the requested feature set is non-default, copy
        # the freshly-built binary to the feature-tagged path so that
        # concurrent or sequential builds with different feature sets
        # (native vs wasm vs rust) do not overwrite each other.
        if not _materialize_rebuilt_backend_binary():
            return _backend_ensure_failure(
                "backend_artifact",
                "Backend binary missing after rebuild.",
                command=cmd,
            )
        # -- Post-build feature probe (defense-in-depth) -----------------
        # Cargo's incremental cache may skip recompilation when only
        # features change, leaving a binary built for the wrong target.
        # Probe the binary and, on mismatch, clean + rebuild once.
        _probe_target = _backend_probe_target()
        _probe_ok, _probe_detail = _probe_backend_binary_support(_probe_target)
        if not _probe_ok:
            if not json_output:
                print(
                    "Backend feature mismatch detected; cleaning and rebuilding...",
                    file=sys.stderr,
                )
            # Skip cargo clean: the deterministic rebuild path plus post-build
            # feature probe is the authority, while cargo clean would hold the
            # Cargo lock and block concurrent sessions.
            try:
                rebuild = _run_cargo_with_sccache_retry(
                    cmd,
                    cwd=project_root,
                    env=build_env,
                    timeout=cargo_timeout,
                    json_output=json_output,
                    label="Backend rebuild (feature fix)",
                )
            except subprocess.TimeoutExpired:
                return _backend_ensure_failure(
                    "backend_feature_rebuild",
                    "Backend rebuild timed out.",
                    command=cmd,
                )
            if rebuild.returncode != 0:
                return _backend_ensure_failure(
                    "backend_feature_rebuild",
                    _completed_process_failure_detail(
                        "Backend feature rebuild", rebuild
                    ),
                    returncode=rebuild.returncode,
                    command=cmd,
                )
            if not _materialize_rebuilt_backend_binary():
                return _backend_ensure_failure(
                    "backend_artifact",
                    "Backend binary missing after rebuild.",
                    command=cmd,
                )
            _reprobe_ok, _reprobe_detail = _probe_backend_binary_support(_probe_target)
            if not _reprobe_ok:
                detail = "Backend feature probe failed after rebuild."
                if _reprobe_detail:
                    detail = f"{detail}\n{_reprobe_detail}"
                return _backend_ensure_failure(
                    "backend_feature_probe",
                    detail,
                    command=cmd,
                )
        # -- End post-build feature probe --------------------------------
        if fingerprint is not None:
            try:
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(fingerprint_path, fingerprint)
            except OSError:
                if not json_output:
                    print(
                        "Warning: failed to write backend fingerprint metadata.",
                        file=sys.stderr,
                    )
    return _backend_ensure_success(fingerprint=fingerprint)


def _artifact_newer_than_sources(
    artifact: Path,
    source_paths: list[Path],
) -> bool:
    """Return True if *artifact* exists and is newer than every file in *source_paths*.

    Handles both individual files and directories (recursed for all files).
    Returns False on any OS error or if no source files are found.
    """
    try:
        lib_mtime = artifact.stat().st_mtime
    except OSError:
        return False
    if not _artifact_content_looks_valid(artifact):
        return False
    newest_src = 0.0
    for path in source_paths:
        try:
            if path.is_dir():
                for item in path.rglob("*"):
                    if item.is_file():
                        newest_src = max(newest_src, item.stat().st_mtime)
            elif path.exists():
                newest_src = max(newest_src, path.stat().st_mtime)
        except OSError:
            return False
    return newest_src > 0 and lib_mtime > newest_src
