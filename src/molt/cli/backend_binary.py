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
from molt.cli.atomic_io import _atomic_copy_file, _atomic_write_json
from molt.cli.build_locks import _build_lock
from molt.cli.cache_fingerprints import _backend_source_paths
from molt.cli.cargo_execution import (
    _cargo_build_env,
    _maybe_enable_native_cpu,
    _run_cargo_with_sccache_retry,
)
from molt.cli.command_runtime import _run_subprocess_captured_to_tempfiles
from molt.cli.compiler_metadata import _compiler_clean_source_state, _rustc_version
from molt.file_hashing import _hash_source_tree_metadata, _hash_source_tree_paths
from molt.cli.native_toolchain import _codesign_binary
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
    _runtime_artifact_fingerprint_matches,
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
from molt.cli.static_archive_identity import artifact_content_identity
from molt.llvm_toolchain import LlvmToolchainConfigError, required_llvm_backend_pin
from molt.exact_json import canonical_json_sha256, read_exact
from molt.compiler_distribution import installed_compiler
from molt.python_identity_common import _valid_sha256
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    executable_content_identity,
    stable_executable_probe,
    verify_stable_regular_file_identity,
)


_BACKEND_PROBE_VALIDATION_SCHEMA_VERSION = 2
_BACKEND_PROBE_VALIDATION_MAX_BYTES = 64 * 1024
_BACKEND_COMPILER_CACHE_FINGERPRINT_SCHEMA_VERSION = 2


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
    binary_identity: Mapping[str, str | int],
) -> str:
    payload = {
        "schema": _BACKEND_COMPILER_CACHE_FINGERPRINT_SCHEMA_VERSION,
        "binary": dict(binary_identity),
        "source": {
            key: fingerprint.get(key)
            for key in ("hash", "rustc", "inputs_digest", "meta_digest")
        }
        if fingerprint is not None
        else None,
    }
    return canonical_json_sha256(payload)


