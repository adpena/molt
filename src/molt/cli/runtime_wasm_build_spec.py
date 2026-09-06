from __future__ import annotations

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
    Sequence,
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
from molt.cli.artifact_state import (
    _build_state_root,
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
    RuntimeBuildMemberPlan,
    _tree_hash_worker_count,
    resolve_wasm_runtime_build_family_identities,
    runtime_build_tooling_authority,
)
from molt.cli.runtime_cargo_plan import (
    CargoResourceRoot,
    RuntimeCargoPlan,
    resolve_runtime_cargo_plan,
)
from molt.cli.runtime_features import (
    _runtime_builtin_features_for_profile,
    _wasm_runtime_feature_plan,
)
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
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
    RuntimeWasmLinkInputs,
    resolve_runtime_wasm_link_inputs,
    _cargo_cmd_with_json_artifact_messages,
    _configure_wasi_sysroot_env,
    _configure_wasm_cc_env,
    _configure_wasm_long_double_env,
    _wasm_runtime_codegen_flags,
)
from molt.cli.runtime_wasm_build_timings import (
    _record_runtime_wasm_build_phase,
)
from molt.cli.wasm_link_args import (
    wasm_link_args_from_rustflags,
    write_wasm_link_args_response_file,
)


def _runtime_wasm_publication_authority(root: Path) -> dict[str, object]:
    """Return the shared complete runtime planning/publication authority."""

    return runtime_build_tooling_authority(root)


class _RuntimeWasmBuildSpec(NamedTuple):
    """Resolved, mode-specific build spec for one runtime-wasm artifact.

    Single source of truth for the cargo profile, feature plan, RUSTFLAGS, and
    the content-address ``fingerprint`` of a reloc/shared runtime-wasm build.
    The atomic pair producer and member finalizer consume the same resolved
    Cargo plan and compile/member fingerprint projections.
    """

    requested_cargo_profile: str
    cargo_profile: str
    profile_dir: str
    incremental_enabled: bool
    env: dict[str, str]
    artifact_selection: RuntimeArtifactSelection
    runtime_exports: str
    link_flags: str
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
    cargo_plan: RuntimeCargoPlan | None = None
    link_inputs: RuntimeWasmLinkInputs | None = None

    def with_cargo_plan(self, plan: RuntimeCargoPlan) -> _RuntimeWasmBuildSpec:
        return self._replace(
            cargo_plan=plan,
            env=dict(plan.environment),
            cargo_rustflags=shlex.join(plan.rustflags),
            fingerprint_rustflags=shlex.join(
                (*plan.rustflags, *shlex.split(self.link_flags))
            ),
        )


def _runtime_wasm_combined_cargo_command(
    spec: _RuntimeWasmBuildSpec,
) -> list[str]:
    """Construct the combined producer command before resolving one Cargo plan."""

    command = [
        "cargo",
        "rustc",
        "--package",
        "molt-runtime",
        "--profile",
        spec.cargo_profile,
        "--target",
        "wasm32-wasip1",
        "--lib",
    ]
    if spec.no_default_features:
        command.append("--no-default-features")
    if spec.wasm_cargo_features:
        command.extend(["--features", ",".join(spec.wasm_cargo_features)])
    RUNTIME_WASM_COMBINED_ARTIFACTS.select_in(command)
    command.append("--")
    return command


def _resolve_runtime_wasm_cargo_specs(
    root: Path,
    shared: _RuntimeWasmBuildSpec,
    reloc: _RuntimeWasmBuildSpec,
    *,
    simd_enabled: bool,
    freestanding: bool,
) -> tuple[_RuntimeWasmBuildSpec, _RuntimeWasmBuildSpec]:
    """Resolve the one command/environment consumed by capture and execution."""
    env = dict(shared.env)
    link_inputs: RuntimeWasmLinkInputs | None = None

    def capture_inputs(
        effective_env: Mapping[str, str],
        _tools: Mapping[str, Path],
        rust_roots: Sequence[CargoResourceRoot],
    ) -> None:
        nonlocal link_inputs
        target_libdirs = [
            resource.path
            for resource in rust_roots
            if resource.label.startswith("rust/target-libdir/")
        ]
        if not target_libdirs:
            raise ValueError(
                "runtime WASM Cargo plan did not capture its Rust target library directory"
            )
        link_inputs = resolve_runtime_wasm_link_inputs(
            env=effective_env,
            target_libdir=target_libdirs[-1],
            project_root=root,
        )

    env["CARGO_TARGET_DIR"] = str(shared.target_root)
    command = _runtime_wasm_combined_cargo_command(shared)
    command[0] = env.get("CARGO", command[0])
    link_args = wasm_link_args_from_rustflags(shared.link_flags)
    if link_args:
        response = write_wasm_link_args_response_file(
            _build_state_root(root) / "wasm_link_args",
            label=f"runtime.{shared.cargo_profile}.combined",
            link_args=link_args,
        )
        command.extend(["-C", f"link-arg=@{response}"])
    plan = resolve_runtime_cargo_plan(
        root,
        env=env,
        cargo_command=_cargo_cmd_with_json_artifact_messages(command),
        requested_target="wasm32-wasip1",
        rustflags_transform=lambda flags: _wasm_runtime_codegen_flags(
            flags,
            simd_enabled=simd_enabled,
            freestanding=freestanding,
        ),
        capture_inputs=capture_inputs,
    )
    if link_inputs is None:
        raise ValueError("runtime WASM Cargo plan did not capture its link inputs")
    return (
        shared.with_cargo_plan(plan)._replace(link_inputs=link_inputs),
        reloc.with_cargo_plan(plan)._replace(link_inputs=link_inputs),
    )


