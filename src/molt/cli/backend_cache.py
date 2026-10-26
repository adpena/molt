from __future__ import annotations

from molt.compilation_target import Target, TargetKind

import contextlib
import hashlib
import json
import os
import time
import uuid
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Collection, Generator, Mapping, Sequence, cast

from molt.cache_eviction import refresh_cache_entry_recency
from molt.default_paths import (
    _default_molt_cache,
    is_backend_cache_artifact_name,
    shared_stdlib_archive_cache_root,
    validate_required_stdlib_output,
    backend_cache_artifact_filename,
    backend_cache_sync_receipt_path,
    backend_cache_publication_lock_name,
    shared_stdlib_cache_entry,
    shared_stdlib_publish_lock_path,
)

from molt.backend_protocol import BackendEmissionOrigin, BackendStdlibOrigin
from molt.backend_environment import compilation_diagnostics_requested
from molt.cli import function_references as _function_references
from molt.cli import native_symbol_inspection as _native_symbols
from molt.cli.artifact_sync import (
    _artifact_sync_emission_origin,
    _artifact_sync_state_matches,
    _artifact_sync_state_path,
    _read_artifact_sync_state,
    _write_artifact_sync_state,
)
from molt.cli.atomic_io import (
    _atomic_copy_file,
    _atomic_write_json,
    _link_failure_wants_copy,
)
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
    BackendArtifactValidationError,
)
from molt.cli.cache_fingerprints import _cache_fingerprint, _cache_tooling_fingerprint
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.cli.cache_keys import FunctionDigests, _cache_key, _sorted_ir_functions
from molt.cli.link_input_observation import CompilationCacheRecord
from molt.cli.models import (
    _ModuleGraphMetadata,
    _SharedStdlibCacheValidationToken,
)
from molt.cli.runtime_identity_schema import RUNTIME_ARTIFACT_METADATA_MAX_BYTES
from molt.file_locks import (
    _acquire_file_lock,
    _release_file_lock,
    _shared_cache_lock,
)
from molt.file_publication import staged_file_path
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    capture_stable_regular_file,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)

_emitted_name_matches_any_module_symbol = (
    _function_references.emitted_name_matches_any_module_symbol
)
_is_protected_runtime_entrypoint = _function_references.is_protected_runtime_entrypoint
_module_symbol_name = _function_references.module_symbol_name
reachable_function_names = _function_references.reachable_function_names

_SHARED_STDLIB_SYMBOL_CONTRACT_SCHEMA_VERSION = 2


