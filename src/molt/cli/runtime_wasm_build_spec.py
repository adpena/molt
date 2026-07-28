from __future__ import annotations

import hashlib
import json
import os
import shlex
import time
from pathlib import Path
from typing import (
    Any,
    Callable,
    Literal,
    Mapping,
    NamedTuple,
    TypeVar,
    cast,
)

from molt._runtime_feature_gates import link_affecting_feature_gate_for_symbol
from molt._wasm_abi_generated import (
    WASM_RESERVED_RUNTIME_CALLABLE_BASE,
    WASM_RESERVED_RUNTIME_CALLABLE_COUNT,
)
from molt._wasm_runtime_exports import (
    wasm_cpython_abi_requested_data_export_names,
    wasm_cpython_abi_requested_export_names,
    wasm_runtime_export_link_args,
    wasm_runtime_export_name_for_import,
    wasm_runtime_shared_export_link_args,
)
from molt.cli import wasm_toolchain
from molt.cli.artifact_state import (
    _runtime_fingerprint_path,
)
from molt.cli.cargo_execution import (
    _cargo_build_env,
)
from molt.cli.config_resolution import (
    DEFAULT_RUNTIME_STDLIB_PROFILE,
)
from molt.cli.runtime_artifact_selection import (
    RUNTIME_CDYLIB_ARTIFACTS,
    RUNTIME_STATICLIB_ARTIFACTS,
    RUNTIME_WASM_COMBINED_ARTIFACTS,
    RuntimeArtifactSelection,
)
from molt.cli.runtime_build_identity import (
    RuntimeBuildIdentity,
    RuntimePairMemberPlan,
    RuntimeToolchainContentManifest,
    _tree_hash_worker_count,
    provision_runtime_toolchain_content_manifest,
    resolve_runtime_build_pair_identities,
)
from molt.cli.runtime_features import (
    _runtime_builtin_features_for_profile,
    _wasm_runtime_feature_plan,
)
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
    _runtime_fingerprint,
)
from molt.cli.runtime_paths import (
    _cargo_profile_dir,
    _cargo_target_root,
)
from molt.cli.runtime_wasm_build_policy import (
    _resolve_wasm_cargo_profile,
    _runtime_wasm_incremental_enabled,
    _runtime_wasm_incremental_family_key,
    _runtime_wasm_incremental_target_root,
)
from molt.cli.runtime_wasm_build_support import (
    _append_rustflags_text,
    _configure_wasi_sysroot_env,
    _configure_wasm_cc_env,
    _configure_wasm_long_double_env,
    _reloc_link_archive_fingerprint_token,
    _wasm_runtime_codegen_rustflags,
)
from molt.cli.runtime_wasm_build_timings import (
    _record_runtime_wasm_build_phase,
)
from molt.cli.wasm_link_args import (
    wasm_link_args_response_file as _wasm_link_args_response_file,
)


class _RuntimeWasmBuildSpec(NamedTuple):
    """Resolved, mode-specific build spec for one runtime-wasm artifact.

    Single source of truth for the cargo profile, feature plan, RUSTFLAGS, and
    the content-address ``fingerprint`` of a reloc/shared runtime-wasm build.
    Shared by ``_ensure_runtime_wasm`` (which consumes exactly these values) and
    ``_prepopulate_combined_runtime_wasm_target`` (which must compute a
    byte-identical ``fingerprint`` so the combined single-compile's artifacts are
    recognised by the per-artifact target-reuse fast path).  Keeping one
    authority means the two can never silently drift out of fingerprint parity.
    """

    requested_cargo_profile: str
    cargo_profile: str
    profile_dir: str
    incremental_enabled: bool
    env: dict[str, str]
    artifact_selection: RuntimeArtifactSelection
    runtime_exports: str
    link_flags: str
    cargo_link_response_path: Path | None
    cargo_rustflags: str
    fingerprint_rustflags: str
    no_default_features: bool
    wasm_cargo_features: tuple[str, ...]
    fingerprint_features: tuple[str, ...]
    fingerprint_path: Path
    target_root: Path
    stored_fingerprint: dict[str, Any] | None
    fingerprint: dict[str, Any] | None
    staticlib_fingerprint: dict[str, Any] | None