def _resolved_runtime_wasm_family_identities(
    root: Path,
    shared_spec: _RuntimeWasmBuildSpec,
    reloc_spec: _RuntimeWasmBuildSpec,
) -> tuple[RuntimeBuildIdentity, RuntimeBuildIdentity]:
    if (
        shared_spec.cargo_profile != reloc_spec.cargo_profile
        or shared_spec.fingerprint_features != reloc_spec.fingerprint_features
        or shared_spec.cargo_rustflags != reloc_spec.cargo_rustflags
    ):
        raise ValueError("runtime shared/reloc specs do not form one resolved family")
    inputs = shared_spec.link_inputs
    if inputs is None:
        raise ValueError("runtime WASM family requires its resolved link inputs")
    inputs.verify()
    if shared_spec.cargo_plan is None:
        raise ValueError("runtime WASM family requires its resolved Cargo plan")
    preserve_debug = shared_spec.cargo_plan.preserve_debug_for_profile(
        shared_spec.cargo_profile
    )
    identities = resolve_wasm_runtime_build_family_identities(
        root,
        env=shared_spec.env,
        cargo_profile=shared_spec.cargo_profile,
        target_triple="wasm32-wasip1",
        runtime_features=shared_spec.fingerprint_features,
        base_rustflags=shared_spec.cargo_rustflags,
        cargo_command=shared_spec.cargo_plan.command,
        producer_artifact_selection=RUNTIME_WASM_COMBINED_ARTIFACTS,
        publication_authority=_runtime_wasm_publication_authority(root),
        members=(
            RuntimeBuildMemberPlan(
                kind="shared",
                resolved_rustflags=shared_spec.fingerprint_rustflags,
                link_args=tuple(shlex.split(shared_spec.link_flags)),
                publication_transform="shared-runtime-publication-v2",
                preserve_debug=preserve_debug,
            ),
            RuntimeBuildMemberPlan(
                kind="reloc",
                resolved_rustflags=reloc_spec.fingerprint_rustflags,
                link_args=tuple(shlex.split(reloc_spec.link_flags)),
                publication_transform="relocatable-runtime-publication-v2",
                preserve_debug=True,
            ),
        ),
        wasi_sysroot=inputs.wasi_sysroot,
        wasm_linker=inputs.linker.entrypoint,
        long_double_archive=inputs.long_double.path,
        builtins_archive=inputs.clang_builtins.path,
        wasi_libc_archive=inputs.libc.path,
        rust_builtins_archive=inputs.rust_builtins.path,
        cargo_plan=shared_spec.cargo_plan,
    )
    if len(identities) != 2:
        raise ValueError("runtime WASM build family must contain shared and reloc")
    return identities[0], identities[1]


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


def _runtime_source_identity_tree(
    identities: tuple[RuntimeBuildIdentity, RuntimeBuildIdentity],
) -> Mapping[str, object] | None:
    identity = identities[0]
    family = identity.payload.get("family")
    if not isinstance(family, Mapping):
        return None
    compile_payload = family.get("compile")
    if not isinstance(compile_payload, Mapping):
        return None
    tree = compile_payload.get("sources")
    return cast(Mapping[str, object], tree) if isinstance(tree, Mapping) else None


_RuntimeIdentityPhaseResult = TypeVar("_RuntimeIdentityPhaseResult")


def _timed_runtime_identity_phase(
    *,
    phase: Literal["runtime_family_identity"],
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
            kind="family",
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
    else:
        runtime_exports = wasm_runtime_shared_export_link_args(required_exports)
        shared_import_flags = (
            "-C link-arg=--import-memory -C link-arg=--import-table"
            " -C link-arg=--growable-table"
            f" -C link-arg=--table-base={1 + WASM_RESERVED_RUNTIME_CALLABLE_BASE + 2 * WASM_RESERVED_RUNTIME_CALLABLE_COUNT}"
        )
        link_flags = f"{shared_import_flags}{runtime_exports}"
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
    return _RuntimeWasmBuildSpec(
        requested_cargo_profile=requested_cargo_profile,
        cargo_profile=cargo_profile,
        profile_dir=profile_dir,
        incremental_enabled=incremental_enabled,
        env=env,
        artifact_selection=artifact_selection,
        runtime_exports=runtime_exports,
        link_flags=link_flags,
        cargo_rustflags="",
        fingerprint_rustflags=link_flags,
        no_default_features=no_default_features,
        wasm_cargo_features=tuple(wasm_cargo_features),
        fingerprint_features=tuple(fingerprint_features),
        fingerprint_path=fingerprint_path,
        target_root=target_root,
        stored_fingerprint=stored_fingerprint,
        fingerprint=None,
        staticlib_fingerprint=None,
    )