def _record_backend_cache_stage_ms(
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


def _validate_backend_cache_artifact(
    path: Path,
    *,
    artifact_contract: BackendArtifactContract,
    identity: StableRegularFileIdentity | None = None,
    publish_symbol_facts: bool = False,
) -> StableRegularFileIdentity:
    # Native shape, member framing and nm consume one admitted owned handle.
    # Content-keyed facts are published only after its closing fences pass.
    # The artifact opener owns the identity-path check: it compares the
    # resolved path, the same path a captured identity records.
    try:
        if artifact_contract.is_native:
            with _native_symbols._native_symbol_facts_admission(
                path,
                target_triple=artifact_contract.target_triple,
                identity=identity,
                publish=publish_symbol_facts,
                validate_shape=lambda opened: artifact_contract.validate_native_shape(
                    opened.path, opened=opened
                ),
            ) as (_opened, identity, facts):
                if not (facts.defined or facts.undefined):
                    raise BackendArtifactValidationError(
                        f"Native application cache artifact has no symbol surface: {path}"
                    )
        else:
            # Non-native validators may invoke external WASM tools; retain the
            # same admitted handle across their path-based read and close fence.
            with _native_symbols._open_native_symbol_artifact(path, identity) as (
                _opened,
                identity,
            ):
                artifact_contract.validate(path)
        return identity
    except _native_symbols.NativeSymbolArtifactError as error:
        raise BackendArtifactValidationError(str(error)) from error
    except _native_symbols.NativeSymbolInspectionError:
        # Reader failures are operational failures, never soft cache misses.
        raise
    except BackendArtifactValidationError:
        raise
    except (OSError, ValueError) as error:
        raise BackendArtifactValidationError(str(error)) from error


def _is_valid_cached_backend_artifact(
    path: Path, *, artifact_contract: BackendArtifactContract
) -> bool:
    try:
        _validate_backend_cache_artifact(path, artifact_contract=artifact_contract)
    except BackendArtifactValidationError:
        return False
    return True


def _native_object_has_unresolved_module_chunks(
    candidate: Path,
    stdlib_object_path: Path | None,
    *,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
) -> bool:
    candidate_symbols = _native_symbols._native_object_global_symbol_sets(
        candidate, target_triple=target_triple, identity=identity
    )
    defined, undefined = candidate_symbols
    # Compiler archives whole-load all members. A reference and its provider
    # can appear in different member symbol tables without being unresolved.
    unresolved_chunks = {
        symbol for symbol in undefined - defined if "__molt_module_chunk_" in symbol
    }
    if not unresolved_chunks:
        return False
    stdlib_defined: set[str] = set()
    if stdlib_object_path is not None:
        stdlib_symbols = _native_symbols._native_object_global_symbol_sets(
            stdlib_object_path, target_triple=target_triple
        )
        stdlib_defined, _ = stdlib_symbols
    return any(symbol not in stdlib_defined for symbol in unresolved_chunks)


def _read_shared_stdlib_partition_facts(
    stdlib_object_path: Path,
) -> tuple[StableRegularFileIdentity, frozenset[str]] | None:
    try:
        identity, raw = capture_stable_regular_file(
            _stdlib_object_partition_manifest_sidecar_path(stdlib_object_path),
            label="shared stdlib partition manifest",
            max_bytes=RUNTIME_ARTIFACT_METADATA_MAX_BYTES,
        )
    except (OSError, UnicodeError, ValueError):
        return None
    try:
        payload = json.loads(raw)
    except (ValueError, RecursionError):
        return None
    if not isinstance(payload, dict):
        return None
    if payload.get("schema") != _SHARED_STDLIB_PARTITION_SCHEMA_VERSION:
        return None
    raw_functions = payload.get("functions")
    if not isinstance(raw_functions, list) or not all(
        isinstance(name, str) and name for name in raw_functions
    ):
        return None
    functions = cast(list[str], raw_functions)
    function_count = payload.get("function_count")
    if isinstance(function_count, int) and function_count != len(functions):
        return None
    return identity, frozenset(functions)


def _read_shared_stdlib_partition_functions(
    stdlib_object_path: Path,
) -> frozenset[str] | None:
    facts = _read_shared_stdlib_partition_facts(stdlib_object_path)
    return None if facts is None else facts[1]


def _unresolved_stdlib_module_symbols(
    undefined_symbols: Collection[str],
    stdlib_module_symbols: Collection[str],
) -> tuple[str, ...]:
    module_symbols = tuple(sorted(set(stdlib_module_symbols)))
    if not module_symbols:
        return ()
    unresolved: list[str] = []
    owners = frozenset(module_symbols)
    for symbol in sorted(set(undefined_symbols)):
        if symbol.startswith("molt_"):
            continue
        if _emitted_name_matches_any_module_symbol(symbol, owners):
            unresolved.append(symbol)
    return tuple(unresolved)


def _shared_stdlib_native_symbol_closure_issue(
    stdlib_object_path: Path,
    *,
    stdlib_module_symbols: Collection[str] | None,
    target_triple: str | None = None,
    symbol_sets: _native_symbols._NativeObjectSymbolSets | None = None,
    partition_functions: frozenset[str] | None = None,
) -> str | None:
    if symbol_sets is None:
        symbol_sets = _native_symbols._native_object_global_symbol_sets(
            stdlib_object_path, target_triple=target_triple
        )
    defined, undefined = symbol_sets
    # All members of a compiler archive are included by the final link plan.
    # References between those members are resolved within this artifact, even
    # though nm reports both their provider and consumer rows independently.
    undefined = undefined - defined
    issues: list[str] = []

    if partition_functions is None:
        partition_functions = _read_shared_stdlib_partition_functions(
            stdlib_object_path
        )
    if partition_functions is None:
        issues.append("missing or malformed partition manifest")
    else:
        missing_definitions = sorted(partition_functions - defined)
        if missing_definitions:
            preview = ", ".join(missing_definitions[:8])
            suffix = "" if len(missing_definitions) <= 8 else ", ..."
            issues.append(f"missing partition definitions: {preview}{suffix}")
        unresolved_declared = sorted(partition_functions & undefined)
        if unresolved_declared:
            preview = ", ".join(unresolved_declared[:8])
            suffix = "" if len(unresolved_declared) <= 8 else ", ..."
            issues.append(f"unresolved partition references: {preview}{suffix}")

    if stdlib_module_symbols is not None:
        unresolved_stdlib = _unresolved_stdlib_module_symbols(
            undefined, stdlib_module_symbols
        )
        if unresolved_stdlib:
            preview = ", ".join(unresolved_stdlib[:8])
            suffix = "" if len(unresolved_stdlib) <= 8 else ", ..."
            issues.append(f"unresolved stdlib module references: {preview}{suffix}")

    return "; ".join(issues) if issues else None


def _copy_verified_backend_artifact(
    src: Path, dst: Path, *, identity: StableRegularFileIdentity
) -> StableRegularFileIdentity:
    """Publish attested bytes without aliasing mutable output to immutable cache."""
    try:
        # Identities from the artifact opener record the resolved path; compare
        # the file each path names, so a symlinked stage directory still matches.
        if identity.path.resolve(strict=True) != src.resolve(strict=True):
            raise ValueError("Backend copy identity belongs to another source path")
        verify_stable_regular_file_identity(identity, label="backend copy source")
        _atomic_copy_file(src, dst, expected_sha256=identity.sha256)
        copied = stable_regular_file_identity(dst, label="backend copied artifact")
        if copied.sha256 != identity.sha256 or copied.size != identity.size:
            raise ValueError("Backend destination changed after verified publication")
        return copied
    except (OSError, ValueError) as error:
        raise BackendArtifactValidationError(
            f"Cannot publish attested backend artifact {src} to {dst}: {error}"
        ) from error


def _require_matching_backend_content(
    expected: StableRegularFileIdentity, actual: StableRegularFileIdentity
) -> None:
    if actual.sha256 != expected.sha256 or actual.size != expected.size:
        raise BackendArtifactValidationError(
            f"Backend cache key has conflicting content: {actual.path}; "
            f"expected {expected.sha256}, found {actual.sha256}"
        )


@dataclass(frozen=True, slots=True)
class _BackendCacheGeneration:
    identity: StableRegularFileIdentity
    emission_origin: BackendEmissionOrigin | None


@dataclass(frozen=True, slots=True)
class _BackendArtifactStageResult:
    error: str | None
    emission_origin: BackendEmissionOrigin | None


def _backend_cache_generation(
    artifact: Path, *, identity: StableRegularFileIdentity
) -> _BackendCacheGeneration:
    # Caller owns the publication lock, including the byte/receipt read fence.
    state = _read_artifact_sync_state(backend_cache_sync_receipt_path(artifact))
    try:
        verify_stable_regular_file_identity(identity, label="backend cache generation")
    except ValueError as error:
        raise BackendArtifactValidationError(str(error)) from error
    origin = (
        _artifact_sync_emission_origin(state, identity=identity)
        if state is not None
        and state.get("source_key") == artifact.name
        and state.get("tier") == "immutable"
        else None
    )
    return _BackendCacheGeneration(identity, origin)


def _publish_immutable_backend_cache_artifact(
    src: Path,
    dst: Path,
    *,
    artifact_contract: BackendArtifactContract,
    warnings: list[str],
    identity: StableRegularFileIdentity | None = None,
    emission_origin: BackendEmissionOrigin | None = None,
) -> _BackendCacheGeneration:
    """Publish bytes and their optional origin under one generation lock.

    An existing winner always retains its own origin, including unknown legacy
    origin. Byte equality does not authorize grafting a new producer onto it.
    """
    identity = _validate_backend_cache_artifact(
        src, artifact_contract=artifact_contract, identity=identity
    )
    if emission_origin is not None and (
        emission_origin.output_sha256 != identity.sha256
        or emission_origin.output_size != identity.size
    ):
        raise BackendArtifactValidationError(
            "Backend cache origin names different bytes"
        )
    dst.parent.mkdir(parents=True, exist_ok=True)
    with _shared_cache_lock(
        backend_cache_publication_lock_name(dst), cache_root=dst.parent
    ):
        if dst.exists():
            try:
                existing = _validate_backend_cache_artifact(
                    dst, artifact_contract=artifact_contract, publish_symbol_facts=True
                )
            except BackendArtifactValidationError:
                warnings.append(
                    f"Ignoring invalid immutable cache artifact; cleanup owns removal: {dst}"
                )
                return _BackendCacheGeneration(identity, emission_origin)
            _require_matching_backend_content(identity, existing)
            return _backend_cache_generation(dst, identity=existing)
        # A retired artifact can leave an orphan receipt. It never belongs to
        # this new generation, even if a filesystem later reuses its inode.
        backend_cache_sync_receipt_path(dst).unlink(missing_ok=True)
        tmp_path = staged_file_path(dst, purpose="backend-cache")
        try:
            _copy_verified_backend_artifact(src, tmp_path, identity=identity)
            try:
                os.link(tmp_path, dst)
            except FileExistsError:
                existing = _validate_backend_cache_artifact(
                    dst, artifact_contract=artifact_contract, publish_symbol_facts=True
                )
                _require_matching_backend_content(identity, existing)
                return _backend_cache_generation(dst, identity=existing)
            except OSError as exc:
                if not _link_failure_wants_copy(exc):
                    raise
                os.replace(tmp_path, dst)
        finally:
            try:
                tmp_path.unlink(missing_ok=True)
            except OSError as error:
                warnings.append(
                    f"Backend cache publication temporary cleanup failed: {error}"
                )
        # Unlinking the temporary hard link changes ctime. Capture only after it.
        published = _validate_backend_cache_artifact(
            dst, artifact_contract=artifact_contract, publish_symbol_facts=True
        )
        _require_matching_backend_content(identity, published)
        if emission_origin is not None:
            _write_artifact_sync_state(
                backend_cache_sync_receipt_path(dst),
                source_key=dst.name,
                tier="immutable",
                artifact=dst,
                identity=published,
                emission_origin=emission_origin,
            )
        return _BackendCacheGeneration(published, emission_origin)


def restore_backend_cache_artifact(
    source: Path,
    destination: Path,
    *,
    artifact_contract: BackendArtifactContract,
) -> None:
    """Republish a sealed cache snapshot, preserving its original observation.

    This explicit transfer boundary consumes a receiver-inventoried immutable
    snapshot, whose inode cannot survive export. Ordinary cache admission still
    requires the local generation fence. Neither boundary authenticates an
    observation: the closed original supervisor session owns that decision.
    """
    if (
        not is_backend_cache_artifact_name(source.name)
        or destination.name != source.name
    ):
        raise BackendArtifactValidationError(
            "Cache transfer requires the same canonical artifact name"
        )
    identity = _validate_backend_cache_artifact(
        source, artifact_contract=artifact_contract
    )
    receipt = backend_cache_sync_receipt_path(source)
    origin = None
    if receipt.exists():
        state = _read_artifact_sync_state(receipt)
        if (
            state is None
            or state["source_key"] != source.name
            or state["tier"] != "immutable"
            or (state["sha256"], state["size"]) != (identity.sha256, identity.size)
        ):
            raise BackendArtifactValidationError(
                "Cache transfer receipt does not bind snapshot bytes"
            )
        if state["emission_origin"] is not None:
            origin = BackendEmissionOrigin.from_payload(state["emission_origin"])
            origin.verify_output(source)
    warnings: list[str] = []
    published = _publish_immutable_backend_cache_artifact(
        source,
        destination,
        artifact_contract=artifact_contract,
        warnings=warnings,
        identity=identity,
        emission_origin=origin,
    )
    if warnings or published.identity.path != destination.expanduser().absolute():
        raise BackendArtifactValidationError(
            "Cache transfer did not publish the requested destination"
        )
    if published.emission_origin != origin:
        raise BackendArtifactValidationError(
            "Cache transfer destination already owns another emission observation"
        )


@contextmanager
def compilation_cache_export_paths(
    cache_root: Path,
    *,
    cache: CompilationCacheRecord,
    compiler_origin: BackendEmissionOrigin,
    stdlib_origin: BackendStdlibOrigin | None,
    artifact_contract: BackendArtifactContract,
) -> Generator[tuple[Path, ...], None, None]:
    """Pin the producer's exact consumed cache coordinates for admission export.

    Coordinates are selection hints, independently fenced against the actual
    generation and output. No directory enumeration or unrelated-byte hashing
    occurs. The caller copies this bounded family while these locks remain held.
    """
    if cache["compiler"] is None:
        raise ValueError("consumed compiler output has no retained cache coordinate")
    root = cache_root.resolve(strict=True)
    artifact = Path(cache["compiler"])
    if (
        artifact.parent != root
        or not is_backend_cache_artifact_name(artifact.name)
        or not artifact.name.endswith(artifact_contract.suffix)
        or f".artifact-{artifact_contract.cache_identity}" not in artifact.name
        or artifact.resolve(strict=True) != artifact
    ):
        raise ValueError("compiler cache coordinate is outside its canonical owner")
    selected = [artifact, backend_cache_sync_receipt_path(artifact)]
    with contextlib.ExitStack() as locks:
        locks.enter_context(
            _shared_cache_lock(
                backend_cache_publication_lock_name(artifact), cache_root=root
            )
        )
        identity = _validate_backend_cache_artifact(
            artifact, artifact_contract=artifact_contract
        )
        generation = _backend_cache_generation(artifact, identity=identity)
        if generation.emission_origin != compiler_origin:
            raise ValueError("compiler cache coordinate owns another emission")
        compiler_origin.verify_output(artifact)
        if stdlib_origin is None:
            if cache["stdlib"] is not None or ".stdlib-" in artifact.name:
                raise ValueError("unused stdlib cache coordinate cannot enter export")
        else:
            if cache["stdlib"] is None:
                raise ValueError(
                    "consumed stdlib archive has no retained cache coordinate"
                )
            archive = Path(cache["stdlib"])
            if (
                shared_stdlib_archive_cache_root(archive) != root
                or archive.resolve(strict=True) != archive
                or not artifact.name.endswith(
                    f".stdlib-{archive.parent.name}{artifact_contract.suffix}"
                )
            ):
                raise ValueError(
                    "stdlib cache coordinate is outside its canonical owner"
                )
            locks.enter_context(_shared_stdlib_cache_lock(archive))
            _, data = capture_stable_regular_file(
                _stdlib_object_origin_sidecar_path(archive),
                label="stdlib cache export origin",
                max_bytes=64 * 1024 * 1024,
            )
            from molt.exact_json import loads_exact

            original = BackendStdlibOrigin.from_payload(
                loads_exact(data.decode("utf-8"))
            )
            if original != stdlib_origin:
                raise ValueError("stdlib cache coordinate owns another emission")
            original.verify_output(archive)
            selected.append(archive)
            for sidecar in (
                _stdlib_object_count_sidecar_path,
                _stdlib_object_key_sidecar_path,
                _stdlib_object_manifest_sidecar_path,
                _stdlib_object_partition_manifest_sidecar_path,
                _stdlib_object_digest_sidecar_path,
                _stdlib_object_symbol_contract_sidecar_path,
                _stdlib_object_origin_sidecar_path,
                _native_symbols._native_object_symbol_facts_sidecar_path,
            ):
                path = sidecar(archive)
                if path.is_file() and not path.is_symlink():
                    selected.append(path)
        yield tuple(selected)


def _materialize_cached_backend_artifact(
    project_root: Path,
    candidate: Path,
    output_artifact: Path,
    *,
    tier: str,
    source_key: str,
    cache_path: Path | None,
    module_cache_key: str | None = None,
    warnings: list[str],
    state_path: Path | None = None,
    state: dict[str, Any] | None = None,
    output_stat: os.stat_result | None = None,
    artifact_contract: BackendArtifactContract,
    stdlib_object_path: Path | None,
    require_origin: bool = False,
) -> _SyncedBackendOutput | None:
    try:
        observed = candidate.lstat()
        with _shared_cache_lock(
            backend_cache_publication_lock_name(candidate), cache_root=candidate.parent
        ):
            candidate_identity = _validate_backend_cache_artifact(
                candidate,
                artifact_contract=artifact_contract,
            )
            # Eligibility belongs to this admitted generation, before origin
            # requirements, synchronized-output reuse or any publication.
            try:
                unresolved_chunks = (
                    artifact_contract.is_native
                    and _native_object_has_unresolved_module_chunks(
                        candidate,
                        stdlib_object_path,
                        target_triple=artifact_contract.target_triple,
                        identity=candidate_identity,
                    )
                )
            except _native_symbols.NativeSymbolArtifactError:
                # A changed optional artifact is a miss; reader failures remain
                # hard errors in the enclosing handler below.
                return None
            if unresolved_chunks:
                warnings.append(
                    "Ignoring native cache artifact with unresolved user module chunks: "
                    f"{candidate}"
                )
                return None
            generation = _backend_cache_generation(
                candidate, identity=candidate_identity
            )
        emission_origin = generation.emission_origin
    except _native_symbols.NativeSymbolInspectionError:
        # Reader failures are operational failures, never soft cache misses.
        raise
    except (OSError, BackendArtifactValidationError) as exc:
        warnings.append(f"Cache candidate admission failed: {exc}")
        return None
    if require_origin and emission_origin is None:
        raise BackendArtifactValidationError(
            "Captured backend cache reuse requires original emission origin; "
            f"the cache generation contains bytes only: {candidate}"
        )
    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
        state = _read_artifact_sync_state(state_path)
    if output_stat is None:
        with contextlib.suppress(OSError):
            output_stat = output_artifact.stat()
    if output_stat is not None:
        synced_identity = _backend_artifact_sync_identity(
            state,
            source_key=source_key,
            tier=tier,
            artifact=output_artifact,
            artifact_contract=artifact_contract,
        )
        if synced_identity is not None:
            try:
                _require_matching_backend_content(candidate_identity, synced_identity)
            except BackendArtifactValidationError as exc:
                warnings.append(str(exc))
                return None
            if (
                candidate.parent == _default_molt_cache()
                and is_backend_cache_artifact_name(candidate.name)
            ):
                refresh_cache_entry_recency(
                    candidate, cache_root=candidate.parent, observed=observed
                )
            return _SyncedBackendOutput(
                tier,
                synced_identity,
                _artifact_sync_emission_origin(state, identity=synced_identity),
            )
    sync_tier = tier
    sync_source_key = source_key
    try:
        output_identity = _copy_verified_backend_artifact(
            candidate, output_artifact, identity=candidate_identity
        )
        if tier == "function" and cache_path is not None and candidate != cache_path:
            try:
                published_module_cache = _publish_immutable_backend_cache_artifact(
                    candidate,
                    cache_path,
                    artifact_contract=artifact_contract,
                    warnings=warnings,
                    identity=candidate_identity,
                    emission_origin=emission_origin,
                )
                if (
                    module_cache_key
                    and published_module_cache.identity.path
                    == cache_path.expanduser().absolute()
                ):
                    # Once the canonical module cache path is valid, future
                    # daemon sync checks should treat output.o as module-synced
                    # rather than function-only.
                    emission_origin = published_module_cache.emission_origin
                    sync_tier = "module"
                    sync_source_key = module_cache_key
            except OSError as exc:
                warnings.append(f"Module cache promotion failed: {exc}")
        try:
            state_path.parent.mkdir(parents=True, exist_ok=True)
            _write_artifact_sync_state(
                state_path,
                source_key=sync_source_key,
                tier=sync_tier,
                artifact=output_artifact,
                identity=output_identity,
                emission_origin=emission_origin,
            )
        except OSError as exc:
            warnings.append(f"Backend cache sync receipt write failed: {exc}")
        if candidate.parent == _default_molt_cache() and is_backend_cache_artifact_name(
            candidate.name
        ):
            refresh_cache_entry_recency(
                candidate, cache_root=candidate.parent, observed=observed
            )
        return _SyncedBackendOutput(tier, output_identity, emission_origin)
    except OSError as exc:
        warnings.append(f"Cache copy failed: {exc}")
        return None


def _backend_artifact_sync_identity(
    state: dict[str, Any] | None,
    *,
    source_key: str,
    tier: str,
    artifact: Path,
    artifact_contract: BackendArtifactContract,
) -> StableRegularFileIdentity | None:
    # Reject nonmatching source/tier before any byte inspection. A receipt owns
    # exactly one tier, so checking module and function reuse never hashes twice.
    if (
        not source_key
        or state is None
        or state.get("source_key") != source_key
        or state.get("tier") != tier
    ):
        return None
    try:
        identity = _validate_backend_cache_artifact(
            artifact, artifact_contract=artifact_contract
        )
    except BackendArtifactValidationError:
        return None
    if not _artifact_sync_state_matches(
        state,
        source_key=source_key,
        tier=tier,
        artifact=artifact,
        identity=identity,
    ):
        return None
    return identity


@dataclass(frozen=True, slots=True)
class _SyncedBackendOutput:
    tier: str
    identity: StableRegularFileIdentity
    emission_origin: BackendEmissionOrigin | None


def _synced_backend_output_cache_hit(
    state: dict[str, Any] | None,
    output_artifact: Path,
    output_stat: os.stat_result | None,
    *,
    artifact_contract: BackendArtifactContract,
    cache_key: str | None,
    function_cache_key: str | None,
    stdlib_object_cache_key: str | None,
    stdlib_object_path: Path | None = None,
) -> _SyncedBackendOutput | None:
    if output_stat is None:
        return None
    for tier, key in (("module", cache_key), ("function", function_cache_key)):
        identity = _backend_artifact_sync_identity(
            state,
            source_key=_backend_artifact_source_key(
                key,
                stdlib_object_cache_key=stdlib_object_cache_key,
                artifact_contract=artifact_contract,
            ),
            tier=tier,
            artifact=output_artifact,
            artifact_contract=artifact_contract,
        )
        if identity is None:
            continue
        if artifact_contract.is_native and _native_object_has_unresolved_module_chunks(
            output_artifact,
            stdlib_object_path,
            target_triple=artifact_contract.target_triple,
            identity=identity,
        ):
            return None
        return _SyncedBackendOutput(
            tier, identity, _artifact_sync_emission_origin(state, identity=identity)
        )
    return None


def _validated_stdlib_contract_token_for_backend_cache_hit(
    *,
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None,
    stdlib_contract_validation_token: _SharedStdlibCacheValidationToken | None,
    stage_timings_ms: dict[str, float] | None,
    target_triple: str | None = None,
) -> tuple[bool, _SharedStdlibCacheValidationToken | None]:
    if stdlib_object_path is None:
        return True, None
    stage_start = time.perf_counter()
    active_stdlib_contract_token = _shared_stdlib_cache_validation_token(
        stdlib_object_path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        target_triple=target_triple,
        previous_token=stdlib_contract_validation_token,
    )
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_try_contract_validate",
        stage_start,
    )
    return active_stdlib_contract_token is not None, active_stdlib_contract_token


