from __future__ import annotations

from molt.cargo_execution_policy import cargo_profile_directory
from molt.cli.build_request import BuildProfile

from molt.process_environment import ProcessEnvironment


import os
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

from molt._wasm_abi_generated import (
    WASM_RESERVED_RUNTIME_CALLABLE_BASE,
    WASM_RESERVED_RUNTIME_CALLABLE_COUNT,
)
from molt._wasm_runtime_exports import (
    wasm_split_runtime_export_rename_map,
    wasm_cpython_abi_requested_data_export_names,
    wasm_cpython_abi_distribution_export_names,
    wasm_cpython_abi_requested_export_names,
    wasm_runtime_export_link_args,
    wasm_runtime_import_feature,
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
    _tree_hash_worker_count,
    capture_runtime_publication_executor,
    resolve_wasm_runtime_build_family_identities,
)
from molt.cli.runtime_identity_schema import (
    RuntimeBuildIdentity,
    RuntimeBuildMemberPlan,
    RuntimePublicationAction,
    RuntimePublicationTransform,
)
from molt.cli.runtime_cargo_plan import (
    RuntimeCargoPlan,
    resolve_runtime_cargo_plan,
)
from molt.cli.runtime_features import (
    _runtime_builtin_features_for_profile,
    _wasm_runtime_feature_plan,
    profile_link_features,
)
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
)
from molt.cli.runtime_paths import _cargo_target_root
from molt.cli.runtime_wasm_build_policy import (
    _resolve_wasm_cargo_profile,
    RuntimeWasmBuildPolicy,
    RuntimeWasmIncrementality,
    RuntimeWasmTargetLayout,
    resolve_runtime_wasm_build_policy,
    _runtime_wasm_family_key,
    _runtime_wasm_family_target_root,
)
from molt.cli.runtime_wasm_build_support import (
    runtime_reloc_publication_action,
    _cargo_cmd_with_json_artifact_messages,
    _wasm_runtime_codegen_flags,
)
from molt.cli.runtime_wasm_build_timings import (
    _record_runtime_wasm_build_phase,
)
from molt.cli.wasm_link_args import (
    wasm_link_args_from_rust_flag_tokens,
    write_wasm_link_args_response_file,
)


class _RuntimeWasmBuildSpec(NamedTuple):
    """Resolved, mode-specific build spec for one runtime-wasm artifact.

    Single source of truth for the cargo profile, feature plan, RUSTFLAGS, and
    the content-address ``fingerprint`` of a reloc/shared runtime-wasm build.
    The atomic pair producer and member finalizer consume the same resolved
    Cargo plan and compile/member fingerprint projections.
    """

    publication_name: str
    requested_cargo_profile: str
    cargo_profile: str
    profile_dir: str
    build_policy: RuntimeWasmBuildPolicy
    env: ProcessEnvironment
    artifact_selection: RuntimeArtifactSelection
    runtime_exports: tuple[str, ...]
    link_flags: tuple[str, ...]
    cargo_rustflags: tuple[str, ...]
    fingerprint_rustflags: tuple[str, ...]
    no_default_features: bool
    wasm_cargo_features: tuple[str, ...]
    fingerprint_features: tuple[str, ...]
    fingerprint_path: Path
    target_root: Path
    stored_fingerprint: dict[str, Any] | None
    fingerprint: dict[str, Any] | None
    staticlib_fingerprint: dict[str, Any] | None
    cargo_plan: RuntimeCargoPlan | None = None

    def with_cargo_plan(self, plan: RuntimeCargoPlan) -> _RuntimeWasmBuildSpec:
        return self._replace(
            cargo_plan=plan,
            build_policy=self.build_policy.bind(plan),
            env=plan.environment,
            cargo_rustflags=plan.rustflags,
            fingerprint_rustflags=(*plan.rustflags, *self.link_flags),
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
    if (
        shared.build_policy != reloc.build_policy
        or shared.target_root != reloc.target_root
    ):
        raise ValueError(
            "runtime WASM members disagree on execution policy or target custody"
        )
    # A resolved spec can re-enter after a profile override. Derive our link
    # policy again rather than carrying compression into a debug-preserving link.
    link_args = wasm_link_args_from_rust_flag_tokens(shared.link_flags)
    if "--compress-relocations" in link_args:
        link_args = tuple(arg for arg in link_args if arg != "--compress-relocations")
        shared = shared._replace(
            link_flags=tuple(
                token for arg in link_args for token in ("-C", f"link-arg={arg}")
            )
        )
    env = shared.env.updated({"CARGO_TARGET_DIR": str(shared.target_root)})
    command = _runtime_wasm_combined_cargo_command(shared)
    command[0] = env.get("CARGO", command[0])
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
    )
    if plan.strips_debug_for_profile(
        shared.cargo_profile,
        package_name="molt-runtime",
        workspace_package=True,
    ):
        # wasm-ld leaves padded call/offset LEBs unless explicitly compressed.
        # Compression changes encoding, not instructions or table slots. It is
        # incompatible with retained DWARF, so use the actual final-link strip
        # policy, including package/config/environment overrides.
        if "link-arg=--compress-relocations" not in shared.link_flags:
            shared = shared._replace(
                link_flags=(*shared.link_flags, "-C", "link-arg=--compress-relocations")
            )
            compressed_response = write_wasm_link_args_response_file(
                _build_state_root(root) / "wasm_link_args",
                label=f"runtime.{shared.cargo_profile}.combined",
                link_args=wasm_link_args_from_rust_flag_tokens(shared.link_flags),
            )
            plan = plan.with_final_link_response(
                compressed_response,
                previous=response if link_args else None,
            )
    return shared.with_cargo_plan(plan), reloc.with_cargo_plan(plan)


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
        cargo_command=shared_spec.cargo_plan.command,
        producer_artifact_selection=RUNTIME_WASM_COMBINED_ARTIFACTS,
        members=(
            RuntimeBuildMemberPlan(
                kind="shared",
                resolved_rustflags=shared_spec.fingerprint_rustflags,
                link_args=shared_spec.link_flags,
                publication=RuntimePublicationAction(
                    RuntimePublicationTransform.SHARED,
                    preserve_debug,
                    capture_runtime_publication_executor(
                        root, RuntimePublicationTransform.SHARED
                    ),
                    export_renames=tuple(
                        sorted(wasm_split_runtime_export_rename_map(None).items())
                    ),
                ),
            ),
            RuntimeBuildMemberPlan(
                kind="reloc",
                resolved_rustflags=reloc_spec.fingerprint_rustflags,
                link_args=reloc_spec.link_flags,
                publication=runtime_reloc_publication_action(
                    reloc_spec.runtime_exports,
                    module_name=reloc_spec.publication_name,
                    executor=capture_runtime_publication_executor(
                        root, RuntimePublicationTransform.RELOC
                    ),
                ),
            ),
        ),
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


