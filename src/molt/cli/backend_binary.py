from __future__ import annotations

from contextlib import contextmanager, ExitStack
from functools import wraps
import hashlib
import json
import os
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, cast

from molt.backend_executable_names import backend_executable_name
from molt.cargo_execution_policy import source_build_disabled_reason
from molt.cli import progress as _progress
from molt.cli.artifact_state import (
    _artifact_state_path,
    _artifact_state_path_for_build_state_root,
    _canonical_build_state_root,
    _canonical_target_root,
    _maybe_hydrate_artifact_from_canonical_target,
)
from molt.cli.atomic_io import _atomic_copy_file, _atomic_write_json
from molt.cli.build_locks import _build_lock
from molt.cli.cache_fingerprints import (
    _backend_source_identity_inputs,
    _backend_source_paths,
)
from molt.cli.cargo_execution import (
    CargoPlanExecutionError,
    _run_resolved_cargo_plan,
)
from molt.cli.command_runtime import _run_subprocess_captured_to_tempfiles
from molt.cli.compiler_metadata import _compiler_clean_source_state
from molt.cli.compiler_identity import (
    BackendBuildAdmission,
    CompilerIdentityError,
    CompilerSourceGeneration,
    backend_build_admission,
    installed_compiler_admission,
)
from molt.cli.runtime_fingerprints import (
    _artifact_semantic_identity,
    _admitted_runtime_fingerprint,
    _read_runtime_fingerprint,
    _refresh_runtime_fingerprint_metadata,
    _runtime_artifact_fingerprint_matches,
    _runtime_fingerprint_metadata_needs_refresh,
    _stored_fingerprint_matches_clean_source_state,
    _stored_fingerprint_matches_source_metadata,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_paths import _cargo_profile_dir
from molt.cli.static_archive_identity import artifact_content_identity
from molt.exact_json import canonical_json_sha256, read_exact
from molt.file_hashing import _hash_source_tree_metadata, _hash_source_tree_paths
from molt.python_identity_common import _valid_sha256
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    executable_content_identity,
    stable_executable_probe,
    verify_stable_regular_file_identity,
)


_BACKEND_PROBE_VALIDATION_SCHEMA_VERSION = 3
_BACKEND_PROBE_VALIDATION_MAX_BYTES = 64 * 1024
_BACKEND_COMPILER_CACHE_FINGERPRINT_SCHEMA_VERSION = 3


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
        # Receipt metadata (source timestamps, clean-head state) never renames
        # a compiler: outputs are keyed only by the identity admission compares.
        "source": _artifact_semantic_identity(fingerprint)
        if fingerprint is not None
        else None,
    }
    return canonical_json_sha256(payload)


class _BackendAdmissionLockError(RuntimeError):
    """Acquisition failure, distinct from failures inside the locked operation."""


@contextmanager
def _backend_admission_lock(
    project_root: Path, cargo_profile: str, *, cargo_timeout: float | None = None
):
    # All feature lanes share Cargo's canonical output and its publication lock.
    # A default waiter must tolerate one bounded cold build; operator overrides
    # retain authority in _build_lock.
    with ExitStack() as stack:
        try:
            stack.enter_context(
                _build_lock(
                    project_root,
                    f"backend.{cargo_profile}",
                    default_timeout_s=cargo_timeout
                    if cargo_timeout is not None
                    else 300.0,
                )
            )
        except (RuntimeError, OSError) as exc:
            raise _BackendAdmissionLockError(str(exc)) from exc
        yield


def _structured_backend_lock_failure(operation):
    @wraps(operation)
    def admitted(*args, **kwargs):
        try:
            return operation(*args, **kwargs)
        except _BackendAdmissionLockError as exc:
            return _backend_ensure_failure("backend_build_lock", str(exc))

    return admitted


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
        "fingerprint": _artifact_semantic_identity(fingerprint),
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
    build_admission: BackendBuildAdmission,
    backend_features: tuple[str, ...],
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, Any]:
    source_paths, lock_digest = _backend_source_identity_inputs(
        project_root, _backend_source_paths(project_root, backend_features)
    )
    meta = f"profile:{cargo_profile}\n"
    meta += f"cargo_build:{build_admission.fingerprint}\n"
    meta += f"features:{','.join(backend_features)}\n"
    meta += f"locked_dependencies:{lock_digest}\n"
    meta_digest = hashlib.sha256(meta.encode("utf-8")).hexdigest()
    rustc_info = canonical_json_sha256(
        next(
            item.content_record()
            for item in build_admission.plan.executable_custody
            if item.label == "tool/rustc"
        )
    )
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
    except OSError as exc:
        raise CompilerIdentityError(
            f"Compiler sources could not be read: {exc}"
        ) from exc
    return {
        "hash": hasher.hexdigest(),
        "rustc": rustc_info,
        "inputs_digest": inputs_digest,
        "meta_digest": meta_digest,
        "source_state": source_state,
    }