def _backend_artifact_source_key(
    base_key: str | None,
    *,
    stdlib_object_cache_key: str | None,
    artifact_contract: BackendArtifactContract,
) -> str:
    artifact_contract.validate_shared_stdlib(enabled=bool(stdlib_object_cache_key))
    if not base_key:
        # Cache disabled (e.g. --rebuild): return empty key so the daemon
        # does not match against a shared sentinel that is identical for
        # every file.  Previously `base_key or ""` produced the same
        # "|stdlib:<hash>" key for every --rebuild invocation, causing the
        # daemon in-memory cache to return the first file's compiled output
        # for all subsequent files in the same daemon session.
        return ""
    key = f"{base_key}|artifact:{artifact_contract.cache_identity}"
    if not stdlib_object_cache_key:
        return key
    return f"{key}|stdlib:{stdlib_object_cache_key}"


def _backend_cache_artifact_path(
    cache_root: Path,
    base_key: str | None,
    *,
    stdlib_object_cache_key: str | None,
    artifact_contract: BackendArtifactContract,
) -> Path | None:
    source_key = _backend_artifact_source_key(
        base_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
        artifact_contract=artifact_contract,
    )
    if not source_key:
        return None
    return cache_root / backend_cache_artifact_filename(
        source_key, artifact_contract.suffix
    )


