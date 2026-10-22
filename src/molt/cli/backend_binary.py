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

from molt.cargo_execution_policy import source_build_disabled_reason
from molt.cli import progress as _progress
from molt.cli.artifact_state import (
    _artifact_state_path,
    _artifact_state_path_for_build_state_root,
    _canonical_build_state_root,
    _canonical_target_root,
)
from molt.cli.atomic_io import _atomic_write_json
from molt.cli.backend_generation import (
    publish_backend_generation,
    record_backend_generation_use,
    selected_backend_generation,
)
from molt.cli.build_locks import _build_lock
from molt.cli.cache_fingerprints import (
    _backend_source_identity_inputs,
    _backend_source_paths,
)
from molt.cli.cargo_build_reads import cargo_build_reads, uncovered_build_inputs
from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
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
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.cli.static_archive_identity import artifact_content_identity
from molt.exact_json import canonical_json_sha256, read_exact
from molt import input_report as _input_report
from molt.file_hashing import (
    _hash_source_tree_metadata,
    _hash_source_tree_paths,
    _iter_source_fingerprint_files,
)
from molt.host_tool_products import HostToolProduct
from molt.process_environment import ProcessEnvironment
from molt.python_identity_common import _valid_sha256
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    executable_content_identity,
    stable_executable_probe,
    verify_stable_regular_file_identity,
)


# Version 4: the probed executable is the immutable generation, not an alias.
_BACKEND_PROBE_VALIDATION_SCHEMA_VERSION = 4
_BACKEND_PROBE_VALIDATION_MAX_BYTES = 64 * 1024
_BACKEND_COMPILER_CACHE_FINGERPRINT_SCHEMA_VERSION = 3