@_structured_backend_lock_failure
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
        installed = installed_compiler_admission(
            project_root, backend_features, cargo_profile
        )
        if installed is not None:
            if backend_bin != installed.compiler.binary:
                raise ValueError(
                    "Selected compiler differs from the installed compiler"
                )
            return _BackendBinaryEnsureResult(
                ok=True,
                cache_compiler_fingerprint=installed.fingerprint,
            )
    except (OSError, ValueError) as exc:
        return _backend_ensure_failure("installed_compiler", str(exc))
    try:
        build_admission = backend_build_admission(
            project_root, backend_features, cargo_profile, os.environ
        )
    except CompilerIdentityError as exc:
        return _backend_ensure_failure("backend_source_identity", str(exc))
    fingerprint_path = _backend_fingerprint_path(
        project_root, backend_bin, cargo_profile
    )
    probe_validation_path = _backend_probe_validation_path(
        project_root, backend_bin, cargo_profile
    )
    # Every feature lane publishes the same Cargo output before its alias.
    with _backend_admission_lock(
        project_root, cargo_profile, cargo_timeout=cargo_timeout
    ):
        try:
            build_admission.verify()
        except CompilerIdentityError as exc:
            return _backend_ensure_failure("backend_source_identity", str(exc))
        stage_start = time.perf_counter()
        stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        _record_backend_binary_stage_ms(
            stage_timings_ms,
            "backend_binary_read_fingerprint",
            stage_start,
        )
        stage_start = time.perf_counter()
        try:
            fingerprint = _backend_fingerprint(
                project_root,
                cargo_profile=cargo_profile,
                build_admission=build_admission,
                backend_features=backend_features,
                stored_fingerprint=stored_fingerprint,
            )
        except (OSError, ValueError) as exc:
            return _backend_ensure_failure("backend_source_identity", str(exc))
        _record_backend_binary_stage_ms(
            stage_timings_ms,
            "backend_binary_compute_fingerprint",
            stage_start,
        )
        rebuilt_source_identity: StableRegularFileIdentity | None = None
        rebuilt_alias_identity: StableRegularFileIdentity | None = None

        def _canonical_cargo_backend_output() -> Path:
            return backend_bin.parent / backend_executable_name(os_name=os.name)

        def _materialize_backend_binary_from(
            source: Path,
            *,
            expected_identity: StableRegularFileIdentity | None = None,
        ) -> tuple[StableRegularFileIdentity, StableRegularFileIdentity] | None:
            if not source.exists():
                return None
            # The alias is the exact Cargo bytes. The source already passed an
            # executable probe, and publication replaces the inode, so its
            # linker signature stays valid; re-signing would only rewrite the
            # bytes (and the compiler identity) on every build.
            with stable_executable_probe(
                source, label="backend alias source", identity=expected_identity
            ) as (
                _entrypoint,
                identity,
            ):
                if source != backend_bin:
                    _atomic_copy_file(source, backend_bin)
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
            expected_identity: StableRegularFileIdentity | None = None,
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
                    binary_path or backend_bin,
                    label="backend probe executable",
                    identity=expected_identity,
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
                detail = str(exc)
                if isinstance(exc, subprocess.TimeoutExpired):
                    for label, output in (
                        ("stderr", exc.stderr),
                        ("stdout", exc.stdout),
                    ):
                        if tail := _process_text_tail(output):
                            detail += f"\nProbe {label}:\n{tail}"
                return _backend_ensure_failure(
                    "backend_feature_probe", detail, command=probe_cmd
                )
            finally:
                try:
                    probe_path.unlink()
                except OSError:
                    pass
            stderr = probe.stderr.decode(errors="replace")
            stdout = probe.stdout.decode(errors="replace")
            if probe.returncode == 0 and payload is not None:
                try:
                    _atomic_write_json(
                        _backend_probe_validation_path(
                            project_root, binary_path or backend_bin, cargo_profile
                        ),
                        payload,
                        indent=2,
                    )
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
            nonlocal fingerprint
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
                    if fingerprint is None:
                        raise ValueError("Backend source identity missing")
                    fingerprint = _admitted_runtime_fingerprint(
                        fingerprint,
                        _read_runtime_fingerprint(candidate_fingerprint_path),
                    )
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
                        probe_target,
                        binary_path=cargo_output,
                        expected_identity=cargo_identity,
                    )
                    if not probe_result:
                        # A rejected capability probe is not source admission.
                        # Let the normal receipt/hydration/Cargo paths repair it;
                        # publication failures remain terminal.
                        return (
                            probe_result
                            if probe_result.phase == "backend_probe_publication"
                            else None
                        )
                    materialized = _materialize_backend_binary_from(
                        cargo_output, expected_identity=cargo_identity
                    )
                    if materialized is None:
                        return _backend_ensure_failure(
                            "backend_artifact", "Backend alias materialization failed."
                        )
                    _publish_backend_artifact(backend_bin, materialized[1])
                    # Transfer successful probe evidence only when publication
                    # preserved the exact probed bytes. Codesigning or any
                    # mutation requires the normal alias probe below instead.
                    if (
                        materialized[0].sha256 == materialized[1].sha256
                        and materialized[0].size == materialized[1].size
                    ):
                        payload = _backend_probe_validation_payload(
                            binary_identity=materialized[1],
                            probe_target=probe_target,
                            backend_features=backend_features,
                            fingerprint=fingerprint,
                        )
                        if payload is not None:
                            verify_stable_regular_file_identity(
                                materialized[1], label="backend probe transfer"
                            )
                            _atomic_write_json(probe_validation_path, payload, indent=2)
                            verify_stable_regular_file_identity(
                                materialized[1], label="backend probe transfer"
                            )
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
        probe_failure: _BackendBinaryEnsureResult | None = None
        alias_failure = _refresh_feature_tagged_backend_alias(_quick_target)
        if alias_failure is not None:
            return alias_failure
        stage_start = time.perf_counter()
        if _runtime_artifact_fingerprint_matches(
            backend_bin, fingerprint, fingerprint_path, require_artifact_digest=True
        ):
            # A temporarily unavailable rustc must not rename an admitted
            # compiler. Fill only unknown semantic coordinates from the receipt
            # whose source and artifact bytes just matched under this lock.
            try:
                if fingerprint is None:
                    raise ValueError("Backend source identity missing")
                fingerprint = _admitted_runtime_fingerprint(
                    fingerprint, _read_runtime_fingerprint(fingerprint_path)
                )
            except ValueError as exc:
                return _backend_ensure_failure("backend_receipt_identity", str(exc))
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_artifact_freshness",
                stage_start,
            )
            if fingerprint is not None and _runtime_fingerprint_metadata_needs_refresh(
                stored_fingerprint, fingerprint
            ):
                # Only fast-path metadata moves here (a same-content touch or
                # stash); an unwritable receipt just costs the next run a rehash.
                # The refresh still refuses to change the identity matched above.
                try:
                    _refresh_runtime_fingerprint_metadata(
                        fingerprint_path,
                        fingerprint,
                    )
                except OSError:
                    pass
                except ValueError as exc:
                    return _backend_ensure_failure(
                        "backend_receipt_refresh",
                        f"Backend receipt changed during admission: {exc}",
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
            probe_failure = _probe_result
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
            try:
                assert fingerprint is not None
                fingerprint = _admitted_runtime_fingerprint(
                    fingerprint, _read_runtime_fingerprint(fingerprint_path)
                )
            except ValueError as exc:
                return _backend_ensure_failure("backend_receipt_identity", str(exc))
            _probe_target = _backend_probe_target()
            _probe_result = _probe_backend_binary_support(_probe_target)
            if _probe_result:
                return _backend_ensure_success(
                    binary_path=backend_bin, fingerprint=fingerprint
                )
            if _probe_result.phase == "backend_probe_publication":
                return _probe_result
            probe_failure = _probe_result
        else:
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_canonical_hydrate",
                stage_start,
            )
        if reason := source_build_disabled_reason("Backend compiler"):
            if probe_failure is not None and probe_failure.detail:
                reason += f"\nBackend feature probe failed: {probe_failure.detail}"
            return _backend_ensure_failure("rebuild-policy", reason)
        # Raw Cargo outputs and source-only sidecars do not establish provenance.
        # Confirm the source/feature build before publishing content-bound receipts.
        if not json_output:
            _progress.notice(
                "Compiler artifact needs a source build to establish provenance"
            )
        # Cache entries include backend/tooling/runtime identity in their keys.
        # A backend rebuild therefore invalidates by selecting new keys, not by
        # deleting shared immutable cache artifacts that concurrent sessions may
        # still be reading. Size/age retention belongs to `molt clean`.
        # Live generation custody is distinct from semantic cache identity. It
        # rejects A -> B -> A edits while Cargo could have consumed B, even when
        # bytes and mtime are restored before the final content comparison.
        try:
            source_generation = CompilerSourceGeneration.capture(
                [
                    *_backend_source_paths(project_root, backend_features),
                    project_root / "Cargo.lock",
                ]
            )
        except (OSError, ValueError) as exc:
            return _backend_ensure_failure("backend_source_identity", str(exc))
        cmd = list(build_admission.plan.command)
        try:
            source_generation.verify()
        except (OSError, ValueError) as exc:
            return _backend_ensure_failure(
                "backend_source_identity", str(exc), command=cmd
            )
        stage_start = time.perf_counter()
        try:
            build = _run_resolved_cargo_plan(
                build_admission.plan,
                timeout=cargo_timeout,
                json_output=json_output,
                label="Backend build",
            )
        except (CargoPlanExecutionError, CompilerIdentityError) as exc:
            return _backend_ensure_failure(
                "backend_source_identity", str(exc), command=cmd
            )
        except subprocess.TimeoutExpired:
            timeout_note = (
                f"Backend build timed out after {cargo_timeout:.1f}s."
                if cargo_timeout is not None
                else "Backend build timed out."
            )
            return _backend_ensure_failure(
                "backend_cargo_build", timeout_note, command=cmd
            )
        except (OSError, ValueError) as exc:
            return _backend_ensure_failure(
                "backend_cargo_build",
                f"Backend Cargo admission or spawn failed: {exc}",
                command=cmd,
            )
        finally:
            _record_backend_binary_stage_ms(
                stage_timings_ms, "backend_binary_cargo_build", stage_start
            )
        if build.returncode != 0:
            return _backend_ensure_failure(
                "backend_cargo_build",
                _completed_process_failure_detail("Backend cargo build", build),
                returncode=build.returncode,
                command=cmd,
            )
        # Cargo success is not publication authority: discard operation caches
        # and compare every source/lock input against the pre-build generation.
        # The execution plan verifies exact config/tool/resource bytes separately.
        try:
            from molt.cli.cache_fingerprints import _fresh_compiler_identity_inputs

            build_admission.verify()
            source_generation.verify()
            with _fresh_compiler_identity_inputs():
                current = _backend_fingerprint(
                    project_root,
                    cargo_profile=cargo_profile,
                    build_admission=build_admission,
                    backend_features=backend_features,
                    stored_fingerprint=None,
                )
            if _artifact_semantic_identity(current) != _artifact_semantic_identity(
                fingerprint
            ):
                raise CompilerIdentityError(
                    "Compiler sources changed during Cargo build"
                )
            source_generation.verify()
        except (OSError, ValueError) as exc:
            return _backend_ensure_failure(
                "backend_source_identity", str(exc), command=cmd
            )
        # Cargo always produces target/<profile>/molt-backend regardless of
        # features. For every selected feature set, including native, copy
        # the freshly-built binary to the feature-tagged path so that
        # concurrent or sequential builds with different feature sets
        # (native vs wasm vs rust) do not overwrite each other.
        _materialization = _materialize_rebuilt_backend_binary()
        if not _materialization:
            return _materialization
        # Admit the built compiler before publishing provenance. A failed probe
        # is a failed build outcome: rerunning the same Cargo plan changes no
        # input and cannot establish why that compiler was unusable.
        _probe_target = _backend_probe_target()
        _probe_result = _probe_backend_binary_support(_probe_target)
        if _probe_result.phase == "backend_probe_publication" and not _probe_result:
            return _probe_result
        if not _probe_result:
            detail = "Built backend failed its feature probe."
            if _probe_result.detail:
                detail += f"\n{_probe_result.detail}"
            return _backend_ensure_failure(
                "backend_feature_probe",
                detail,
                returncode=_probe_result.returncode,
                command=_probe_result.command,
            )
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