def _try_cached_backend_candidates(
    *,
    project_root: Path,
    cache_candidates: Sequence[tuple[str, Path]],
    output_artifact: Path,
    artifact_contract: BackendArtifactContract,
    cache_key: str | None,
    function_cache_key: str | None,
    cache_path: Path | None,
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    warnings: list[str],
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    stdlib_contract_validation_token: _SharedStdlibCacheValidationToken | None = None,
    stage_timings_ms: dict[str, float] | None = None,
    require_origin: bool = False,
) -> _SyncedBackendOutput | None:
    artifact_contract.validate_shared_stdlib(
        enabled=stdlib_object_path is not None or bool(stdlib_object_cache_key)
    )
    # Both synchronized outputs and stored candidates bypass backend execution.
    if compilation_diagnostics_requested():
        return None
    stage_start = time.perf_counter()
    state_path = _artifact_sync_state_path(project_root, output_artifact)
    state = _read_artifact_sync_state(state_path)
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_try_sync_state",
        stage_start,
    )
    stage_start = time.perf_counter()
    try:
        output_stat: os.stat_result | None = output_artifact.stat()
    except OSError:
        output_stat = None
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_try_output_stat",
        stage_start,
    )
    stdlib_contract_valid, _active_stdlib_contract_token = (
        _validated_stdlib_contract_token_for_backend_cache_hit(
            stdlib_object_path=stdlib_object_path,
            stdlib_object_cache_key=stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            stdlib_contract_validation_token=stdlib_contract_validation_token,
            stage_timings_ms=stage_timings_ms,
            target_triple=artifact_contract.target_triple,
        )
    )
    del _active_stdlib_contract_token
    if not stdlib_contract_valid:
        if stdlib_object_path is not None and stdlib_object_path.exists():
            warnings.append(
                "Ignoring shared stdlib cache with mismatched contract: "
                + _shared_stdlib_cache_mismatch_detail(
                    stdlib_object_path,
                    stdlib_object_cache_key,
                    stdlib_object_manifest=stdlib_object_manifest,
                    stdlib_module_symbols=stdlib_module_symbols,
                    target_triple=artifact_contract.target_triple,
                )
            )
        # Native application archive cache hits are invalid without the matching
        # stdlib_shared object they were compiled against.
        return None

    stage_start = time.perf_counter()
    synced = _synced_backend_output_cache_hit(
        state,
        output_artifact,
        output_stat,
        artifact_contract=artifact_contract,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_path=stdlib_object_path,
    )
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_try_synced_output",
        stage_start,
    )
    if synced is not None:
        if require_origin and synced.emission_origin is None:
            raise BackendArtifactValidationError(
                "Captured backend cache reuse requires original emission origin; "
                "the synchronized output contains bytes only"
            )
        return synced

    for tier, candidate in cache_candidates:
        stage_start = time.perf_counter()
        if not candidate.exists():
            _record_backend_cache_stage_ms(
                stage_timings_ms,
                "backend_cache_try_candidate_exists",
                stage_start,
            )
            continue
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_try_candidate_exists",
            stage_start,
        )
        stage_start = time.perf_counter()
        materialized = _materialize_cached_backend_artifact(
            project_root,
            candidate,
            output_artifact,
            tier=tier,
            source_key=_backend_artifact_source_key(
                cache_key
                if tier == "module"
                else (function_cache_key or cache_key or ""),
                stdlib_object_cache_key=stdlib_object_cache_key,
                artifact_contract=artifact_contract,
            ),
            cache_path=cache_path,
            module_cache_key=_backend_artifact_source_key(
                cache_key,
                stdlib_object_cache_key=stdlib_object_cache_key,
                artifact_contract=artifact_contract,
            ),
            warnings=warnings,
            state_path=state_path,
            state=state,
            output_stat=output_stat,
            artifact_contract=artifact_contract,
            stdlib_object_path=stdlib_object_path,
            require_origin=require_origin,
        )
        if materialized is not None:
            if require_origin and materialized.emission_origin is None:
                raise BackendArtifactValidationError(
                    "Captured backend cache reuse requires original emission origin; "
                    f"the cache generation contains bytes only: {candidate}"
                )
            _record_backend_cache_stage_ms(
                stage_timings_ms,
                "backend_cache_try_materialize",
                stage_start,
            )
            return materialized
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_try_materialize",
            stage_start,
        )
    return None