def _resolved_runtime_wasm_pair_identities(
    root: Path,
    shared_spec: _RuntimeWasmBuildSpec,
    reloc_spec: _RuntimeWasmBuildSpec,
    *,
    toolchain_manifest: RuntimeToolchainContentManifest | None = None,
) -> tuple[RuntimeBuildIdentity, RuntimeBuildIdentity]:
    if (
        shared_spec.cargo_profile != reloc_spec.cargo_profile
        or shared_spec.fingerprint_features != reloc_spec.fingerprint_features
        or shared_spec.cargo_rustflags != reloc_spec.cargo_rustflags
    ):
        raise ValueError("runtime shared/reloc specs do not form one resolved pair")
    sysroot_raw = shared_spec.env.get("MOLT_WASI_SYSROOT") or shared_spec.env.get(
        "WASI_SYSROOT"
    )
    linker = wasm_toolchain.resolve_wasm_linker()
    policy = wasm_toolchain.resolve_long_double_link_policy(required=True)
    wasi_libc = wasm_toolchain.wasm_wasi_libc_archive()
    rust_builtins = wasm_toolchain.wasm_compiler_builtins_archive()
    if (
        not sysroot_raw
        or linker is None
        or policy.error is not None
        or policy.printscan is None
        or policy.builtins is None
        or wasi_libc is None
        or rust_builtins is None
    ):
        raise ValueError(
            policy.error or "runtime WASM toolchain identity is incomplete"
        )
    preserve_debug = any(
        marker in shared_spec.cargo_profile.lower() for marker in ("dev", "debug")
    )
    return resolve_runtime_build_pair_identities(
        root,
        env=shared_spec.env,
        cargo_profile=shared_spec.cargo_profile,
        target_triple="wasm32-wasip1",
        runtime_features=shared_spec.fingerprint_features,
        base_rustflags=shared_spec.cargo_rustflags,
        producer_artifact_selection=RUNTIME_WASM_COMBINED_ARTIFACTS,
        shared=RuntimePairMemberPlan(
            kind="shared",
            resolved_rustflags=shared_spec.fingerprint_rustflags,
            link_args=tuple(shlex.split(shared_spec.link_flags)),
            publication_transform="strip-final-link-metadata-v1",
            preserve_debug=preserve_debug,
        ),
        reloc=RuntimePairMemberPlan(
            kind="reloc",
            resolved_rustflags=reloc_spec.fingerprint_rustflags,
            link_args=tuple(shlex.split(reloc_spec.link_flags)),
            publication_transform="relocatable-wasm-byte-identity-v1",
            preserve_debug=preserve_debug,
        ),
        wasi_sysroot=Path(sysroot_raw),
        wasm_linker=linker.path,
        long_double_archive=policy.printscan,
        builtins_archive=policy.builtins,
        wasi_libc_archive=wasi_libc,
        rust_builtins_archive=rust_builtins,
        toolchain_manifest=toolchain_manifest,
    )


def _provision_runtime_wasm_toolchain_manifest(
    spec: _RuntimeWasmBuildSpec,
) -> RuntimeToolchainContentManifest:
    sysroot_raw = spec.env.get("MOLT_WASI_SYSROOT") or spec.env.get("WASI_SYSROOT")
    linker = wasm_toolchain.resolve_wasm_linker()
    policy = wasm_toolchain.resolve_long_double_link_policy(required=True)
    wasi_libc = wasm_toolchain.wasm_wasi_libc_archive()
    rust_builtins = wasm_toolchain.wasm_compiler_builtins_archive()
    if (
        not sysroot_raw
        or linker is None
        or policy.error is not None
        or policy.printscan is None
        or policy.builtins is None
        or wasi_libc is None
        or rust_builtins is None
    ):
        raise ValueError(
            policy.error or "runtime WASM toolchain identity is incomplete"
        )
    return provision_runtime_toolchain_content_manifest(
        env=spec.env,
        target_triple="wasm32-wasip1",
        wasi_sysroot=Path(sysroot_raw),
        wasm_linker=linker.path,
        long_double_archive=policy.printscan,
        builtins_archive=policy.builtins,
        wasi_libc_archive=wasi_libc,
        rust_builtins_archive=rust_builtins,
    )