def _backend_ensure_success(
    *,
    binary_path: Path,
    fingerprint: Mapping[str, Any] | None = None,
) -> _BackendBinaryEnsureResult:
    try:
        identity = executable_content_identity(binary_path, label="backend executable")
    except (OSError, ValueError) as exc:
        return _backend_ensure_failure("backend_binary_identity", str(exc))
    return _BackendBinaryEnsureResult(
        ok=True,
        cache_compiler_fingerprint=_backend_compiler_cache_fingerprint(
            fingerprint, identity
        ),
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


def _backend_probe_validation_payload(
    *,
    binary_identity: StableRegularFileIdentity,
    probe_target: str,
    backend_features: tuple[str, ...],
    fingerprint: dict[str, str | None] | None,
) -> dict[str, object] | None:
    if fingerprint is None:
        return None
    fingerprint_hash = fingerprint.get("hash")
    if not _valid_sha256(fingerprint_hash):
        return None
    return {
        "schema": _BACKEND_PROBE_VALIDATION_SCHEMA_VERSION,
        "binary": {
            "path": os.fspath(binary_identity.path),
            "size": binary_identity.size,
            "sha256": binary_identity.sha256,
        },
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
    try:
        with stable_executable_probe(binary_path, label="backend probe executable") as (
            _entrypoint,
            identity,
        ):
            expected = _backend_probe_validation_payload(
                binary_identity=identity,
                probe_target=probe_target,
                backend_features=backend_features,
                fingerprint=fingerprint,
            )
            if expected is None:
                return False
            stored = read_exact(
                path,
                max_bytes=_BACKEND_PROBE_VALIDATION_MAX_BYTES,
                label="backend probe validation",
            )
            return (
                isinstance(stored, dict)
                and type(stored.get("schema")) is int
                and isinstance(stored.get("binary"), dict)
                and type(stored["binary"].get("size")) is int
                and stored == expected
            )
    except (OSError, ValueError):
        return False


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
    # Installed compilers are immutable release inputs, never Cargo outputs.
    # Admit before every developer skip/hydration/rebuild path.
    try:
        installed = installed_compiler(project_root)
        if installed is not None:
            if backend_bin != installed.binary:
                raise ValueError(
                    "Selected compiler differs from the installed compiler"
                )
            identity = installed.verify_binary(backend_features, cargo_profile)
            return _BackendBinaryEnsureResult(
                ok=True,
                cache_compiler_fingerprint=_backend_compiler_cache_fingerprint(
                    {"hash": installed.source_sha}, identity
                ),
            )
    except (OSError, ValueError) as exc:
        return _backend_ensure_failure("installed_compiler", str(exc))
    # MOLT_SKIP_RUNTIME_REBUILD=1 also skips the backend fingerprint check.
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
        if backend_bin.exists():
            return _backend_ensure_success(binary_path=backend_bin)
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
    # All feature lanes publish the same canonical Cargo output before copying
    # their aliases; that shared publication, not the alias, owns the lock.
    lock_name = f"backend.{cargo_profile}"
    with _build_lock(project_root, lock_name):
        rebuilt_source_identity: StableRegularFileIdentity | None = None
        rebuilt_alias_identity: StableRegularFileIdentity | None = None

        def _canonical_cargo_backend_output() -> Path:
            exe_suffix = ".exe" if os.name == "nt" else ""
            return backend_bin.parent / f"molt-backend{exe_suffix}"

        def _materialize_backend_binary_from(
            source: Path,
        ) -> tuple[StableRegularFileIdentity, StableRegularFileIdentity] | None:
            if not source.exists():
                return None
            if source == backend_bin:
                _codesign_binary(backend_bin)
            with stable_executable_probe(source, label="backend alias source") as (
                _entrypoint,
                identity,
            ):
                if source != backend_bin:
                    _atomic_copy_file(source, backend_bin, codesign=True)
                with stable_executable_probe(
                    backend_bin, label="materialized backend alias"
                ) as (_alias_entrypoint, alias_identity):
                    pass
            return identity, alias_identity

        def _publish_backend_artifact(
            artifact: Path, identity: StableRegularFileIdentity
        ) -> None:
            assert fingerprint is not None
            verify_stable_regular_file_identity(identity, label="backend publication")
            content_identity = artifact_content_identity(artifact)
            verify_stable_regular_file_identity(identity, label="backend publication")
            _write_runtime_fingerprint(
                _backend_fingerprint_path(project_root, artifact, cargo_profile),
                {**fingerprint, "artifact_content_identity": content_identity},
            )
            verify_stable_regular_file_identity(identity, label="backend publication")

        def _materialize_rebuilt_backend_binary() -> _BackendBinaryEnsureResult:
            nonlocal rebuilt_source_identity, rebuilt_alias_identity
            source = _canonical_cargo_backend_output()
            try:
                materialized = _materialize_backend_binary_from(source)
            except (OSError, ValueError) as exc:
                return _backend_ensure_failure("backend_artifact", str(exc))
            if materialized is None:
                return _backend_ensure_failure(
                    "backend_artifact", "Backend binary missing after rebuild."
                )
            rebuilt_source_identity, rebuilt_alias_identity = materialized
            return _BackendBinaryEnsureResult(ok=True)

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
        ) -> _BackendBinaryEnsureResult:
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
                with stable_executable_probe(
                    binary_path or backend_bin, label="backend probe executable"
                ) as (_entrypoint, identity):
                    payload = _backend_probe_validation_payload(
                        binary_identity=identity,
                        probe_target=probe_target,
                        backend_features=backend_features,
                        fingerprint=fingerprint,
                    )
                    probe = _run_subprocess_captured_to_tempfiles(
                        probe_cmd,
                        input=probe_ir,
                        cwd=project_root,
                        timeout=10,
                        memory_guard_prefix="MOLT_BUILD",
                    )
            except (subprocess.TimeoutExpired, OSError, ValueError) as exc:
                _record_backend_binary_stage_ms(
                    stage_timings_ms,
                    "backend_binary_probe",
                    stage_start,
                )
                return _backend_ensure_failure("backend_feature_probe", str(exc))
            finally:
                try:
                    probe_path.unlink()
                except OSError:
                    pass
            stderr = probe.stderr.decode(errors="replace")
            stdout = probe.stdout.decode(errors="replace")
            if probe.returncode == 0 and binary_path is None and payload is not None:
                try:
                    _atomic_write_json(probe_validation_path, payload, indent=2)
                except (OSError, ValueError) as exc:
                    _record_backend_binary_stage_ms(
                        stage_timings_ms,
                        "backend_binary_probe",
                        stage_start,
                    )
                    return _backend_ensure_failure(
                        "backend_probe_publication",
                        f"Backend probe validation publication failed: {exc}",
                    )
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_probe",
                stage_start,
            )
            return _BackendBinaryEnsureResult(
                ok=probe.returncode == 0,
                phase="backend_feature_probe",
                detail=(stderr or stdout).strip(),
                returncode=probe.returncode,
                command=tuple(probe_cmd),
            )

        def _refresh_feature_tagged_backend_alias(
            probe_target: str,
        ) -> _BackendBinaryEnsureResult | None:
            cargo_output = _canonical_cargo_backend_output()
            if cargo_output == backend_bin or not cargo_output.exists():
                return None
            candidate_fingerprint_path = _backend_fingerprint_path(
                project_root, cargo_output, cargo_profile
            )
            try:
                with stable_executable_probe(
                    cargo_output, label="admitted Cargo backend output"
                ) as (_entrypoint, cargo_identity):
                    if not _runtime_artifact_fingerprint_matches(
                        cargo_output,
                        fingerprint,
                        candidate_fingerprint_path,
                        require_artifact_digest=True,
                    ):
                        return None
                    if backend_bin.exists():
                        alias_identity = executable_content_identity(
                            backend_bin, label="backend feature alias"
                        )
                        if (
                            alias_identity["sha256"] == cargo_identity.sha256
                            and alias_identity["size"] == cargo_identity.size
                            and _runtime_artifact_fingerprint_matches(
                                backend_bin,
                                fingerprint,
                                fingerprint_path,
                                require_artifact_digest=True,
                            )
                        ):
                            return None
                    probe_result = _probe_backend_binary_support(
                        probe_target, binary_path=cargo_output
                    )
                    if not probe_result:
                        return probe_result
                    materialized = _materialize_backend_binary_from(cargo_output)
                    if materialized is None:
                        return _backend_ensure_failure(
                            "backend_artifact", "Backend alias materialization failed."
                        )
                    _publish_backend_artifact(backend_bin, materialized[1])
            except (OSError, ValueError) as exc:
                return _backend_ensure_failure("backend_alias_publication", str(exc))
            return None

        if stored_fingerprint is None:
            stage_start = time.perf_counter()
            stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_read_fingerprint",
                stage_start,
            )
        _quick_target = _backend_probe_target()
        alias_failure = _refresh_feature_tagged_backend_alias(_quick_target)
        if alias_failure is not None:
            return alias_failure
        stage_start = time.perf_counter()
        if _runtime_artifact_fingerprint_matches(
            backend_bin, fingerprint, fingerprint_path, require_artifact_digest=True
        ):
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
                return _backend_ensure_success(
                    binary_path=backend_bin, fingerprint=fingerprint
                )
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_probe_validation",
                stage_start,
            )
            _probe_result = _probe_backend_binary_support(_quick_target)
            if _probe_result:
                return _backend_ensure_success(
                    binary_path=backend_bin, fingerprint=fingerprint
                )
            if _probe_result.phase == "backend_probe_publication":
                return _probe_result
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
            require_artifact_digest=True,
        ):
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_canonical_hydrate",
                stage_start,
            )
            _probe_target = _backend_probe_target()
            _probe_result = _probe_backend_binary_support(_probe_target)
            if _probe_result:
                return _backend_ensure_success(
                    binary_path=backend_bin, fingerprint=fingerprint
                )
            if _probe_result.phase == "backend_probe_publication":
                return _probe_result
        else:
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_canonical_hydrate",
                stage_start,
            )
        # Raw Cargo outputs and source-only sidecars do not establish provenance.
        # Confirm the source/feature build before publishing content-bound receipts.
        if not json_output:
            print(
                "Backend artifact lacks a matching source/content receipt; "
                "running Cargo to establish build provenance...",
                file=sys.stderr,
            )
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
        _materialization = _materialize_rebuilt_backend_binary()
        if not _materialization:
            return _materialization
        # -- Post-build feature probe (defense-in-depth) -----------------
        # Cargo's incremental cache may skip recompilation when only
        # features change, leaving a binary built for the wrong target.
        # Probe the binary and, on mismatch, clean + rebuild once.
        _probe_target = _backend_probe_target()
        _probe_result = _probe_backend_binary_support(_probe_target)
        if _probe_result.phase == "backend_probe_publication" and not _probe_result:
            return _probe_result
        if not _probe_result:
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
            _materialization = _materialize_rebuilt_backend_binary()
            if not _materialization:
                return _materialization
            _reprobe_result = _probe_backend_binary_support(_probe_target)
            if (
                _reprobe_result.phase == "backend_probe_publication"
                and not _reprobe_result
            ):
                return _reprobe_result
            if not _reprobe_result:
                detail = "Backend feature probe failed after rebuild."
                if _reprobe_result.detail:
                    detail = f"{detail}\n{_reprobe_result.detail}"
                return _backend_ensure_failure(
                    "backend_feature_probe",
                    detail,
                    command=cmd,
                )
        # -- End post-build feature probe --------------------------------
        if fingerprint is not None:
            try:
                cargo_output = _canonical_cargo_backend_output()
                assert rebuilt_source_identity is not None
                assert rebuilt_alias_identity is not None
                _publish_backend_artifact(cargo_output, rebuilt_source_identity)
                if cargo_output != backend_bin:
                    _publish_backend_artifact(backend_bin, rebuilt_alias_identity)
            except (OSError, ValueError) as exc:
                return _backend_ensure_failure(
                    "backend_artifact_publication",
                    f"Backend artifact provenance publication failed: {exc}",
                    command=cmd,
                )
    return _backend_ensure_success(binary_path=backend_bin, fingerprint=fingerprint)