@dataclass(frozen=True)
class _BackendBinaryEnsureResult:
    """One admission outcome; success names the executable the build runs.

    ``binary`` is an immutable generation (or the installed compiler). A build
    resolves it once and runs it for every later step, so a concurrent
    publication of another generation never changes what this build holds.
    """

    ok: bool
    detail: str | None = None
    returncode: int | None = None
    phase: str | None = None
    command: tuple[str, ...] = ()
    cache_compiler_fingerprint: str | None = None
    binary: Path | None = None

    def __post_init__(self) -> None:
        if self.ok and self.binary is None:
            raise ValueError("an admitted backend names the executable it admitted")

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
    coordinate: Path,
    binary_path: Path,
    fingerprint: Mapping[str, Any] | None = None,
) -> _BackendBinaryEnsureResult:
    try:
        identity = executable_content_identity(binary_path, label="backend executable")
    except (OSError, ValueError) as exc:
        return _backend_ensure_failure("backend_binary_identity", str(exc))
    try:
        record_backend_generation_use(coordinate, binary_path)
    except (OSError, RuntimeError, ValueError) as exc:
        return _backend_ensure_failure("backend_generation_store", str(exc))
    return _BackendBinaryEnsureResult(
        ok=True,
        cache_compiler_fingerprint=_backend_compiler_cache_fingerprint(
            fingerprint, identity
        ),
        binary=binary_path,
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


# A probe that timed out or could not publish its receipt says nothing about
# the compiler's capability: hydrating or rebuilding cannot repair it, and a
# rebuilt backend is a new executable whose first run is slow again (HF-233).
_PROBE_TIMEOUT_PHASE = "backend_feature_probe_timeout"
_PROBE_TERMINAL_PHASES = frozenset({"backend_probe_publication", _PROBE_TIMEOUT_PHASE})


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


def _canonical_backend_candidate(
    project_root: Path, backend_bin: Path, cargo_profile: str
) -> tuple[Path, Path]:
    """The canonical target's backend coordinate and its selecting receipt."""
    binary = (
        _canonical_target_root(project_root)
        / _cargo_profile_dir(cargo_profile)
        / backend_bin.name
    )
    receipt = _artifact_state_path_for_build_state_root(
        _canonical_build_state_root(project_root),
        binary,
        subdir="backend_fingerprints",
        stem_suffix=f"{cargo_profile}",
        extension="fingerprint",
    )
    return binary, receipt


def _backend_probe_validation_path(
    project_root: Path,
    coordinate: Path,
    cargo_profile: str,
) -> Path:
    return _artifact_state_path(
        project_root,
        coordinate,
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
    product: HostToolProduct = HostToolProduct.BACKEND,
) -> dict[str, Any]:
    source_paths, lock_digest = _backend_source_identity_inputs(
        project_root,
        _backend_source_paths(project_root, backend_features, product=product),
        product=product,
    )
    if _input_report.active():
        # Enumerate every backend source for the build record, whichever
        # shortcut this fingerprint takes below.
        for source in (*source_paths, project_root / "Cargo.lock"):
            for path in _iter_source_fingerprint_files(
                source, on_directory=_input_report.report_directory
            ):
                _input_report.report_file(path)
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


def _current_backend_fingerprint(
    project_root: Path,
    *,
    cargo_profile: str,
    backend_features: tuple[str, ...],
    build_admission: BackendBuildAdmission,
    product: HostToolProduct = HostToolProduct.BACKEND,
) -> dict[str, Any]:
    """This checkout's selected host-tool identity, without a stored receipt."""
    return _backend_fingerprint(
        project_root,
        cargo_profile=cargo_profile,
        build_admission=build_admission,
        product=product,
        backend_features=backend_features,
    )


def _selected_backend_generation_matches(
    coordinate: Path,
    fingerprint: dict[str, Any] | None,
    receipt: Path,
    *,
    environment: ProcessEnvironment,
) -> Path | None:
    """The generation ``receipt`` selects for ``coordinate`` when it is admitted.

    A match needs the receipt's source identity and the generation's bytes. The
    receipt can name a newer selection by the time the bytes are compared; the
    comparison then fails, and the generation this returned stays unchanged.
    """
    generation = selected_backend_generation(
        coordinate, _read_runtime_fingerprint(receipt)
    )
    if generation is None or not _runtime_artifact_fingerprint_matches(
        generation,
        fingerprint,
        receipt,
        environment=environment,
        require_artifact_digest=True,
    ):
        return None
    return generation


def admitted_backend_binary(
    project_root: Path,
    *,
    binary: Path,
    cargo_profile: str,
    backend_features: tuple[str, ...],
    rust_cpu: RustCpuPolicy,
) -> tuple[Path, Path]:
    """The backend generation and receipt a build of this checkout reuses.

    Candidates are the selected coordinate and the canonical one, in the order
    a build consults them. A candidate counts only when its receipt matches this
    checkout's backend identity and binds the selected generation's content.
    """
    build_admission = backend_build_admission(
        project_root, backend_features, cargo_profile, os.environ, rust_cpu=rust_cpu
    )
    build_admission.verify()
    fingerprint = _current_backend_fingerprint(
        project_root,
        cargo_profile=cargo_profile,
        backend_features=backend_features,
        build_admission=build_admission,
    )
    for coordinate, receipt in (
        (binary, _backend_fingerprint_path(project_root, binary, cargo_profile)),
        _canonical_backend_candidate(project_root, binary, cargo_profile),
    ):
        generation = _selected_backend_generation_matches(
            coordinate,
            fingerprint,
            receipt,
            environment=build_admission.plan.environment,
        )
        if generation is not None:
            return generation, receipt
    raise ValueError(f"no backend compiler admitted for this checkout: {binary}")


def _publish_backend_generation_from(
    coordinate: Path,
    source: Path,
    *,
    label: str,
    expected_identity: StableRegularFileIdentity | None = None,
) -> tuple[StableRegularFileIdentity, StableRegularFileIdentity]:
    """Publish an executable's bytes as a generation of ``coordinate``.

    Returns the source identity the copy was bound to and the generation.
    A generation is the exact source bytes: a copy keeps the linker signature
    valid, and re-signing would only rewrite the bytes (and the compiler
    identity) on every build.
    """
    with stable_executable_probe(source, label=label, identity=expected_identity) as (
        _entrypoint,
        identity,
    ):
        generation = publish_backend_generation(
            coordinate, identity.path, observed=identity
        )
    return identity, generation


def _select_backend_generation(
    receipt: Path,
    generation: StableRegularFileIdentity,
    *,
    fingerprint: dict[str, Any],
    cargo_environment: CargoEnvironmentInputs,
) -> None:
    """Point a coordinate's ``receipt`` at ``generation``: the selection switch.

    The receipt names the source identity and the generation's bytes. Only
    this file changes; the generation, and every other one, stays as it was.
    """
    verify_stable_regular_file_identity(generation, label="backend selection")
    content_identity = artifact_content_identity(generation.path, observed=generation)
    _write_runtime_fingerprint(
        receipt,
        {**fingerprint, "artifact_content_identity": content_identity},
        cargo_environment=cargo_environment,
    )
    verify_stable_regular_file_identity(generation, label="backend selection")


def import_backend_binary(
    project_root: Path,
    *,
    binary: Path,
    cargo_profile: str,
    backend_features: tuple[str, ...],
    rust_cpu: RustCpuPolicy,
    executable: Path,
    receipt: Path,
) -> Path:
    """Admit another checkout's backend as this checkout's canonical candidate.

    The transported receipt must carry this checkout's freshly hashed backend
    identity and bind the executable's content. The executable becomes one
    generation of the canonical coordinate, which a build then hydrates
    exactly as it hydrates one built locally.
    """
    transported = _read_runtime_fingerprint(receipt)
    if transported is None or "artifact_content_identity" not in transported:
        raise ValueError(f"transported backend receipt is invalid: {receipt}")
    build_admission = backend_build_admission(
        project_root, backend_features, cargo_profile, os.environ, rust_cpu=rust_cpu
    )
    build_admission.verify()
    fingerprint = _current_backend_fingerprint(
        project_root,
        cargo_profile=cargo_profile,
        backend_features=backend_features,
        build_admission=build_admission,
    )
    if any(
        transported.get(key) != fingerprint.get(key)
        for key in ("hash", "rustc", "meta_digest")
    ):
        raise ValueError(
            "transported backend was built from other inputs than this checkout"
        )
    cargo_environment = CargoEnvironmentInputs.from_dict(
        transported.get("cargo_environment")
    )
    cargo_environment.verify(build_admission.plan.environment)
    canonical_binary, canonical_receipt = _canonical_backend_candidate(
        project_root, binary, cargo_profile
    )
    with _backend_admission_lock(project_root, cargo_profile):
        build_admission.verify()
        with stable_executable_probe(executable, label="transported backend") as (
            _entrypoint,
            identity,
        ):
            if (
                artifact_content_identity(identity.path, observed=identity)
                != transported["artifact_content_identity"]
            ):
                raise ValueError("transported backend differs from its receipt")
            generation = publish_backend_generation(
                canonical_binary, identity.path, observed=identity
            )
        build_admission.verify()
        _select_backend_generation(
            canonical_receipt,
            generation,
            fingerprint=fingerprint,
            cargo_environment=cargo_environment,
        )
        admitted = _selected_backend_generation_matches(
            canonical_binary,
            fingerprint,
            canonical_receipt,
            environment=build_admission.plan.environment,
        )
        if admitted is None or admitted.resolve() != generation.path.resolve():
            raise ValueError("imported backend failed its own admission")
    return generation.path


def _backend_probe_target(
    backend_features: tuple[str, ...],
    product: HostToolProduct = HostToolProduct.BACKEND,
) -> str:
    if product is not HostToolProduct.BACKEND:
        return product.value
    if "wasm-backend" in backend_features:
        return "wasm"
    if "luau-backend" in backend_features:
        return "luau"
    if "rust-backend" in backend_features:
        return "rust"
    return "native"


def _host_tool_output_name(product: HostToolProduct) -> str:
    return product.value + (".exe" if os.name == "nt" else "")


def _is_cargo_output_coordinate(
    coordinate: Path, product: HostToolProduct = HostToolProduct.BACKEND
) -> bool:
    return coordinate == coordinate.parent / _host_tool_output_name(product)


def _admit_published_backend(
    backend_bin: Path,
    *,
    project_root: Path,
    cargo_profile: str,
    build_admission: BackendBuildAdmission,
    backend_features: tuple[str, ...],
    fingerprint_path: Path,
    probe_validation_path: Path,
    product: HostToolProduct = HostToolProduct.BACKEND,
) -> _BackendBinaryEnsureResult | None:
    """Admit a published feature lane by reads alone, or return None.

    The build lock serializes Cargo's shared output and publication, which
    only a build or a republication needs. Every check here binds content: the
    receipt names the source identity and the selected generation's bytes, and
    the probe evidence names those bytes. A concurrent publication can make a
    check fail but never pass for other bytes, and any failure takes the
    locked path, which repairs it. So reusing one lane never waits out another
    lane's cold build.
    """
    try:
        build_admission.verify()
        stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        if stored_fingerprint is None:
            return None
        fingerprint = _backend_fingerprint(
            project_root,
            cargo_profile=cargo_profile,
            build_admission=build_admission,
            backend_features=backend_features,
            stored_fingerprint=stored_fingerprint,
            product=product,
        )
        generation = _selected_backend_generation_matches(
            backend_bin,
            fingerprint,
            fingerprint_path,
            environment=build_admission.plan.environment,
        )
        if generation is None:
            return None
        fingerprint = _admitted_runtime_fingerprint(
            fingerprint, _read_runtime_fingerprint(fingerprint_path)
        )
        CargoEnvironmentInputs.from_dict(fingerprint.get("cargo_environment")).verify(
            build_admission.plan.environment
        )
        if _runtime_fingerprint_metadata_needs_refresh(stored_fingerprint, fingerprint):
            return None
        if not _backend_probe_validation_matches(
            probe_validation_path,
            binary_path=generation,
            probe_target=_backend_probe_target(backend_features, product),
            backend_features=backend_features,
            fingerprint=fingerprint,
        ):
            return None
    except (CompilerIdentityError, OSError, ValueError):
        return None
    return _backend_ensure_success(
        coordinate=backend_bin, binary_path=generation, fingerprint=fingerprint
    )


@_structured_backend_lock_failure
def _ensure_backend_binary(
    backend_bin: Path,
    *,
    cargo_timeout: float | None,
    json_output: bool,
    cargo_profile: str,
    project_root: Path,
    backend_features: tuple[str, ...],
    rust_cpu: RustCpuPolicy,
    stage_timings_ms: dict[str, float] | None = None,
    product: HostToolProduct = HostToolProduct.BACKEND,
    environment: Mapping[str, str] | None = None,
) -> _BackendBinaryEnsureResult:
    """Admit one selected host-tool generation, publishing one if needed.

    ``backend_bin`` is the variant's coordinate. Success names the immutable
    generation the build runs; the coordinate itself is never an executable.
    """
    if not isinstance(product, HostToolProduct):
        return _backend_ensure_failure("host_tool_product", "Unknown host tool product")
    tool_label = product.label
    # Installed compilers are immutable release inputs, never Cargo outputs.
    # Admit before every developer skip/hydration/rebuild path.
    try:
        installed = installed_compiler_admission(
            project_root,
            backend_features if product is HostToolProduct.BACKEND else None,
            cargo_profile if product is HostToolProduct.BACKEND else None,
        )
        if installed is not None:
            if product is HostToolProduct.JIT:
                raise ValueError(
                    "Installed Molt ships no JIT runner; the JIT tier needs a "
                    "source checkout"
                )
            if rust_cpu is not RustCpuPolicy.BASELINE:
                raise ValueError(
                    "Installed Molt ships a baseline-CPU backend compiler; "
                    "MOLT_PERF_PROFILE=native-arch needs a source checkout"
                )
            installed_binary = installed.compiler.binary
            if product is HostToolProduct.WASM_FACTS:
                installed.compiler.verify_wasm_facts()
                installed_binary = installed.compiler.wasm_facts_binary
            if backend_bin != installed_binary:
                raise ValueError(
                    f"Selected {product.value} differs from the installed product"
                )
            return _BackendBinaryEnsureResult(
                ok=True,
                cache_compiler_fingerprint=installed.fingerprint,
                binary=installed_binary,
            )
    except (OSError, ValueError) as exc:
        return _backend_ensure_failure("installed_compiler", str(exc))
    if _is_cargo_output_coordinate(backend_bin, product):
        # Cargo's shared output belongs to whichever lane built last; only a
        # feature variant names a selection.
        return _backend_ensure_failure(
            "backend_artifact",
            f"{tool_label} coordinate names Cargo's shared output, not a variant: {backend_bin}",
        )
    try:
        admission_environment = ProcessEnvironment(
            os.environ if environment is None else environment
        )
        build_admission = backend_build_admission(
            project_root,
            backend_features,
            cargo_profile,
            admission_environment,
            rust_cpu=rust_cpu,
            product=product,
        )
    except CompilerIdentityError as exc:
        return _backend_ensure_failure("backend_source_identity", str(exc))
    fingerprint_path = _backend_fingerprint_path(
        project_root, backend_bin, cargo_profile
    )
    probe_validation_path = _backend_probe_validation_path(
        project_root, backend_bin, cargo_profile
    )
    stage_start = time.perf_counter()
    published = _admit_published_backend(
        backend_bin,
        project_root=project_root,
        cargo_profile=cargo_profile,
        build_admission=build_admission,
        backend_features=backend_features,
        fingerprint_path=fingerprint_path,
        probe_validation_path=probe_validation_path,
        product=product,
    )
    if published is not None:
        _record_backend_binary_stage_ms(
            stage_timings_ms, "backend_binary_published_admission", stage_start
        )
        return published
    # Every feature lane publishes from the same Cargo output under this lock.
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
                product=product,
            )
        except (OSError, ValueError) as exc:
            return _backend_ensure_failure("backend_source_identity", str(exc))
        _record_backend_binary_stage_ms(
            stage_timings_ms,
            "backend_binary_compute_fingerprint",
            stage_start,
        )
        cargo_output = backend_bin.parent / _host_tool_output_name(product)

        def _publish_cargo_output_receipt(
            identity: StableRegularFileIdentity,
            cargo_environment: CargoEnvironmentInputs,
        ) -> None:
            assert fingerprint is not None
            verify_stable_regular_file_identity(identity, label="backend publication")
            content_identity = artifact_content_identity(
                identity.path, observed=identity
            )
            verify_stable_regular_file_identity(identity, label="backend publication")
            _write_runtime_fingerprint(
                _backend_fingerprint_path(project_root, cargo_output, cargo_profile),
                {**fingerprint, "artifact_content_identity": content_identity},
                cargo_environment=cargo_environment,
            )
            verify_stable_regular_file_identity(identity, label="backend publication")

        def _probe_backend_binary_support(
            probe_target: str,
            *,
            binary: Path,
            identity: StableRegularFileIdentity | None = None,
        ) -> _BackendBinaryEnsureResult:
            stage_start = time.perf_counter()
            probe_ir: bytes | None = json.dumps(
                {
                    "functions": [],
                    "module": "__probe__",
                    "entry": "main",
                    "metadata": {"target": probe_target, "deterministic": True},
                }
            ).encode()
            probe_suffix = ".o"
            if probe_target == "wasm" or product is HostToolProduct.WASM_FACTS:
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
            probe_cmd = [str(binary), "--output", str(probe_path)]
            if product is HostToolProduct.WASM_FACTS:
                probe_ir = None
                probe_cmd = [str(binary), "--scan-wasm-link-facts", str(probe_path)]
            elif product is HostToolProduct.JIT:
                # Compile and run one function in executable memory: the
                # host's W^X policy must admit the runner before any program.
                probe_ir = None
                probe_cmd = [str(binary), "--probe"]
            elif probe_target == "wasm":
                probe_cmd.extend(["--target", "wasm"])
            elif probe_target == "luau":
                probe_cmd.extend(["--target", "luau"])
            elif probe_target == "rust":
                probe_cmd.extend(["--target", "rust"])
            try:
                if product is HostToolProduct.WASM_FACTS:
                    probe_tmp.write(b"\0asm\x01\0\0\0")
                probe_tmp.close()
                with stable_executable_probe(
                    binary,
                    label="backend probe executable",
                    identity=identity,
                ) as (_entrypoint, probed):
                    payload = _backend_probe_validation_payload(
                        binary_identity=probed,
                        probe_target=probe_target,
                        backend_features=backend_features,
                        fingerprint=fingerprint,
                    )
                    # The probe is the first run of a newly built backend, and a
                    # first run can wait on the operating system's check of a new
                    # executable: 11.4 s for a never-seen backend on macOS at
                    # load 7, 0.2 s warm (HF-233). It shares the budget of the
                    # step that built the binary instead of a fixed limit.
                    probe = _run_subprocess_captured_to_tempfiles(
                        probe_cmd,
                        input=probe_ir,
                        cwd=project_root,
                        env=build_admission.plan.environment,
                        timeout=cargo_timeout,
                        memory_guard_prefix="MOLT_BUILD",
                    )
            except subprocess.TimeoutExpired as exc:
                _record_backend_binary_stage_ms(
                    stage_timings_ms,
                    "backend_binary_probe",
                    stage_start,
                )
                detail = (
                    f"{tool_label} capability probe did not finish within {exc.timeout:.1f}s "
                    "(MOLT_CARGO_TIMEOUT); the first run of a newly built backend "
                    "can wait on the operating system's check of a new executable."
                )
                for label, output in (("stderr", exc.stderr), ("stdout", exc.stdout)):
                    if tail := _process_text_tail(output):
                        detail += f"\nProbe {label}:\n{tail}"
                return _backend_ensure_failure(
                    _PROBE_TIMEOUT_PHASE, detail, command=probe_cmd
                )
            except (OSError, ValueError) as exc:
                _record_backend_binary_stage_ms(
                    stage_timings_ms,
                    "backend_binary_probe",
                    stage_start,
                )
                return _backend_ensure_failure(
                    "backend_feature_probe", str(exc), command=probe_cmd
                )
            finally:
                probe_tmp.close()
                try:
                    probe_path.unlink()
                except OSError:
                    pass
            stderr = probe.stderr.decode(errors="replace")
            stdout = probe.stdout.decode(errors="replace")
            if probe.returncode == 0 and payload is not None:
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
                        f"{tool_label} probe validation publication failed: {exc}",
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
                binary=binary,
            )

        def _select_admitted_cargo_output(
            probe_target: str,
        ) -> _BackendBinaryEnsureResult | None:
            """Select an admitted Cargo output that the selection does not name."""
            nonlocal fingerprint
            if not cargo_output.exists():
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
                        environment=build_admission.plan.environment,
                        require_artifact_digest=True,
                    ):
                        return None
                    if fingerprint is None:
                        raise ValueError("Backend source identity missing")
                    fingerprint = _admitted_runtime_fingerprint(
                        fingerprint,
                        _read_runtime_fingerprint(candidate_fingerprint_path),
                    )
                    CargoEnvironmentInputs.from_dict(
                        fingerprint.get("cargo_environment")
                    ).verify(build_admission.plan.environment)
                    selected = _selected_backend_generation_matches(
                        backend_bin,
                        fingerprint,
                        fingerprint_path,
                        environment=build_admission.plan.environment,
                    )
                    if (
                        selected is not None
                        and selected.parent.name == cargo_identity.sha256
                    ):
                        return None
                    _source, generation = _publish_backend_generation_from(
                        backend_bin,
                        cargo_output,
                        label="admitted Cargo backend output",
                        expected_identity=cargo_identity,
                    )
                probe_result = _probe_backend_binary_support(
                    probe_target, binary=generation.path, identity=generation
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
                _select_backend_generation(
                    fingerprint_path,
                    generation,
                    fingerprint=fingerprint,
                    cargo_environment=CargoEnvironmentInputs.from_dict(
                        fingerprint.get("cargo_environment")
                    ),
                )
            except (OSError, RuntimeError, ValueError) as exc:
                return _backend_ensure_failure(
                    "backend_selection_publication", str(exc)
                )
            return None

        if stored_fingerprint is None:
            stage_start = time.perf_counter()
            stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_read_fingerprint",
                stage_start,
            )
        _quick_target = _backend_probe_target(backend_features, product)
        probe_failure: _BackendBinaryEnsureResult | None = None
        selection_failure = _select_admitted_cargo_output(_quick_target)
        if selection_failure is not None:
            return selection_failure
        stage_start = time.perf_counter()
        selected_generation = _selected_backend_generation_matches(
            backend_bin,
            fingerprint,
            fingerprint_path,
            environment=build_admission.plan.environment,
        )
        if selected_generation is not None:
            # A temporarily unavailable rustc must not rename an admitted
            # compiler. Fill only unknown semantic coordinates from the receipt
            # whose source and artifact bytes just matched under this lock.
            try:
                if fingerprint is None:
                    raise ValueError("Backend source identity missing")
                fingerprint = _admitted_runtime_fingerprint(
                    fingerprint, _read_runtime_fingerprint(fingerprint_path)
                )
                CargoEnvironmentInputs.from_dict(
                    fingerprint.get("cargo_environment")
                ).verify(build_admission.plan.environment)
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
                binary_path=selected_generation,
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
                    coordinate=backend_bin,
                    binary_path=selected_generation,
                    fingerprint=fingerprint,
                )
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_probe_validation",
                stage_start,
            )
            _probe_result = _probe_backend_binary_support(
                _quick_target, binary=selected_generation
            )
            if _probe_result:
                return _backend_ensure_success(
                    coordinate=backend_bin,
                    binary_path=selected_generation,
                    fingerprint=fingerprint,
                )
            if _probe_result.phase in _PROBE_TERMINAL_PHASES:
                return _probe_result
            probe_failure = _probe_result
        else:
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_artifact_freshness",
                stage_start,
            )
        canonical_backend_bin, canonical_fingerprint_path = (
            _canonical_backend_candidate(project_root, backend_bin, cargo_profile)
        )
        stage_start = time.perf_counter()
        hydrated = _hydrate_canonical_backend_generation(
            backend_bin,
            fingerprint_path,
            canonical_backend_bin,
            canonical_fingerprint_path,
            fingerprint=fingerprint,
            environment=build_admission.plan.environment,
        )
        _record_backend_binary_stage_ms(
            stage_timings_ms,
            "backend_binary_canonical_hydrate",
            stage_start,
        )
        if hydrated is not None:
            hydrated_generation, fingerprint = hydrated
            _probe_target = _backend_probe_target(backend_features, product)
            _probe_result = _probe_backend_binary_support(
                _probe_target,
                binary=hydrated_generation.path,
                identity=hydrated_generation,
            )
            if _probe_result:
                return _backend_ensure_success(
                    coordinate=backend_bin,
                    binary_path=hydrated_generation.path,
                    fingerprint=fingerprint,
                )
            if _probe_result.phase in _PROBE_TERMINAL_PHASES:
                return _probe_result
            probe_failure = _probe_result
        if reason := source_build_disabled_reason(
            product.value, environment=admission_environment
        ):
            if probe_failure is not None and probe_failure.detail:
                reason += (
                    f"\n{tool_label} capability probe failed: {probe_failure.detail}"
                )
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
                    *_backend_source_paths(
                        project_root, backend_features, product=product
                    ),
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
                label=f"{tool_label} build",
            )
        except (CargoPlanExecutionError, CompilerIdentityError) as exc:
            return _backend_ensure_failure(
                "backend_source_identity", str(exc), command=cmd
            )
        except subprocess.TimeoutExpired:
            timeout_note = (
                f"{tool_label} build timed out after {cargo_timeout:.1f}s."
                if cargo_timeout is not None
                else f"{tool_label} build timed out."
            )
            return _backend_ensure_failure(
                "backend_cargo_build", timeout_note, command=cmd
            )
        except (OSError, ValueError) as exc:
            return _backend_ensure_failure(
                "backend_cargo_build",
                f"{tool_label} Cargo admission or spawn failed: {exc}",
                command=cmd,
            )
        finally:
            _record_backend_binary_stage_ms(
                stage_timings_ms, "backend_binary_cargo_build", stage_start
            )
        if build.returncode != 0:
            return _backend_ensure_failure(
                "backend_cargo_build",
                _completed_process_failure_detail(f"{tool_label} cargo build", build),
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
                    product=product,
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
        # Cargo produces its selected package/bin regardless of backend
        # features. Every selected product/feature set publishes the
        # freshly built bytes as a generation of its own variant, so builds
        # with different feature sets (native vs wasm vs rust) never replace
        # each other's compiler, and no build replaces one that another holds.
        if not cargo_output.exists():
            return _backend_ensure_failure(
                "backend_artifact", f"{tool_label} binary missing after rebuild."
            )
        try:
            environment = build_admission.plan.environment
            reads = cargo_build_reads(
                build.stdout,
                workspace_root=project_root,
                dep_info_base=project_root,
                product_roots=tuple(
                    Path(environment[name])
                    for name in ("CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR")
                    if environment.get(name)
                ),
                toolchain_roots=tuple(
                    resource.identity.path
                    for custody in (
                        build_admission.plan.rust_resources,
                        build_admission.plan.link_resources,
                    )
                    for resource in custody.files
                ),
                environment=environment,
            )
            uncovered = uncovered_build_inputs(
                reads,
                project_root,
                _backend_source_paths(project_root, backend_features, product=product),
                identity=product.value,
            )
            build_admission.verify()
            source_generation.verify()
        except (OSError, ValueError) as exc:
            return _backend_ensure_failure(
                "backend_source_identity", str(exc), command=cmd
            )
        if uncovered is not None:
            # The generation is keyed by the source closure. A read outside it
            # would let a later run adopt this build after that input changed.
            return _backend_ensure_failure(
                "backend_build_input_coverage",
                f"Refusing to publish the {tool_label}: {uncovered}",
                command=cmd,
            )
        try:
            rebuilt_source_identity, rebuilt_generation = (
                _publish_backend_generation_from(
                    backend_bin, cargo_output, label="rebuilt Cargo backend output"
                )
            )
        except (OSError, RuntimeError, ValueError) as exc:
            return _backend_ensure_failure("backend_artifact", str(exc))
        # Admit the built compiler before publishing provenance. A failed probe
        # is a failed build outcome: rerunning the same Cargo plan changes no
        # input and cannot establish why that compiler was unusable.
        _probe_target = _backend_probe_target(backend_features, product)
        _probe_result = _probe_backend_binary_support(
            _probe_target,
            binary=rebuilt_generation.path,
            identity=rebuilt_generation,
        )
        if _probe_result.phase in _PROBE_TERMINAL_PHASES and not _probe_result:
            return _probe_result
        if not _probe_result:
            detail = f"Built {tool_label} failed its capability probe."
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
                _publish_cargo_output_receipt(
                    rebuilt_source_identity, reads.environment
                )
                _select_backend_generation(
                    fingerprint_path,
                    rebuilt_generation,
                    fingerprint=fingerprint,
                    cargo_environment=reads.environment,
                )
            except (OSError, ValueError) as exc:
                return _backend_ensure_failure(
                    "backend_artifact_publication",
                    f"{tool_label} artifact provenance publication failed: {exc}",
                    command=cmd,
                )
        return _backend_ensure_success(
            coordinate=backend_bin,
            binary_path=rebuilt_generation.path,
            fingerprint=fingerprint,
        )


def _hydrate_canonical_backend_generation(
    backend_bin: Path,
    fingerprint_path: Path,
    canonical_backend_bin: Path,
    canonical_fingerprint_path: Path,
    *,
    fingerprint: dict[str, Any] | None,
    environment: ProcessEnvironment,
) -> tuple[StableRegularFileIdentity, dict[str, Any]] | None:
    """Publish the canonical target's admitted generation into this target.

    The canonical selection must match this checkout's source identity and
    bind its generation's bytes. The copy becomes this coordinate's own
    generation and selection; the canonical one is only read. Returns the
    generation and the admitted fingerprint, or None when nothing matches.
    """
    if fingerprint is None or backend_bin.resolve() == canonical_backend_bin.resolve():
        return None
    stored = _read_runtime_fingerprint(canonical_fingerprint_path)
    candidate = selected_backend_generation(canonical_backend_bin, stored)
    if (
        stored is None
        or candidate is None
        or not _runtime_artifact_fingerprint_matches(
            candidate,
            fingerprint,
            canonical_fingerprint_path,
            environment=environment,
            require_artifact_digest=True,
        )
    ):
        return None
    try:
        admitted = _admitted_runtime_fingerprint(fingerprint, stored)
        CargoEnvironmentInputs.from_dict(admitted.get("cargo_environment")).verify(
            environment
        )
        _source, generation = _publish_backend_generation_from(
            backend_bin, candidate, label="canonical backend generation"
        )
        # The copy binds the bytes it read; they must be the bytes the
        # canonical receipt selected, or nothing is selected here.
        if (
            artifact_content_identity(generation.path, observed=generation)
            != stored["artifact_content_identity"]
        ):
            return None
        _select_backend_generation(
            fingerprint_path,
            generation,
            fingerprint=admitted,
            cargo_environment=CargoEnvironmentInputs.from_dict(
                admitted.get("cargo_environment")
            ),
        )
    except (OSError, RuntimeError, ValueError):
        return None
    return generation, admitted
