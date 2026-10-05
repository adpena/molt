from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
import time
from pathlib import Path

from molt._wasm_abi_generated import WASM_NON_RUNTIME_CALLABLE_INTRINSICS
from molt.file_publication import atomic_write_bytes
from molt.cli import native_symbol_inspection
from molt.cli.config_resolution import DEFAULT_RUNTIME_STDLIB_PROFILE
from molt.cli.installed_runtime_contract import InstalledNativeAdmission
from molt.cli.installed_runtime import (
    InstalledRuntimeCell,
    installed_native_callable_projection,
    select_installed_native_runtime,
)
from molt.cli.models import _RuntimeArtifactState
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail
from molt.cli.runtime_native_build import _ensure_native_runtime_lib_ready_for_codegen
from molt.cli.runtime_native_codegen import NativeRuntimeCodegenBinding
from molt.cli.native_link_manifest import (
    NativeLinkDependencyManifestError,
    read_native_link_dependency_manifest,
)
from molt.toolchain_identity import (
    StableRegularFileHandle,
    StableRegularFileIdentity,
    capture_stable_regular_file,
    verify_stable_regular_file_identity,
)


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


@dataclass(frozen=True, slots=True)
class RuntimeCallableProjection:
    """Materialized generation admitted against immutable archive-derived bytes."""

    identity: StableRegularFileIdentity
    semantic_digest: str


def _runtime_callable_symbols_file(
    runtime_lib: Path,
    *,
    identity: StableRegularFileIdentity,
    target_triple: str | None = None,
) -> tuple[RuntimeCallableProjection | None, str | None]:
    """Project runtime callables from the shared, generation-bound archive facts.

    The reader owns candidate selection, target decoration, typed failures and
    artifact/reader identity. This stage owns only the callable projection and
    its materialized input to native codegen.
    """
    try:
        with native_symbol_inspection._native_symbol_facts_admission(
            runtime_lib,
            archive=True,
            target_triple=target_triple,
            identity=identity,
            requirement=native_symbol_inspection.NativeSymbolRequirement(
                function_prefix="molt_",
                excluded_functions=WASM_NON_RUNTIME_CALLABLE_INTRINSICS,
            ),
        ) as (opened, identity, facts):
            symbols = tuple(
                sorted(
                    name
                    for name in facts.defined_functions
                    if name.startswith("molt_")
                    and name not in WASM_NON_RUNTIME_CALLABLE_INTRINSICS
                )
            )
            if not symbols:
                return None, "runtime staticlib defines no molt_* callable symbols"
            content = _runtime_callable_projection_content(symbols)
            projection_digest = hashlib.sha256(content).hexdigest()
            cache_path = runtime_lib.with_name(
                _runtime_callable_projection_name(
                    runtime_lib.name,
                    archive_sha256=identity.sha256,
                    projection_sha256=projection_digest,
                )
            )
            # Content-addressed projections are immutable generations. A concurrent
            # creator may publish first; admit its bytes without replacing the file
            # already bound by that operation. Corruption and read failures fail
            # closed below rather than invalidating another operation's generation.
            try:
                atomic_write_bytes(cache_path, content, exclusive=True)
            except FileExistsError:
                pass
            return (
                _admit_runtime_callable_projection(
                    cache_path,
                    runtime_lib=runtime_lib,
                    archive_identity=identity,
                    expected_sha256=projection_digest,
                    _archive_opened=opened,
                ),
                None,
            )
    except (OSError, ValueError) as exc:
        return None, f"runtime staticlib callable inspection failed: {exc}"


def _runtime_callable_projection_name(
    runtime_lib_name: str, *, archive_sha256: str, projection_sha256: str
) -> str:
    """Content-addressed name binding one projection to one archive generation."""
    return (
        f"{runtime_lib_name}.callable_symbols.v3."
        f"{archive_sha256}.{projection_sha256}.txt"
    )


def _runtime_callable_projection_content(symbols: tuple[str, ...]) -> bytes:
    """The one canonical byte encoding native codegen consumes."""
    return ("\n".join(symbols) + "\n").encode("utf-8")