def _wasm_required_link_features(
    required_exports: set[str] | frozenset[str] | None,
    required_link_features: set[str] | frozenset[str],
) -> frozenset[str]:
    """Close link features over every required export's declared feature."""
    export_link_features = frozenset(
        feature
        for import_name in required_exports or ()
        if (feature := wasm_runtime_import_feature(import_name)) is not None
    )
    return frozenset(required_link_features) | export_link_features


def runtime_wasm_distribution_surface(
    stdlib_profile: str,
) -> tuple[frozenset[str], frozenset[str]]:
    """Required exports and link features of one distributed WASM runtime cell.

    Distribution is the canonical selector with its complete requirement set:
    every CPython C-API link import plus the stdlib tier's feature ceiling.
    """
    return (
        frozenset(wasm_cpython_abi_distribution_export_names()),
        frozenset(profile_link_features(stdlib_profile, target_triple="wasm32-wasip1")),
    )


def runtime_wasm_distribution_features(
    stdlib_profile: str, *, freestanding: bool
) -> tuple[str, ...]:
    """Fingerprint features the distributed cell's identity must record."""
    exports, ceiling = runtime_wasm_distribution_surface(stdlib_profile)
    _no_default, _cargo, fingerprint = _wasm_runtime_feature_plan(
        stdlib_profile=stdlib_profile,
        runtime_features=("wasm_freestanding",) if freestanding else (),
        builtin_features=(),
        resolved_modules=None,
        required_link_features=_wasm_required_link_features(exports, ceiling),
    )
    return fingerprint


def _compute_runtime_wasm_build_spec(
    root: Path,
    runtime_wasm: Path,
    *,
    reloc: bool,
    build_profile: BuildProfile,
    cargo_profile: str,
    simd_enabled: bool,
    freestanding: bool,
    stdlib_profile: str | None,
    resolved_modules: set[str] | frozenset[str] | None,
    required_link_features: frozenset[str],
    required_exports: set[str] | frozenset[str] | None,
    full_export_surface: bool = False,
) -> _RuntimeWasmBuildSpec:
    """Resolve the mode-specific runtime-wasm build spec (see _RuntimeWasmBuildSpec)."""
    # The emitted app import ABI is the final link-time requirement authority.
    # Reachability-derived features normally predict this set, but external
    # native objects and runtime-support module initializers can add imports
    # after that earlier scan.  Project every required export through the same
    # generated symbol->feature authority and close the feature plan here, so
    # Cargo can never build an artifact that the immediately following export
    # validator proves insufficient.
    required_link_features = _wasm_required_link_features(
        required_exports, required_link_features
    )
    requested_cargo_profile = cargo_profile
    cargo_profile = _resolve_wasm_cargo_profile(cargo_profile)
    profile_dir = cargo_profile_directory(cargo_profile)
    build_policy = resolve_runtime_wasm_build_policy(build_profile, os.environ)
    env = _cargo_build_env()
    if build_policy.incrementality is RuntimeWasmIncrementality.FORCED_OFF:
        env["CARGO_INCREMENTAL"] = "0"
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
        runtime_exports = (
            wasm_runtime_shared_export_link_args(required_exports)
            if full_export_surface
            else wasm_runtime_export_link_args(
                required_exports,
                resolved_modules=resolved_modules,
            )
        )
        link_flags = runtime_exports
    else:
        runtime_exports = wasm_runtime_shared_export_link_args(required_exports)
        shared_import_flags = (
            "-C",
            "link-arg=--import-memory",
            "-C",
            "link-arg=--import-table",
            "-C",
            "link-arg=--growable-table",
            "-C",
            f"link-arg=--table-base={1 + WASM_RESERVED_RUNTIME_CALLABLE_BASE + 2 * WASM_RESERVED_RUNTIME_CALLABLE_COUNT}",
        )
        link_flags = (*shared_import_flags, *runtime_exports)
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
    if build_policy.layout is RuntimeWasmTargetLayout.FAMILY:
        target_root = _runtime_wasm_family_target_root(
            root,
            _runtime_wasm_family_key(
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
        publication_name=runtime_wasm.name,
        requested_cargo_profile=requested_cargo_profile,
        cargo_profile=cargo_profile,
        profile_dir=profile_dir,
        build_policy=build_policy,
        env=ProcessEnvironment(env),
        artifact_selection=artifact_selection,
        runtime_exports=runtime_exports,
        link_flags=link_flags,
        cargo_rustflags=(),
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