def _backend_daemon_skip_output_sync_flags(
    project_root: Path,
    output_artifact: Path,
    *,
    cache_key: str | None,
    function_cache_key: str | None,
    stdlib_object_path: Path | None = None,
    stdlib_object_cache_key: str | None = None,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    state_path: Path | None = None,
    state: dict[str, Any] | None = None,
    output_stat: os.stat_result | None = None,
    artifact_contract: BackendArtifactContract,
) -> tuple[bool, bool]:
    artifact_contract.validate_shared_stdlib(
        enabled=stdlib_object_path is not None or bool(stdlib_object_cache_key)
    )
    if output_stat is None:
        try:
            output_stat = output_artifact.stat()
        except OSError:
            return False, False
    if stdlib_object_path is not None and not _shared_stdlib_cache_matches_key_locked(
        stdlib_object_path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        target_triple=artifact_contract.target_triple,
    ):
        return False, False
    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
        state = _read_artifact_sync_state(state_path)
    synced = _synced_backend_output_cache_hit(
        state,
        output_artifact,
        output_stat,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_path=stdlib_object_path,
        artifact_contract=artifact_contract,
    )
    if synced is None:
        return False, False
    return synced.tier == "module", synced.tier == "function"


@contextmanager
def _temporary_backend_output_path(
    artifacts_root: Path,
    *,
    artifact_contract: BackendArtifactContract,
) -> Generator[Path]:
    suffix = artifact_contract.suffix
    artifacts_root.mkdir(parents=True, exist_ok=True)
    path = artifacts_root / f"backend_{os.getpid()}_{uuid.uuid4().hex}{suffix}"
    try:
        yield path
    finally:
        with contextlib.suppress(OSError):
            path.unlink()


def _stage_backend_output_and_caches(
    project_root: Path,
    backend_output: Path,
    output_artifact: Path,
    *,
    cache_path: Path | None,
    cache_key: str | None,
    stdlib_object_cache_key: str | None,
    function_cache_path: Path | None,
    warnings: list[str],
    output_already_synced: bool | None = None,
    state_path: Path | None = None,
    state: dict[str, Any] | None = None,
    output_stat: os.stat_result | None = None,
    artifact_contract: BackendArtifactContract,
    emission_origin: BackendEmissionOrigin | None = None,
) -> _BackendArtifactStageResult:
    artifact_contract.validate_shared_stdlib(enabled=bool(stdlib_object_cache_key))
    if any(
        destination is not None
        and backend_output.expanduser().absolute()
        == destination.expanduser().absolute()
        for destination in (cache_path, function_cache_path)
    ):
        return _BackendArtifactStageResult(
            "Backend output must be build-owned, not an evictable cache destination",
            None,
        )
    try:
        staged_identity = _validate_backend_cache_artifact(
            backend_output, artifact_contract=artifact_contract
        )
        if emission_origin is not None and (
            emission_origin.output_sha256 != staged_identity.sha256
            or emission_origin.output_size != staged_identity.size
        ):
            raise BackendArtifactValidationError(
                "Backend emission origin names different staged output bytes"
            )
        if output_artifact.parent != Path("."):
            output_artifact.parent.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        return _BackendArtifactStageResult(
            f"Failed to move backend output: {exc}", None
        )

    staged_source = backend_output
    if cache_path is not None:
        try:
            published = _publish_immutable_backend_cache_artifact(
                backend_output,
                cache_path,
                artifact_contract=artifact_contract,
                warnings=warnings,
                identity=staged_identity,
                emission_origin=emission_origin,
            )
            emission_origin = published.emission_origin
        except OSError as exc:
            return _BackendArtifactStageResult(
                f"Failed to publish backend cache output: {exc}", None
            )

    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
    if output_already_synced is not False:
        if state is None:
            state = _read_artifact_sync_state(state_path)
        if output_stat is None:
            try:
                output_stat = output_artifact.stat()
            except OSError:
                output_stat = None
        synced_identity = (
            _backend_artifact_sync_identity(
                state,
                source_key=_backend_artifact_source_key(
                    cache_key,
                    stdlib_object_cache_key=stdlib_object_cache_key,
                    artifact_contract=artifact_contract,
                ),
                tier="module",
                artifact=output_artifact,
                artifact_contract=artifact_contract,
            )
            if cache_key and output_stat is not None
            else None
        )
        output_already_synced = synced_identity is not None
        if synced_identity is not None:
            try:
                _require_matching_backend_content(staged_identity, synced_identity)
            except BackendArtifactValidationError as exc:
                return _BackendArtifactStageResult(str(exc), None)
    try:
        if output_already_synced:
            assert synced_identity is not None
            emission_origin = _artifact_sync_emission_origin(
                state, identity=synced_identity
            )
        elif staged_source == output_artifact:
            verify_stable_regular_file_identity(
                staged_identity, label="backend staged output"
            )
            output_identity = staged_identity
        else:
            output_identity = _copy_verified_backend_artifact(
                staged_source, output_artifact, identity=staged_identity
            )
    except (OSError, ValueError) as exc:
        return _BackendArtifactStageResult(
            f"Failed to move backend output: {exc}", None
        )

    if cache_path is not None:
        if function_cache_path is not None and function_cache_path != cache_path:
            try:
                _publish_immutable_backend_cache_artifact(
                    staged_source,
                    function_cache_path,
                    artifact_contract=artifact_contract,
                    warnings=warnings,
                    identity=staged_identity,
                    emission_origin=emission_origin,
                )
            except OSError as exc:
                warnings.append(f"Function cache write failed: {exc}")
        if cache_key and not output_already_synced:
            try:
                state_path.parent.mkdir(parents=True, exist_ok=True)
                _write_artifact_sync_state(
                    state_path,
                    source_key=_backend_artifact_source_key(
                        cache_key,
                        stdlib_object_cache_key=stdlib_object_cache_key,
                        artifact_contract=artifact_contract,
                    ),
                    tier="module",
                    artifact=output_artifact,
                    identity=output_identity,
                    emission_origin=emission_origin,
                )
            except OSError as exc:
                warnings.append(f"Backend cache sync receipt write failed: {exc}")

    if backend_output not in {output_artifact, cache_path, function_cache_path}:
        try:
            backend_output.unlink(missing_ok=True)
        except OSError as exc:
            warnings.append(f"Backend private output cleanup failed: {exc}")
    return _BackendArtifactStageResult(None, emission_origin)


def _stdlib_object_count_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".count")


def _stdlib_object_key_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".key")


def _stdlib_object_manifest_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".manifest.json")


def _stdlib_object_partition_manifest_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".partition.json")


def _stdlib_object_digest_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".sha256")


def _stdlib_object_origin_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".origin.json")


def _stdlib_object_symbol_contract_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".symbol-contract.json")


def _shared_stdlib_manifest(
    *,
    cache_key: str | None,
    cache_variant: str,
    target_triple: str | None,
    compiler_fingerprint: str | None = None,
) -> str | None:
    if not cache_key:
        return None
    if compiler_fingerprint is None:
        compiler_fingerprint = _shared_stdlib_compiler_fingerprint()
    payload = {
        "schema": _SHARED_STDLIB_MANIFEST_SCHEMA_VERSION,
        "artifact_kind": "archive",
        "cache_key": cache_key,
        "cache_variant": cache_variant,
        "compiler_fingerprint": compiler_fingerprint,
        "target_triple": target_triple,
    }
    return json.dumps(payload, sort_keys=True, separators=(",", ":"))


@contextmanager
def _shared_stdlib_cache_lock(stdlib_object_path: Path) -> Generator[None]:
    lock_path = shared_stdlib_publish_lock_path(stdlib_object_path)
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=None,
        timeout_message=f"Timed out waiting for shared stdlib cache lock {lock_path}.",
    )
    try:
        yield
    finally:
        _release_file_lock(handle)


def _validate_shared_stdlib_output_for_link(
    path: Path,
    *,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
    target_triple: str | None = None,
) -> None:
    try:
        validate_required_stdlib_output(path)
    except ValueError as error:
        raise OSError(str(error)) from error
    if not _shared_stdlib_cache_matches_key_locked(
        path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        target_triple=target_triple,
    ):
        raise OSError(f"Required build-owned stdlib archive failed admission: {path}")