def _runtime_callable_projection_symbols(content: bytes) -> tuple[str, ...]:
    """Decode only bytes that are exactly this module's canonical encoding."""
    try:
        text = content.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError("runtime callable projection is not canonical UTF-8") from exc
    symbols = tuple(text[:-1].split("\n")) if text.endswith("\n") else ()
    if (
        not symbols
        or _runtime_callable_projection_content(symbols) != content
        or list(symbols) != sorted(set(symbols))
        or any(
            not name.startswith("molt_") or name in WASM_NON_RUNTIME_CALLABLE_INTRINSICS
            for name in symbols
        )
    ):
        raise ValueError("runtime callable projection is not canonical")
    return symbols


def _admit_runtime_callable_projection(
    path: Path,
    *,
    runtime_lib: Path,
    archive_identity: StableRegularFileIdentity,
    expected_sha256: str,
    captured: tuple[StableRegularFileIdentity, bytes] | None = None,
    _archive_opened: StableRegularFileHandle | None = None,
) -> RuntimeCallableProjection:
    """Admit one materialized projection by bytes, archive binding and location.

    The filename is a claim, never an authority: the captured bytes must have
    the expected digest and canonical encoding, and the name must address both
    those bytes and the exact archive generation beside which they are stored.
    ``_archive_opened`` is an internal borrow from the active canonical native
    symbol admission; that transaction already checked this digest and owns
    the descriptor through this projection and its closing fences.
    """
    identity, content = (
        captured
        if captured is not None
        else capture_stable_regular_file(
            path, label="native runtime callable projection", max_bytes=16 * 1024 * 1024
        )
    )
    if identity.path != path.absolute():
        raise ValueError("runtime callable projection observation names another path")
    if identity.sha256 != expected_sha256:
        raise ValueError(
            "runtime callable projection changed before archive-derived admission: "
            f"{path}; content does not match its archive-derived digest"
        )
    if (
        path.resolve(strict=True).parent
        != archive_identity.path.resolve(strict=True).parent
    ):
        raise ValueError(
            f"runtime callable projection {path} is not adjacent to its archive "
            f"{archive_identity.path}"
        )
    if path.name != _runtime_callable_projection_name(
        runtime_lib.name,
        archive_sha256=archive_identity.sha256,
        projection_sha256=identity.sha256,
    ):
        raise ValueError(
            f"runtime callable projection {path} is not named for archive "
            f"{archive_identity.sha256} and its own bytes"
        )
    symbols = _runtime_callable_projection_symbols(content)
    verify_stable_regular_file_identity(
        identity, label="native runtime callable projection"
    )
    if _archive_opened is None:
        native_symbol_inspection._require_unchanged_symbol_artifact(
            runtime_lib, archive_identity
        )
    elif _archive_opened.path != archive_identity.path or _archive_opened.stream.closed:
        raise ValueError("callable projection lost its admitted archive handle")
    # An owned caller already hashed this archive and retains the same context
    # through materialization. Its closing fences run before the result escapes.
    return RuntimeCallableProjection(
        identity, _runtime_callable_symbols_digest(symbols)
    )


def _installed_runtime_callable_projection(
    cell: InstalledRuntimeCell,
    admission: InstalledNativeAdmission,
) -> tuple[RuntimeCallableProjection | None, str | None]:
    """Admit the signed cell's projection; installed Molt never reads symbols.

    The release producer materialized these bytes with
    ``_runtime_callable_symbols_file``. They must equal the signed cell record,
    be canonical, and be addressed to the retained archive this operation
    admitted. There is no symbol-reader fallback.
    """
    try:
        installed_native_callable_projection(cell, admission)
        return (
            RuntimeCallableProjection(
                admission.callable_projection, admission.callable_semantic_digest
            ),
            None,
        )
    except (OSError, ValueError) as exc:
        return None, f"installed runtime callable projection admission failed: {exc}"


def _runtime_callable_projection_for_codegen(
    runtime_state: _RuntimeArtifactState,
    runtime_lib: Path,
    *,
    identity: StableRegularFileIdentity,
    target_triple: str | None,
    runtime_cargo_profile: str,
    molt_root: Path,
    stdlib_profile: str | None,
) -> tuple[RuntimeCallableProjection | None, str | None]:
    """Installed cells ship their projection; source checkouts inspect the archive."""
    try:
        cell = select_installed_native_runtime(
            molt_root,
            target_triple=target_triple,
            cargo_profile=runtime_cargo_profile,
            stdlib_profile=stdlib_profile,
            extra_runtime_features=runtime_state.extra_runtime_features,
        )
    except (OSError, ValueError) as exc:
        return None, f"installed runtime selection failed: {exc}"
    admission = runtime_state.installed_native_admission
    if cell is None:
        if admission is not None:
            return None, "installed runtime selection changed after admission"
        return _runtime_callable_symbols_file(
            runtime_lib, identity=identity, target_triple=target_triple
        )
    if admission is None:
        return None, "installed runtime has no admitted generation for code generation"
    return _installed_runtime_callable_projection(cell, admission)