def _runtime_wasm_toolchain_manifest_path(spec: _RuntimeWasmBuildSpec) -> Path:
    return spec.target_root / ".molt" / "runtime-toolchain-content.wasm32-wasip1.json"


def _runtime_identity_tree_phase_detail(
    tree: Mapping[str, object] | None,
    *,
    status: str,
) -> str:
    if tree is None:
        return f"status={status}"
    file_count = tree.get("file_count")
    total_size = tree.get("total_size")
    if not isinstance(file_count, int) or not isinstance(total_size, int):
        return f"status={status}"
    workers = _tree_hash_worker_count(file_count)
    return f"status={status},files={file_count},bytes={total_size},workers={workers}"


def _runtime_toolchain_identity_tree(
    manifest: RuntimeToolchainContentManifest,
) -> Mapping[str, object] | None:
    toolchain = manifest.payload.get("toolchain")
    if not isinstance(toolchain, Mapping):
        return None
    tree = toolchain.get("wasi_sysroot")
    return cast(Mapping[str, object], tree) if isinstance(tree, Mapping) else None


def _runtime_source_identity_tree(
    identities: tuple[RuntimeBuildIdentity, RuntimeBuildIdentity],
) -> Mapping[str, object] | None:
    identity = identities[0]
    pair = identity.payload.get("pair")
    if not isinstance(pair, Mapping):
        return None
    tree = pair.get("sources")
    return cast(Mapping[str, object], tree) if isinstance(tree, Mapping) else None


_RuntimeIdentityPhaseResult = TypeVar("_RuntimeIdentityPhaseResult")


def _timed_runtime_identity_phase(
    *,
    phase: Literal["runtime_toolchain_identity", "runtime_source_identity"],
    mode: Literal["pre_build", "post_build"],
    operation: Callable[[], _RuntimeIdentityPhaseResult],
    identity_tree: Callable[[_RuntimeIdentityPhaseResult], Mapping[str, object] | None],
) -> _RuntimeIdentityPhaseResult:
    started = time.perf_counter()
    result: _RuntimeIdentityPhaseResult | None = None
    try:
        result = operation()
        return result
    finally:
        tree = identity_tree(result) if result is not None else None
        _record_runtime_wasm_build_phase(
            phase,
            time.perf_counter() - started,
            kind="pair",
            mode=mode,
            detail=_runtime_identity_tree_phase_detail(
                tree,
                status="ok" if result is not None else "failed",
            ),
        )