def _hydrate_shared_stdlib_archive(
    stdlib_object_path: Path,
    *,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
    output_path: Path,
    target_triple: str | None = None,
) -> Path | None:
    # Admission transfers the complete immutable generation into build custody
    # before releasing its cache stripe. Later compiler/link work uses this copy.
    staged_stdlib_obj = output_path
    try:
        validate_required_stdlib_output(output_path, stdlib_object_path)
    except ValueError as error:
        raise OSError(str(error)) from error
    stdlib_object_path = stdlib_object_path.resolve()
    staged_key_path = _stdlib_object_key_sidecar_path(staged_stdlib_obj)
    staged_count_path = _stdlib_object_count_sidecar_path(staged_stdlib_obj)
    staged_manifest_path = _stdlib_object_manifest_sidecar_path(staged_stdlib_obj)
    staged_partition_manifest_path = _stdlib_object_partition_manifest_sidecar_path(
        staged_stdlib_obj
    )
    staged_origin_path = _stdlib_object_origin_sidecar_path(staged_stdlib_obj)
    staged_digest_path = _stdlib_object_digest_sidecar_path(staged_stdlib_obj)
    source_key_path = _stdlib_object_key_sidecar_path(stdlib_object_path)
    source_count_path = _stdlib_object_count_sidecar_path(stdlib_object_path)
    source_manifest_path = _stdlib_object_manifest_sidecar_path(stdlib_object_path)
    source_partition_manifest_path = _stdlib_object_partition_manifest_sidecar_path(
        stdlib_object_path
    )
    source_origin_path = _stdlib_object_origin_sidecar_path(stdlib_object_path)
    source_digest_path = _stdlib_object_digest_sidecar_path(stdlib_object_path)
    with (
        _shared_stdlib_cache_lock(stdlib_object_path),
        _shared_stdlib_cache_lock(staged_stdlib_obj),
    ):
        generation = _shared_stdlib_cache_generation_token(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        )
        if generation is None:
            _remove_shared_stdlib_cache_artifacts(staged_stdlib_obj)
            return None
        identities = {entry.path: entry for entry in generation[1]}
        if not _shared_stdlib_cache_matches_key(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        ):
            _remove_shared_stdlib_cache_artifacts(staged_stdlib_obj)
            return None

        try:

            def copy_snapshot(source: Path, destination: Path) -> None:
                identity = identities.get(source.expanduser().absolute())
                if identity is None:
                    # Optional count metadata is not part of semantic admission.
                    identity = _native_symbols._native_symbol_artifact_identity(source)
                _copy_verified_backend_artifact(source, destination, identity=identity)

            copy_snapshot(stdlib_object_path, staged_stdlib_obj)
            if source_origin_path.exists():
                copy_snapshot(source_origin_path, staged_origin_path)
            else:
                staged_origin_path.unlink(missing_ok=True)
            if source_key_path.exists():
                copy_snapshot(source_key_path, staged_key_path)
            elif stdlib_object_cache_key:
                raise OSError(
                    "Shared stdlib cache key mismatch during staging: "
                    f"missing key sidecar for {stdlib_object_path}"
                )
            elif staged_key_path.exists():
                staged_key_path.unlink()
            if source_count_path.exists():
                copy_snapshot(source_count_path, staged_count_path)
            elif staged_count_path.exists():
                staged_count_path.unlink()
            if source_manifest_path.exists():
                copy_snapshot(source_manifest_path, staged_manifest_path)
            elif stdlib_object_manifest:
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    f"missing manifest sidecar for {stdlib_object_path}"
                )
            elif staged_manifest_path.exists():
                staged_manifest_path.unlink()
            if source_partition_manifest_path.exists():
                copy_snapshot(
                    source_partition_manifest_path, staged_partition_manifest_path
                )
            else:
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    f"missing partition manifest sidecar for {stdlib_object_path}"
                )
            if source_digest_path.exists():
                copy_snapshot(source_digest_path, staged_digest_path)
            else:
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    f"missing object digest sidecar for {stdlib_object_path}"
                )
        except OSError as exc:
            try:
                _remove_shared_stdlib_cache_artifacts(staged_stdlib_obj)
            except OSError as cleanup_error:
                exc.add_note(
                    f"Shared stdlib staging cleanup also failed: {cleanup_error}"
                )
            raise
        if shared_stdlib_archive_cache_root(stdlib_object_path) is not None:
            refresh_cache_entry_recency(stdlib_object_path.parent)
    return staged_stdlib_obj


def _remove_shared_stdlib_cache_artifacts(stdlib_object_path: Path) -> None:
    """Attempt every owned path; a partial invalidation is never a cache miss."""
    paths = (
        stdlib_object_path,
        _stdlib_object_count_sidecar_path(stdlib_object_path),
        _stdlib_object_key_sidecar_path(stdlib_object_path),
        _stdlib_object_manifest_sidecar_path(stdlib_object_path),
        _stdlib_object_partition_manifest_sidecar_path(stdlib_object_path),
        _stdlib_object_digest_sidecar_path(stdlib_object_path),
        _stdlib_object_symbol_contract_sidecar_path(stdlib_object_path),
        _stdlib_object_origin_sidecar_path(stdlib_object_path),
        _native_symbols._native_object_symbol_facts_sidecar_path(stdlib_object_path),
    )
    failures: list[tuple[Path, OSError]] = []
    for path in paths:
        try:
            path.unlink()
        except FileNotFoundError:
            continue
        except OSError as exc:
            failures.append((path, exc))
    if failures:
        detail = "; ".join(f"{path}: {exc}" for path, exc in failures)
        raise OSError(
            f"Failed to remove shared stdlib cache artifacts: {detail}"
        ) from ExceptionGroup(
            "Shared stdlib artifact deletion failures",
            [exc for _path, exc in failures],
        )


def _shared_stdlib_artifact_validation_error(
    path: Path, *, target_triple: str | None
) -> BackendArtifactValidationError | None:
    contract = BackendArtifactContract(
        BackendArtifactKind.NATIVE_ARCHIVE, target_triple
    )
    try:
        contract.validate(path)
    except BackendArtifactValidationError as error:
        return error
    return None


def _shared_stdlib_cache_matches_key(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
    target_triple: str | None = None,
) -> bool:
    if (
        stdlib_object_path is None
        or stdlib_object_cache_key is None
        or stdlib_object_manifest is None
    ):
        return False
    stage_start = time.perf_counter()
    if not stdlib_object_path.exists():
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_exists",
            stage_start,
        )
        return False
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_exists",
        stage_start,
    )
    stage_start = time.perf_counter()
    try:
        cached_key = _stdlib_object_key_sidecar_path(stdlib_object_path).read_text(
            encoding="utf-8"
        )
    except OSError:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    if cached_key.strip() != stdlib_object_cache_key:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    try:
        cached_manifest = _stdlib_object_manifest_sidecar_path(
            stdlib_object_path
        ).read_text(encoding="utf-8")
    except OSError:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    if cached_manifest.strip() != stdlib_object_manifest:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    partition_manifest_path = _stdlib_object_partition_manifest_sidecar_path(
        stdlib_object_path
    )
    if not partition_manifest_path.exists():
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    try:
        cached_object_digest = _stdlib_object_digest_sidecar_path(
            stdlib_object_path
        ).read_text(encoding="utf-8")
    except OSError:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_sidecars",
        stage_start,
    )
    stage_start = time.perf_counter()
    try:
        contract = BackendArtifactContract(
            BackendArtifactKind.NATIVE_ARCHIVE, target_triple
        )
        with _native_symbols._native_symbol_facts_admission(
            stdlib_object_path,
            target_triple=target_triple,
            validate_shape=lambda opened: contract.validate_native_shape(
                opened.path, opened=opened
            ),
        ) as (_opened, admitted, symbol_facts):
            actual_object_digest = admitted.sha256
    except (
        _native_symbols.NativeSymbolArtifactError,
        BackendArtifactValidationError,
    ):
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_digest",
            stage_start,
        )
        return False
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_digest",
        stage_start,
    )
    if cached_object_digest.strip().lower() != actual_object_digest.lower():
        return False
    stage_start = time.perf_counter()
    partition_facts = _read_shared_stdlib_partition_facts(stdlib_object_path)
    if partition_facts is None:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_partition",
            stage_start,
        )
        return False
    partition_identity, partition_functions = partition_facts
    partition_manifest_digest = partition_identity.sha256
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_partition",
        stage_start,
    )
    stage_start = time.perf_counter()
    if _shared_stdlib_symbol_contract_matches(
        stdlib_object_path,
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        object_digest=actual_object_digest,
        partition_manifest_digest=partition_manifest_digest,
        target_triple=target_triple,
    ):
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_symbol_token",
            stage_start,
        )
        return True
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_symbol_token",
        stage_start,
    )
    stage_start = time.perf_counter()
    symbol_closure_ok = (
        _shared_stdlib_native_symbol_closure_issue(
            stdlib_object_path,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
            symbol_sets=symbol_facts.symbol_sets(),
            partition_functions=partition_functions,
        )
        is None
    )
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_symbols",
        stage_start,
    )
    if symbol_closure_ok:
        with contextlib.suppress(OSError):
            _write_shared_stdlib_symbol_contract(
                stdlib_object_path,
                stdlib_object_cache_key=stdlib_object_cache_key,
                stdlib_object_manifest=stdlib_object_manifest,
                stdlib_module_symbols=stdlib_module_symbols,
                object_digest=actual_object_digest,
                partition_manifest_digest=partition_manifest_digest,
                target_triple=target_triple,
            )
    return symbol_closure_ok