def _runtime_callable_symbols_digest(symbols: tuple[str, ...]) -> str:
    """Digest the same immutable symbol tuple used for the byte projection."""
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
    # Readiness may be an in-flight producer that already owns the build
    # identity. Discard only the old codegen binding until it has completed.
    runtime_state.native_runtime_codegen_binding = None
    if runtime_lib is None or is_wasm_freestanding:
        runtime_state.revoke_native_runtime_admission()
        return "", None
    ensure_start = time.perf_counter()
    runtime_ready = _ensure_native_runtime_lib_ready_for_codegen(
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
    # Readiness selects an immutable generation. The original coordinate names
    # Cargo output and cannot authorize symbol projection or final linking.
    runtime_lib = runtime_state.runtime_lib
    if not runtime_ready or runtime_lib is None or not runtime_lib.exists():
        runtime_state.revoke_native_runtime_admission()
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
    build_identity = runtime_state.native_runtime_build_identity
    if build_identity is None:
        runtime_state.revoke_native_runtime_admission()
        return "", _fail(
            "native runtime readiness omitted its admitted build identity",
            json_output,
            command="build",
        )
    admission = runtime_state.installed_native_admission
    try:
        if admission is not None:
            # The operation's installed admission hashed these retained members
            # and validated their receipt and custody; bind that exact
            # generation through its fences instead of reading it again.
            if (
                admission.runtime_lib != runtime_lib
                or admission.build_identity != build_identity
            ):
                raise ValueError(
                    "installed native runtime admission names another generation"
                )
            admission.verify()
            archive_identity = admission.archive
        else:
            archive_identity = (
                native_symbol_inspection._native_symbol_artifact_identity(runtime_lib)
            )
            read_native_link_dependency_manifest(
                runtime_lib,
                cargo_profile=runtime_cargo_profile,
                target_triple=target_triple,
                runtime_build_identity=build_identity,
            )
            native_symbol_inspection._require_unchanged_symbol_artifact(
                runtime_lib, archive_identity
            )
    except (OSError, ValueError, NativeLinkDependencyManifestError) as exc:
        runtime_state.revoke_native_runtime_admission()
        return "", _fail(
            f"native runtime changed before callable codegen admission: {exc}",
            json_output,
            command="build",
        )
    symbol_file_start = time.perf_counter()
    projection, symbols_failure = _runtime_callable_projection_for_codegen(
        runtime_state,
        runtime_lib,
        identity=archive_identity,
        target_triple=target_triple,
        runtime_cargo_profile=runtime_cargo_profile,
        molt_root=molt_root,
        stdlib_profile=stdlib_profile,
    )
    _record_runtime_callable_stage_ms(
        stage_timings_ms,
        "runtime_callable_symbols_file",
        symbol_file_start,
    )
    if projection is None:
        runtime_state.revoke_native_runtime_admission()
        return "", _fail(
            "failed to admit the runtime staticlib's molt_* callable "
            f"symbols for {runtime_lib}: {symbols_failure}. Native codegen "
            "requires callable inputs admitted from the selected runtime archive.",
            json_output,
            command="build",
        )
    binding = NativeRuntimeCodegenBinding(
        runtime_lib=runtime_lib,
        build_identity=build_identity,
        archive=archive_identity,
        callable_symbols=projection.identity,
        semantic_digest=projection.semantic_digest,
        manifest=admission.manifest if admission is not None else None,
        link_facts=admission.link_facts if admission is not None else None,
        custody=admission.custody if admission is not None else None,
    )
    try:
        binding.verify()
    except (OSError, ValueError) as exc:
        runtime_state.revoke_native_runtime_admission()
        return "", _fail(
            f"native runtime changed while binding code generation: {exc}",
            json_output,
            command="build",
        )
    runtime_state.native_runtime_codegen_binding = binding
    return projection.semantic_digest, None