def _compute_runtime_wasm_build_spec(
    root: Path,
    runtime_wasm: Path,
    *,
    reloc: bool,
    cargo_profile: str,
    simd_enabled: bool,
    freestanding: bool,
    stdlib_profile: str | None,
    resolved_modules: set[str] | frozenset[str] | None,
    required_link_features: frozenset[str],
    required_exports: set[str] | frozenset[str] | None,
    wasm_linker_identity: wasm_toolchain.WasmLinkerIdentity | None = None,
) -> _RuntimeWasmBuildSpec:
    """Resolve the mode-specific runtime-wasm build spec (see _RuntimeWasmBuildSpec)."""
    # The emitted app import ABI is the final link-time requirement authority.
    # Reachability-derived features normally predict this set, but external
    # native objects and runtime-support module initializers can add imports
    # after that earlier scan.  Project every required export through the same
    # generated symbol->feature authority and close the feature plan here, so
    # Cargo can never build an artifact that the immediately following export
    # validator proves insufficient.
    export_link_features = frozenset(
        feature
        for import_name in required_exports or ()
        if (runtime_symbol := wasm_runtime_export_name_for_import(import_name))
        is not None
        if (feature := link_affecting_feature_gate_for_symbol(runtime_symbol))
        is not None
    )
    required_link_features = frozenset(required_link_features) | export_link_features
    requested_cargo_profile = cargo_profile
    cargo_profile = _resolve_wasm_cargo_profile(cargo_profile)
    profile_dir = _cargo_profile_dir(cargo_profile)
    incremental_enabled = _runtime_wasm_incremental_enabled()
    env = _cargo_build_env()
    _configure_wasm_cc_env(env)
    _configure_wasi_sysroot_env(env)
    _configure_wasm_long_double_env(env)
    if "CARGO_INCREMENTAL" not in os.environ:
        env["CARGO_INCREMENTAL"] = "1" if incremental_enabled else "0"
    cpython_abi_requested_exports = wasm_cpython_abi_requested_export_names(
        required_exports
    )
    if cpython_abi_requested_exports:
        env["MOLT_WASM_CPYTHON_ABI_EXPORTS"] = "\n".join(cpython_abi_requested_exports)
        cpython_abi_requested_data_exports = (
            wasm_cpython_abi_requested_data_export_names(required_exports)
        )
        if cpython_abi_requested_data_exports:
            env["MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS"] = "\n".join(
                cpython_abi_requested_data_exports
            )
    if reloc:
        runtime_exports = wasm_runtime_export_link_args(
            required_exports,
            resolved_modules=resolved_modules,
        )
        link_flags = runtime_exports
        cargo_link_response_path = None
    else:
        runtime_exports = wasm_runtime_shared_export_link_args(required_exports)
        shared_import_flags = (
            "-C link-arg=--import-memory -C link-arg=--import-table"
            " -C link-arg=--growable-table"
            f" -C link-arg=--table-base={1 + WASM_RESERVED_RUNTIME_CALLABLE_BASE + 2 * WASM_RESERVED_RUNTIME_CALLABLE_COUNT}"
        )
        link_flags = f"{shared_import_flags}{runtime_exports}"
        cargo_link_response_path = _wasm_link_args_response_file(
            root,
            label=f"runtime.{_resolve_wasm_cargo_profile(cargo_profile)}.shared",
            link_flags=link_flags,
        )
    base_rustflags = env.get("RUSTFLAGS", "").strip()
    cargo_rustflags = _wasm_runtime_codegen_rustflags(
        base_rustflags,
        simd_enabled=simd_enabled,
        freestanding=freestanding,
    )
    fingerprint_rustflags = _wasm_runtime_codegen_rustflags(
        _append_rustflags_text(base_rustflags, link_flags),
        simd_enabled=simd_enabled,
        freestanding=freestanding,
    )
    # Fold the long-double archive identity into BOTH runtime fingerprints so a
    # change to those archives (first provisioning, version bump, or removal)
    # invalidates the cached runtime instead of serving a stale
    # long-double-stubbed one (effect-attestation: configured != effective, M34).
    # The reloc link whole-archives them via wasm-ld; the shared cdylib links
    # them via build.rs `rustc-link-lib` (see _configure_wasm_long_double_env) â€”
    # in BOTH cases the archives never otherwise enter the fingerprint/compat
    # digest (the shared build passes them by env, not rustc link-args). The tag
    # is a pure fingerprint input (a distinct cfg name per crate-type, never
    # handed to the real compile) so the emitted wasm stays byte-identical for a
    # fixed archive set â€” the CDN-cacheable shared runtime is stable across
    # builds and only re-keys when the archives actually change.
    _longdouble_link_token = _reloc_link_archive_fingerprint_token()
    fingerprint_rustflags = _append_rustflags_text(
        fingerprint_rustflags,
        f"--cfg molt_{'reloc' if reloc else 'shared'}_longdouble_link"
        f'="{_longdouble_link_token}"',
    )
    if reloc:
        linker_token = (
            wasm_linker_identity.fingerprint_token
            if wasm_linker_identity is not None
            else "wasm-ld:unattested"
        )
        fingerprint_rustflags = _append_rustflags_text(
            fingerprint_rustflags,
            f'--cfg molt_wasm_linker_identity="{linker_token}"',
        )
    effective_stdlib_profile = stdlib_profile or DEFAULT_RUNTIME_STDLIB_PROFILE
    artifact_selection = (
        RUNTIME_STATICLIB_ARTIFACTS if reloc else RUNTIME_CDYLIB_ARTIFACTS
    )
    cargo_runtime_features = tuple(["wasm_freestanding"] if freestanding else [])
    builtin_features = _runtime_builtin_features_for_profile(
        effective_stdlib_profile,
        target_triple="wasm32-wasip1",
    )
    no_default_features, wasm_cargo_features, fingerprint_features = (
        _wasm_runtime_feature_plan(
            stdlib_profile=effective_stdlib_profile,
            runtime_features=cargo_runtime_features,
            builtin_features=builtin_features,
            resolved_modules=resolved_modules,
            required_link_features=required_link_features,
        )
    )
    fingerprint_path = _runtime_fingerprint_path(
        root, runtime_wasm, cargo_profile, "wasm32-wasip1"
    )
    if incremental_enabled:
        target_root = _runtime_wasm_incremental_target_root(
            root,
            _runtime_wasm_incremental_family_key(
                cargo_profile=cargo_profile,
                target_triple="wasm32-wasip1",
                features=tuple(fingerprint_features),
                simd_enabled=simd_enabled,
                freestanding=freestanding,
            ),
        )
    else:
        target_root = _cargo_target_root(root)
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    fingerprint = _runtime_fingerprint(
        root,
        cargo_profile=cargo_profile,
        target_triple="wasm32-wasip1",
        rustflags=fingerprint_rustflags,
        runtime_features=fingerprint_features,
        artifact_selection=artifact_selection,
        stored_fingerprint=stored_fingerprint,
    )
    # Cargo's staticlib is a pre-link compile product.  Its bytes depend on the
    # source/feature/codegen plan and on the exact CPython-ABI anchors emitted
    # by build.rs, but NOT on reloc export flags or long-double archives that
    # are consumed only by the later wasm-ld publication step.  Keying the
    # target staticlib with the final reloc fingerprint made every change from
    # the early native-object closure to the final app import subset look like
    # a codegen miss and caused a second full Cargo compile.  Keep a distinct
    # compile identity while the published reloc wasm retains the complete
    # link identity above.
    requested_function_anchors = wasm_cpython_abi_requested_export_names(
        required_exports
    )
    requested_data_anchors = wasm_cpython_abi_requested_data_export_names(
        required_exports
    )
    anchor_payload = json.dumps(
        {
            "functions": requested_function_anchors,
            "data": requested_data_anchors,
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    anchor_digest = hashlib.sha256(anchor_payload.encode("utf-8")).hexdigest()
    staticlib_identity_rustflags = _append_rustflags_text(
        cargo_rustflags,
        f'--cfg molt_staticlib_anchor_plan="{anchor_digest}"',
    )
    staticlib_fingerprint = _runtime_fingerprint(
        root,
        cargo_profile=cargo_profile,
        target_triple="wasm32-wasip1",
        rustflags=staticlib_identity_rustflags,
        runtime_features=fingerprint_features,
        artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
        stored_fingerprint=None,
    )
    return _RuntimeWasmBuildSpec(
        requested_cargo_profile=requested_cargo_profile,
        cargo_profile=cargo_profile,
        profile_dir=profile_dir,
        incremental_enabled=incremental_enabled,
        env=env,
        artifact_selection=artifact_selection,
        runtime_exports=runtime_exports,
        link_flags=link_flags,
        cargo_link_response_path=cargo_link_response_path,
        cargo_rustflags=cargo_rustflags,
        fingerprint_rustflags=fingerprint_rustflags,
        no_default_features=no_default_features,
        wasm_cargo_features=tuple(wasm_cargo_features),
        fingerprint_features=tuple(fingerprint_features),
        fingerprint_path=fingerprint_path,
        target_root=target_root,
        stored_fingerprint=stored_fingerprint,
        fingerprint=fingerprint,
        staticlib_fingerprint=staticlib_fingerprint,
    )
