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
from molt.cli.artifact_state import _artifact_state_path
from molt.cli.atomic_io import _atomic_write_json
from molt.cli.backend_generation import (
    is_backend_generation,
    publish_backend_generation,
    selected_backend_generation,
)
from molt.cli.build_locks import BuildLockAcquisitionError, _build_lock
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
from molt.cli.native_launch_abi_generated import JIT_RUNNER_PROBE_FLAG
from molt.cli.product_store import ProductStore, ProductStoreError
from molt.cli.runtime_fingerprints import (
    _artifact_semantic_identity,
    _admitted_runtime_fingerprint,
    _fingerprint_payload,
    _read_runtime_fingerprint,
    _runtime_artifact_fingerprint_matches,
    _stored_fingerprint_matches_clean_source_state,
    _stored_fingerprint_matches_source_metadata,
)
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.cli.static_archive_identity import artifact_content_identity
from molt.exact_json import canonical_json_sha256, read_exact
from molt import input_report as _input_report
from molt.file_publication import atomic_write_bytes
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
    project_root: Path,
    coordinate: Path,
    cargo_profile: str,
):
    """Hold the lock over the Cargo output ``coordinate``'s variants publish from.

    Every feature variant of one target publishes from Cargo's one output in
    ``coordinate``'s profile directory, so the lock lives in that target's
    build control. A waiter waits while the holder builds, probes and
    publishes, by the one build-lock policy (``molt.file_locks.lock_wait_s``).
    """
    with ExitStack() as stack:
        try:
            stack.enter_context(
                _build_lock(
                    project_root,
                    f"backend.{cargo_profile}",
                    cargo_target=coordinate.parent.parent,
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
    # The selected generation of a written-once variant is never evicted, and
    # the product store renews the whole entry, so admission writes nothing.
    if not is_backend_generation(coordinate, binary_path):
        return _backend_ensure_failure(
            "backend_generation_store",
            f"backend executable is not a generation of {coordinate}: {binary_path}",
        )
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
    """This checkout's receipt for ``artifact``.

    For a variant coordinate it is the identity memo: the source identity and
    the metadata that let a later build reuse it without hashing sources. For
    Cargo's own output it also binds that output's bytes. Selection lives in
    the product store, never here.
    """
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="backend_fingerprints",
        stem_suffix=f"{cargo_profile}",
        extension="fingerprint",
    )


def host_tool_store(project_root: Path) -> ProductStore:
    """The product store every host-tool build of ``project_root`` uses."""
    return ProductStore.of("host-tool", project_root)


def host_tool_identity(coordinate: Path, fingerprint: Mapping[str, Any]) -> str:
    """The store key of the host tool ``coordinate`` names.

    The source fingerprint names the tool's sources, lockfile, toolchain,
    profile, features and product by content; the coordinate's name adds the
    variant. No checkout path enters it.
    """
    source = _artifact_semantic_identity(fingerprint)
    if any(source.get(key) is None for key in ("hash", "rustc", "meta_digest")):
        raise CompilerIdentityError("host tool source identity is incomplete")
    return canonical_json_sha256(
        {
            "schema": "molt.host-tool-identity.v1",
            "tool": coordinate.name,
            "source": source,
        }
    )


@dataclass(frozen=True, slots=True)
class StoredHostTool:
    """One store variant of a host tool: its receipt, probe evidence and generations.

    ``coordinate`` names the variant's tool; its immutable generations live
    beside it (``molt.cli.backend_generation``) and its receipt selects one.
    """

    coordinate: Path

    @property
    def receipt(self) -> Path:
        return self.coordinate.with_name(f"{self.coordinate.name}.fingerprint")

    @property
    def probe_validation(self) -> Path:
        return self.coordinate.with_name(f"{self.coordinate.name}.probe.json")


def stored_host_tool(
    store: ProductStore,
    identity: str,
    coordinate: Path,
    cargo_environment: CargoEnvironmentInputs,
) -> StoredHostTool:
    return StoredHostTool(store.variant(identity, cargo_environment) / coordinate.name)


@dataclass(frozen=True, slots=True)
class StoredHostToolAdmission:
    """A stored host tool admitted for one checkout's identity and environment."""

    stored: StoredHostTool
    generation: Path
    fingerprint: dict[str, Any]


def stored_host_tools(
    store: ProductStore,
    identity: str,
    coordinate: Path,
    fingerprint: Mapping[str, Any],
    *,
    environment: ProcessEnvironment,
) -> tuple[StoredHostToolAdmission, ...]:
    """The published variants of ``identity`` built under ``environment``.

    A variant whose receipt has not appeared is a publication in progress. A
    reader admits only its own variant, the one whose receipt records a Cargo
    environment that matches ``environment``, and checks no other variant's
    bytes, so a corrupt variant of another environment never fails it. Its
    own variant, when it exists but does not verify, raises
    ``ProductStoreError``. A receipt that cannot be read names no
    environment; if it is this reader's, the publication that follows the
    miss finds it (``_require_unpublished_host_tool``).
    """
    admitted = []
    for variant in store.variants(identity):
        stored = StoredHostTool(variant / coordinate.name)
        try:
            stored.receipt.lstat()
        except FileNotFoundError:
            continue
        receipt = _read_runtime_fingerprint(stored.receipt)
        if receipt is None or "artifact_content_identity" not in receipt:
            continue
        try:
            recorded = CargoEnvironmentInputs.from_dict(
                receipt.get("cargo_environment")
            )
        except (TypeError, ValueError):
            continue
        if recorded.digest != variant.name:
            continue
        try:
            recorded.verify(environment)
        except ValueError:
            continue
        if _artifact_semantic_identity(receipt) != _artifact_semantic_identity(
            fingerprint
        ):
            raise ProductStoreError(
                store, identity, "it holds another host tool identity", variant=variant
            )
        generation = selected_backend_generation(stored.coordinate, receipt)
        if generation is None or not _runtime_artifact_fingerprint_matches(
            generation,
            dict(fingerprint),
            stored.receipt,
            environment=environment,
            require_artifact_digest=True,
        ):
            raise ProductStoreError(
                store,
                identity,
                "its executable differs from its receipt",
                variant=variant,
            )
        admitted.append(
            StoredHostToolAdmission(
                stored,
                generation,
                _admitted_runtime_fingerprint(dict(fingerprint), receipt),
            )
        )
    return tuple(admitted)


def _require_unpublished_host_tool(
    store: ProductStore, identity: str, stored: StoredHostTool
) -> None:
    """Refuse to publish over a variant whose receipt already exists.

    The caller holds the identity's lock and admitted no variant for its
    environment, so an existing receipt for the environment it built under is
    its own variant, corrupt. A variant is written once.
    """
    if stored.receipt.exists():
        raise ProductStoreError(
            store,
            identity,
            "a receipt for this environment exists but does not verify",
            variant=stored.coordinate.parent,
        )


def _remember_backend_identity(
    memo: Path, stored: Mapping[str, Any] | None, fingerprint: Mapping[str, Any]
) -> None:
    """Keep this checkout's identity memo current; a failed write costs a rehash."""
    current = {key: fingerprint.get(key) for key in _BACKEND_IDENTITY_MEMO_KEYS}
    if stored is not None and all(
        stored.get(key) == value for key, value in current.items()
    ):
        return
    try:
        _atomic_write_json(memo, _fingerprint_payload(current), indent=2)
    except OSError:
        pass


_BACKEND_IDENTITY_MEMO_KEYS = (
    "hash",
    "rustc",
    "inputs_digest",
    "meta_digest",
    "source_state",
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


def admitted_backend_binary(
    project_root: Path,
    *,
    binary: Path,
    cargo_profile: str,
    backend_features: tuple[str, ...],
    rust_cpu: RustCpuPolicy,
) -> tuple[Path, Path]:
    """The stored backend generation and receipt a build of this checkout reuses.

    The store entry must hold this checkout's freshly computed backend identity,
    built under its observed Cargo environment, with the selected bytes.
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
    store = host_tool_store(project_root)
    identity = host_tool_identity(binary, fingerprint)
    # Renewed before the read, so no eviction races it (design 56, 2.8).
    store.record_use(identity)
    for admission in stored_host_tools(
        store,
        identity,
        binary,
        fingerprint,
        environment=build_admission.plan.environment,
    ):
        return admission.generation, admission.stored.receipt
    raise ValueError(
        f"no backend compiler is stored for this checkout's inputs "
        f"({identity} in {store.root}): {binary}"
    )


def _publish_backend_generation_from(
    coordinate: Path, source: Path, *, label: str
) -> StableRegularFileIdentity:
    """Publish an executable's bytes as a generation of ``coordinate``.

    A generation is the exact source bytes: a copy keeps the linker signature
    valid, and re-signing would only rewrite the bytes (and the compiler
    identity) on every build.
    """
    with stable_executable_probe(source, label=label) as (_entrypoint, identity):
        return publish_backend_generation(coordinate, identity.path, observed=identity)


def _stored_receipt_identity(fingerprint: Mapping[str, Any]) -> dict[str, Any]:
    """The identity a store receipt names: the product's, never a checkout's.

    Source timestamps and clean-head states are one checkout's fast-path
    metadata; they live in that checkout's identity memo, not in an entry
    every checkout reads.
    """
    return {
        key: value
        for key, value in _artifact_semantic_identity(fingerprint).items()
        if value is not None
    }


def _select_backend_generation(
    receipt: Path,
    generation: StableRegularFileIdentity,
    *,
    fingerprint: Mapping[str, Any],
    cargo_environment: CargoEnvironmentInputs,
) -> None:
    """Write a store variant's ``receipt``, which selects ``generation``.

    The receipt names the product's source identity, the observed Cargo
    environment and the generation's bytes. A variant's receipt is written
    once: it appears after its generation and probe evidence, and nothing
    rewrites it while readers may hold what it selects.
    """
    verify_stable_regular_file_identity(generation, label="backend selection")
    content_identity = artifact_content_identity(generation.path, observed=generation)
    payload = _fingerprint_payload(
        {
            **_stored_receipt_identity(fingerprint),
            "artifact_content_identity": content_identity,
            "cargo_environment": cargo_environment.to_dict(),
        }
    )
    # Exclusive: an existing receipt is never replaced.
    atomic_write_bytes(
        receipt,
        json.dumps(payload, indent=2, allow_nan=False).encode() + b"\n",
        exclusive=True,
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
    """Publish another host's backend into this host's store, through admission.

    The transported receipt must carry this checkout's freshly hashed backend
    identity and bind the executable's content, and its observed Cargo
    environment must match this one. The executable becomes the stored
    generation of that identity and environment, which a build then admits
    exactly as it admits one built on this host. A variant this host already
    holds is returned as it is: a published variant is never rewritten.
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
    store = host_tool_store(project_root)
    identity = host_tool_identity(binary, fingerprint)
    stored = stored_host_tool(store, identity, binary, cargo_environment)
    with store.single_flight(identity):
        build_admission.verify()
        for admission in stored_host_tools(
            store,
            identity,
            binary,
            fingerprint,
            environment=build_admission.plan.environment,
        ):
            if admission.stored == stored:
                # This host already holds the product; a published variant is
                # never rewritten, so the transported copy is not needed.
                return admission.generation
        _require_unpublished_host_tool(store, identity, stored)
        with stable_executable_probe(executable, label="transported backend") as (
            _entrypoint,
            observed,
        ):
            if (
                artifact_content_identity(observed.path, observed=observed)
                != transported["artifact_content_identity"]
            ):
                raise ValueError("transported backend differs from its receipt")
            generation = publish_backend_generation(
                stored.coordinate, observed.path, observed=observed
            )
        build_admission.verify()
        _select_backend_generation(
            stored.receipt,
            generation,
            fingerprint=fingerprint,
            cargo_environment=cargo_environment,
        )
        admitted = [
            admission
            for admission in stored_host_tools(
                store,
                identity,
                binary,
                fingerprint,
                environment=build_admission.plan.environment,
            )
            if admission.stored == stored
        ]
        if (
            not admitted
            or admitted[0].generation.resolve() != generation.path.resolve()
        ):
            raise ValueError("imported backend failed its own admission")
    store.retain(identity)
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


def _admit_stored_backend(
    backend_bin: Path,
    *,
    project_root: Path,
    cargo_profile: str,
    build_admission: BackendBuildAdmission,
    backend_features: tuple[str, ...],
    memo_path: Path,
    product: HostToolProduct = HostToolProduct.BACKEND,
) -> tuple[_BackendBinaryEnsureResult | None, dict[str, Any] | None]:
    """Admit a stored host tool by reads alone, or return None.

    The build lock serializes Cargo's shared output and publication, which
    only a build or a republication needs. Every check here binds content: the
    store receipt names the source identity and the selected generation's
    bytes, and the probe evidence names those bytes. Any failure to admit takes
    the locked path, so reusing one variant never waits out another build. A
    store entry that exists but does not verify fails loud instead.

    The source identity computed to key the store is returned beside the
    result, so a miss's locked path rechecks metadata instead of rehashing.
    """
    fingerprint: dict[str, Any] | None = None
    try:
        build_admission.verify()
        memo = _read_runtime_fingerprint(memo_path)
        fingerprint = _backend_fingerprint(
            project_root,
            cargo_profile=cargo_profile,
            build_admission=build_admission,
            backend_features=backend_features,
            stored_fingerprint=memo,
            product=product,
        )
        store = host_tool_store(project_root)
        identity = host_tool_identity(backend_bin, fingerprint)
        store.record_use(identity)
        admissions = stored_host_tools(
            store,
            identity,
            backend_bin,
            fingerprint,
            environment=build_admission.plan.environment,
        )
    except ProductStoreError as exc:
        return _backend_ensure_failure("product_store", str(exc)), fingerprint
    except (CompilerIdentityError, OSError, ValueError):
        return None, fingerprint
    for admission in admissions:
        if _backend_probe_validation_matches(
            admission.stored.probe_validation,
            binary_path=admission.generation,
            probe_target=_backend_probe_target(backend_features, product),
            backend_features=backend_features,
            fingerprint=admission.fingerprint,
        ):
            _remember_backend_identity(memo_path, memo, fingerprint)
            return _backend_ensure_success(
                coordinate=admission.stored.coordinate,
                binary_path=admission.generation,
                fingerprint=admission.fingerprint,
            ), fingerprint
    return None, fingerprint


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
    """Admit one stored host-tool generation, building and publishing it once.

    ``backend_bin`` is the variant's coordinate in this checkout's target: its
    name names the variant and its directory holds Cargo's output. Success
    names the immutable generation in the host's product store that the build
    runs (``molt.cli.product_store``); the coordinate itself is never an
    executable.
    """
    if not isinstance(product, HostToolProduct):
        return _backend_ensure_failure("host_tool_product", "Unknown host tool product")
    tool_label = product.label
    # Installed compilers are immutable release inputs, never Cargo outputs.
    # Admit before every developer store/rebuild path.
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
    memo_path = _backend_fingerprint_path(project_root, backend_bin, cargo_profile)
    stage_start = time.perf_counter()
    stored, unlocked_fingerprint = _admit_stored_backend(
        backend_bin,
        project_root=project_root,
        cargo_profile=cargo_profile,
        build_admission=build_admission,
        backend_features=backend_features,
        memo_path=memo_path,
        product=product,
    )
    if stored is not None:
        _record_backend_binary_stage_ms(
            stage_timings_ms, "backend_binary_published_admission", stage_start
        )
        return stored
    # Every feature lane publishes from the same Cargo output under this lock;
    # the store's identity lock then admits one builder per input on the host.
    with (
        _backend_admission_lock(project_root, backend_bin, cargo_profile),
        ExitStack() as held,
    ):
        try:
            build_admission.verify()
        except CompilerIdentityError as exc:
            return _backend_ensure_failure("backend_source_identity", str(exc))
        stage_start = time.perf_counter()
        memo = _read_runtime_fingerprint(memo_path)
        _record_backend_binary_stage_ms(
            stage_timings_ms,
            "backend_binary_read_fingerprint",
            stage_start,
        )
        stage_start = time.perf_counter()
        try:
            # The identity the store lookup just computed serves as the memo,
            # so the sources are rehashed only if their metadata moved since.
            fingerprint = _backend_fingerprint(
                project_root,
                cargo_profile=cargo_profile,
                build_admission=build_admission,
                backend_features=backend_features,
                stored_fingerprint=memo
                if unlocked_fingerprint is None
                else unlocked_fingerprint,
                product=product,
            )
            identity = host_tool_identity(backend_bin, fingerprint)
        except (OSError, ValueError) as exc:
            return _backend_ensure_failure("backend_source_identity", str(exc))
        _record_backend_binary_stage_ms(
            stage_timings_ms,
            "backend_binary_compute_fingerprint",
            stage_start,
        )
        store = host_tool_store(project_root)
        try:
            # A waiter waits while the build it waits for holds the lock:
            # Cargo, the capability probe and publication (design 56, 2.8).
            held.enter_context(store.single_flight(identity))
        except BuildLockAcquisitionError as exc:
            return _backend_ensure_failure("backend_build_lock", str(exc))
        cargo_output = backend_bin.parent / _host_tool_output_name(product)

        def _probe_backend_binary_support(
            probe_target: str,
            *,
            binary: Path,
            validation_path: Path,
            admitted: Mapping[str, Any],
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
                probe_cmd = [str(binary), JIT_RUNNER_PROBE_FLAG]
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
                        fingerprint=dict(admitted),
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
                    _atomic_write_json(validation_path, payload, indent=2)
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

        def _admitted(
            admission: StoredHostToolAdmission,
        ) -> _BackendBinaryEnsureResult:
            _remember_backend_identity(memo_path, memo, fingerprint)
            return _backend_ensure_success(
                coordinate=admission.stored.coordinate,
                binary_path=admission.generation,
                fingerprint=admission.fingerprint,
            )

        _quick_target = _backend_probe_target(backend_features, product)
        probe_failure: tuple[Path, _BackendBinaryEnsureResult] | None = None
        # Another checkout with these inputs may have published while this
        # build waited for the identity's lock: read the store again.
        stage_start = time.perf_counter()
        try:
            admissions = stored_host_tools(
                store,
                identity,
                backend_bin,
                fingerprint,
                environment=build_admission.plan.environment,
            )
        except ProductStoreError as exc:
            return _backend_ensure_failure("product_store", str(exc))
        except (OSError, ValueError) as exc:
            return _backend_ensure_failure("backend_receipt_identity", str(exc))
        _record_backend_binary_stage_ms(
            stage_timings_ms,
            "backend_binary_artifact_freshness",
            stage_start,
        )
        for admission in admissions:
            # Force a real compile-path probe. An empty stdin-only probe can
            # miss feature-lane poisoning because it never exercises output
            # emission for the requested target.
            stage_start = time.perf_counter()
            validated = _backend_probe_validation_matches(
                admission.stored.probe_validation,
                binary_path=admission.generation,
                probe_target=_quick_target,
                backend_features=backend_features,
                fingerprint=admission.fingerprint,
            )
            _record_backend_binary_stage_ms(
                stage_timings_ms,
                "backend_binary_probe_validation",
                stage_start,
            )
            if validated:
                return _admitted(admission)
            _probe_result = _probe_backend_binary_support(
                _quick_target,
                binary=admission.generation,
                validation_path=admission.stored.probe_validation,
                admitted=admission.fingerprint,
            )
            if _probe_result:
                return _admitted(admission)
            if _probe_result.phase in _PROBE_TERMINAL_PHASES:
                return _probe_result
            probe_failure = (admission.generation, _probe_result)
        if probe_failure is not None:
            # A published variant is never rebuilt over: the same inputs build
            # the same compiler, and other builds may hold this one.
            failed, result = probe_failure
            detail = (
                f"Stored {tool_label} {failed} failed its capability probe; "
                "rebuilding the same inputs cannot change it."
            )
            if result.detail:
                detail += f"\n{result.detail}"
            return _backend_ensure_failure(
                "backend_feature_probe",
                detail,
                returncode=result.returncode,
                command=result.command,
            )
        if reason := source_build_disabled_reason(
            product.value, environment=admission_environment
        ):
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
        stored = stored_host_tool(store, identity, backend_bin, reads.environment)
        try:
            _require_unpublished_host_tool(store, identity, stored)
        except ProductStoreError as exc:
            return _backend_ensure_failure("product_store", str(exc), command=cmd)
        try:
            rebuilt_generation = _publish_backend_generation_from(
                stored.coordinate, cargo_output, label="rebuilt Cargo backend output"
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
            validation_path=stored.probe_validation,
            admitted=fingerprint,
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
        try:
            _select_backend_generation(
                stored.receipt,
                rebuilt_generation,
                fingerprint=fingerprint,
                cargo_environment=reads.environment,
            )
            store.retain(identity)
        except (OSError, ValueError) as exc:
            return _backend_ensure_failure(
                "backend_artifact_publication",
                f"{tool_label} artifact provenance publication failed: {exc}",
                command=cmd,
            )
        _remember_backend_identity(memo_path, memo, fingerprint)
        return _backend_ensure_success(
            coordinate=stored.coordinate,
            binary_path=rebuilt_generation.path,
            fingerprint=fingerprint,
        )