def _shared_stdlib_cache_matches_key_locked(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
    evict_corrupt: bool = False,
    target_triple: str | None = None,
) -> bool:
    return (
        _shared_stdlib_cache_validation_token(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
            evict_corrupt=evict_corrupt,
            stage_timings_ms=stage_timings_ms,
        )
        is not None
    )


def _shared_stdlib_contract_identity(
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    target_triple: str | None = None,
) -> str:
    payload = {
        "symbol_schema": _SHARED_STDLIB_SYMBOL_CONTRACT_SCHEMA_VERSION,
        "symbol_protocol": _native_symbols._NATIVE_SYMBOL_FACTS_PROTOCOL,
        "symbol_target": _native_symbols._symbol_normalization_target(target_triple),
        "key": stdlib_object_cache_key,
        "manifest": stdlib_object_manifest,
        "symbols": sorted(set(stdlib_module_symbols or ())),
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _shared_stdlib_symbol_contract_payload(
    *,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None,
    object_digest: str,
    partition_manifest_digest: str,
    target_triple: str | None = None,
) -> dict[str, object]:
    return {
        "schema": _SHARED_STDLIB_SYMBOL_CONTRACT_SCHEMA_VERSION,
        "contract_identity": _shared_stdlib_contract_identity(
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        ),
        "object_digest": object_digest,
        "partition_manifest_digest": partition_manifest_digest,
    }


def _shared_stdlib_symbol_contract_matches(
    stdlib_object_path: Path,
    *,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None,
    object_digest: str,
    partition_manifest_digest: str,
    target_triple: str | None = None,
) -> bool:
    path = _stdlib_object_symbol_contract_sidecar_path(stdlib_object_path)
    try:
        source = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        return False
    try:
        payload = json.loads(source)
    except (ValueError, RecursionError):
        return False
    expected = _shared_stdlib_symbol_contract_payload(
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        object_digest=object_digest,
        partition_manifest_digest=partition_manifest_digest,
        target_triple=target_triple,
    )
    return payload == expected


def _write_shared_stdlib_symbol_contract(
    stdlib_object_path: Path,
    *,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None,
    object_digest: str,
    partition_manifest_digest: str,
    target_triple: str | None = None,
) -> None:
    payload = _shared_stdlib_symbol_contract_payload(
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        object_digest=object_digest,
        partition_manifest_digest=partition_manifest_digest,
        target_triple=target_triple,
    )
    _atomic_write_json(
        _stdlib_object_symbol_contract_sidecar_path(stdlib_object_path),
        payload,
        indent=2,
    )


def _shared_stdlib_cache_validation_file_token(
    path: Path,
) -> StableRegularFileIdentity | None:
    try:
        return _native_symbols._native_symbol_artifact_identity(path)
    except _native_symbols.NativeSymbolArtifactError:
        return None


def _shared_stdlib_cache_generation_token(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    target_triple: str | None = None,
) -> _SharedStdlibCacheValidationToken | None:
    if stdlib_object_path is None:
        return None
    paths = [
        stdlib_object_path,
        _stdlib_object_key_sidecar_path(stdlib_object_path),
        _stdlib_object_digest_sidecar_path(stdlib_object_path),
        _stdlib_object_partition_manifest_sidecar_path(stdlib_object_path),
    ]
    origin_path = _stdlib_object_origin_sidecar_path(stdlib_object_path)
    if origin_path.exists():
        paths.append(origin_path)
    if stdlib_object_manifest is not None:
        paths.append(_stdlib_object_manifest_sidecar_path(stdlib_object_path))
    entries: list[StableRegularFileIdentity] = []
    for path in paths:
        token = _shared_stdlib_cache_validation_file_token(path)
        if token is None:
            return None
        entries.append(token)
    return (
        _shared_stdlib_contract_identity(
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        ),
        tuple(entries),
    )


def _shared_stdlib_cache_validation_token(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    target_triple: str | None = None,
    previous_token: _SharedStdlibCacheValidationToken | None = None,
    evict_corrupt: bool = False,
    stage_timings_ms: dict[str, float] | None = None,
) -> _SharedStdlibCacheValidationToken | None:
    """Validate and issue a token while owning the same publication generation."""
    if stdlib_object_path is None:
        return None
    stage_start = time.perf_counter()
    with _shared_stdlib_cache_lock(stdlib_object_path):
        _record_backend_cache_stage_ms(
            stage_timings_ms, "backend_cache_stdlib_contract_lock", stage_start
        )
        before = _shared_stdlib_cache_generation_token(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        )
        reuse_previous = (
            before is not None
            and previous_token is not None
            and before == previous_token
            and (
                _shared_stdlib_artifact_validation_error(
                    stdlib_object_path, target_triple=target_triple
                )
                is None
            )
        )
        if not reuse_previous and not _shared_stdlib_cache_matches_key(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
            stage_timings_ms=stage_timings_ms,
        ):
            if (
                evict_corrupt
                and stdlib_object_cache_key
                and _read_stdlib_cache_key(stdlib_object_path)
                == stdlib_object_cache_key
            ):
                _remove_shared_stdlib_cache_artifacts(stdlib_object_path)
            return None
        after = _shared_stdlib_cache_generation_token(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        )
        if before != after:
            raise _native_symbols.NativeSymbolInspectionError(
                stdlib_object_path,
                ["shared stdlib generation changed during locked validation"],
            )
        return after


def _shared_stdlib_cache_validation_token_matches(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    token: _SharedStdlibCacheValidationToken,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    target_triple: str | None = None,
) -> bool:
    return token == _shared_stdlib_cache_validation_token(
        stdlib_object_path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        target_triple=target_triple,
        previous_token=token,
    )


def _native_stdlib_object_split_enabled(*, target: Target, emit_mode: str) -> bool:
    # An explicit object is one complete relocatable compilation unit, not
    # an archive disguised as an object after merging a cached stdlib.
    return target is TargetKind.NATIVE and emit_mode != "obj"


def _stdlib_module_symbols(
    module_graph_metadata: _ModuleGraphMetadata,
) -> frozenset[str]:
    stdlib_like_by_module = module_graph_metadata.stdlib_like_by_module or {}
    return frozenset(
        _module_symbol_name(module_name)
        for module_name, is_stdlib in sorted(stdlib_like_by_module.items())
        if is_stdlib
    )


def _encode_stdlib_module_symbols(stdlib_module_symbols: Collection[str]) -> str:
    return json.dumps(sorted(set(stdlib_module_symbols)), separators=(",", ":"))


def _is_user_owned_symbol(
    name: str,
    entry_module: str,
    *,
    stdlib_module_symbols: Collection[str] | None = None,
) -> bool:
    entry_init = f"molt_init_{entry_module}"
    if (
        name == "molt_main"
        or name == "molt_host_init"
        or name.startswith(f"{entry_module}__")
        or name == entry_init
        or name == "molt_init___main__"
        or name == "molt_isolate_import"
        or name == "molt_isolate_bootstrap"
    ):
        return True
    if stdlib_module_symbols is not None:
        return not _emitted_name_matches_any_module_symbol(
            name, _module_symbol_set(stdlib_module_symbols)
        )
    return False


def _is_stdlib_owned_symbol(
    name: str,
    *,
    stdlib_module_symbols: Collection[str],
) -> bool:
    if name in {
        "molt_main",
        "molt_host_init",
        "molt_init___main__",
        "molt_isolate_import",
        "molt_isolate_bootstrap",
    }:
        return False
    return _emitted_name_matches_any_module_symbol(
        name, _module_symbol_set(stdlib_module_symbols)
    )


def _module_symbol_set(module_symbols: Collection[str]) -> frozenset[str]:
    return (
        module_symbols
        if isinstance(module_symbols, frozenset)
        else frozenset(module_symbols)
    )


def _reachable_function_names_for_stdlib_cache(
    ir: Mapping[str, Any],
    *,
    stdlib_module_symbols: Collection[str],
) -> set[str]:
    functions = ir.get("functions")
    if not isinstance(functions, list) or not functions:
        return set()

    function_maps = [func for func in functions if isinstance(func, Mapping)]
    defined: set[str] = {
        name
        for func in function_maps
        if isinstance((name := func.get("name")), str) and name
    }

    roots: list[str] = []
    first_function = functions[0]
    if isinstance(first_function, Mapping):
        first_name = first_function.get("name")
        if isinstance(first_name, str) and first_name in defined:
            roots.append(first_name)
    if "molt_main" in defined:
        roots.append("molt_main")
    roots.extend(
        sorted(name for name in defined if _is_protected_runtime_entrypoint(name))
    )
    roots.extend(
        init_name
        for init_name in (
            f"molt_init_{module_symbol}"
            for module_symbol in sorted(set(stdlib_module_symbols))
        )
        if init_name in defined
    )
    return set(reachable_function_names(function_maps, extra_roots=roots))


def _shared_stdlib_cache_payload_ir(
    ir: Mapping[str, Any],
    *,
    entry_module: str,
    stdlib_module_symbols: Collection[str],
    compiler_fingerprint: str | None = None,
) -> dict[str, Any]:
    """Build a cache payload for the stdlib shared object.

    The key is based on the sorted stdlib function subset and their
    backend-facing IR bodies, excluding user-owned symbols. This preserves
    sharing across programs that import the same stdlib surface while
    invalidating automatically when stdlib lowering changes.
    """
    functions = ir.get("functions")
    stdlib_functions: list[dict[str, Any]] = []
    stdlib_module_symbols = _module_symbol_set(stdlib_module_symbols)
    reachable = _reachable_function_names_for_stdlib_cache(
        ir,
        stdlib_module_symbols=stdlib_module_symbols,
    )
    if isinstance(functions, list):
        for func in functions:
            if not isinstance(func, dict):
                continue
            name = func.get("name")
            if (
                not isinstance(name, str)
                or _is_user_owned_symbol(
                    name,
                    entry_module,
                    stdlib_module_symbols=stdlib_module_symbols,
                )
                or not _is_stdlib_owned_symbol(
                    name,
                    stdlib_module_symbols=stdlib_module_symbols,
                )
            ):
                continue
            if reachable and name not in reachable:
                continue
            stdlib_functions.append(func)
    stdlib_functions = _sorted_ir_functions(stdlib_functions)
    if compiler_fingerprint is None:
        compiler_fingerprint = _shared_stdlib_compiler_fingerprint()
    return {
        "cache_schema": _SHARED_STDLIB_CACHE_SCHEMA_VERSION,
        "compiler_fingerprint": compiler_fingerprint,
        "functions": stdlib_functions,
        "profile": ir.get("profile"),
        "stdlib_module_symbols": sorted(set(stdlib_module_symbols)),
    }


def _shared_stdlib_cache_key(
    ir: Mapping[str, Any],
    *,
    entry_module: str,
    stdlib_module_symbols: Collection[str],
    target_triple: str | None,
    cache_variant: str,
    compiler_fingerprint: str | None = None,
    cache_compiler_fingerprint: str | None = None,
    cache_tooling_fingerprint: str | None = None,
    function_digests: FunctionDigests | None = None,
) -> str:
    if compiler_fingerprint is None:
        compiler_fingerprint = _shared_stdlib_compiler_fingerprint(
            cache_compiler_fingerprint=cache_compiler_fingerprint,
            cache_tooling_fingerprint=cache_tooling_fingerprint,
        )
    payload_ir = _shared_stdlib_cache_payload_ir(
        ir,
        entry_module=entry_module,
        stdlib_module_symbols=stdlib_module_symbols,
        compiler_fingerprint=compiler_fingerprint,
    )
    return _cache_key(
        cast(dict[str, Any], ir),
        TargetKind.NATIVE,
        target_triple,
        f"native-stdlib:{cache_variant}",
        payload_ir=payload_ir,
        compiler_fingerprint=cache_compiler_fingerprint,
        tooling_fingerprint=cache_tooling_fingerprint,
        function_digests=function_digests,
    )


def _shared_stdlib_compiler_fingerprint(
    *,
    cache_compiler_fingerprint: str | None = None,
    cache_tooling_fingerprint: str | None = None,
) -> str:
    if cache_compiler_fingerprint is None:
        # No admitted compiler means no build request: the baseline compiler.
        cache_compiler_fingerprint = _cache_fingerprint(rust_cpu=RustCpuPolicy.BASELINE)
    if cache_tooling_fingerprint is None:
        cache_tooling_fingerprint = _cache_tooling_fingerprint()
    payload = {
        "runtime_backend": cache_compiler_fingerprint,
        "tooling": cache_tooling_fingerprint,
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _read_stdlib_cache_key(stdlib_path: Path) -> str | None:
    try:
        raw = _stdlib_object_key_sidecar_path(stdlib_path).read_text(encoding="utf-8")
    except OSError:
        return None
    key = raw.strip()
    return key or None


def _shared_stdlib_cache_mismatch_detail(
    stdlib_path: Path,
    expected_key: str | None,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    target_triple: str | None = None,
) -> str:
    actual_key = _read_stdlib_cache_key(stdlib_path)
    if not expected_key:
        return f"{stdlib_path} (missing expected key)"
    if actual_key is None:
        return f"{stdlib_path} (missing sidecar; expected key {expected_key})"
    if actual_key == expected_key:
        if stdlib_object_manifest is not None:
            manifest_path = _stdlib_object_manifest_sidecar_path(stdlib_path)
            try:
                actual_manifest = manifest_path.read_text(encoding="utf-8").strip()
            except OSError:
                return f"{stdlib_path} (missing manifest sidecar)"
            if actual_manifest != stdlib_object_manifest:
                return f"{stdlib_path} (manifest sidecar mismatch)"
        artifact_error = _shared_stdlib_artifact_validation_error(
            stdlib_path, target_triple=target_triple
        )
        if artifact_error is not None:
            return f"{stdlib_path} ({artifact_error})"
        issue = _shared_stdlib_native_symbol_closure_issue(
            stdlib_path,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        )
        if issue is not None:
            return f"{stdlib_path} ({issue})"
        return str(stdlib_path)
    return f"{stdlib_path} (expected {expected_key}, found {actual_key})"


def _stdlib_object_cache_path(
    cache_path: Path | None,
    stdlib_cache_key: str | None,
) -> Path | None:
    """Return a shared stdlib cache path scoped to exact stdlib IR identity."""
    if cache_path is None or stdlib_cache_key is None:
        return None
    cache_root = cache_path.parent
    entry_root, _lock = shared_stdlib_cache_entry(cache_root, stdlib_cache_key)
    return entry_root / "stdlib.a"


def _validate_shared_stdlib_cache_contract(
    stdlib_object_path: Path,
    project_root: Path | None,
    expected_key: str | None = None,
    *,
    expected_manifest: str | None = None,
    target_triple: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
) -> bool:
    """Validate a shared stdlib entry and evict corrupt exact-key artifacts."""
    del project_root
    return _shared_stdlib_cache_matches_key_locked(
        stdlib_object_path,
        expected_key,
        stdlib_object_manifest=expected_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        stage_timings_ms=stage_timings_ms,
        evict_corrupt=True,
        target_triple=target_triple,
    )


_SHARED_STDLIB_CACHE_SCHEMA_VERSION = "stdlib-v4-archive"


_SHARED_STDLIB_MANIFEST_SCHEMA_VERSION = "stdlib-manifest-v2-archive"


_SHARED_STDLIB_PARTITION_SCHEMA_VERSION = "stdlib-partition-v2-exact-linkage-abi"
