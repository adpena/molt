from __future__ import annotations

from pathlib import Path
from typing import Any, Mapping, Sequence

from molt.cli import build_inputs as _build_inputs
from molt.cli.config_resolution import DEFAULT_RUNTIME_STDLIB_PROFILE
from molt.cli.backend_cache import (
    _backend_cache_artifact_path,
    _encode_stdlib_module_symbols,
    _native_stdlib_object_split_enabled,
    _shared_stdlib_cache_key,
    _shared_stdlib_manifest,
    _stdlib_module_symbols,
    _stdlib_object_cache_path,
    _try_cached_backend_candidates,
    _validate_shared_stdlib_cache_contract,
)
from molt.cli.backend_execution import (
    _backend_bin_path,
    _backend_binary_identity,
    _backend_codegen_env_digest,
    _backend_features_for_build_target,
)
from molt.cli.build_output_layout import _resolve_cache_root
from molt.cli.cache_keys import (
    _cache_backend_payload_ir,
    _cache_ir_payload_ir,
    _cache_key,
    _function_cache_key,
)
from molt.cli.models import (
    _BackendCacheSetup,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _ExternalPackageNativeArtifactPlan,
    _ModuleGraphMetadata,
)
from molt.cli.runtime_paths import _normalize_runtime_stdlib_profile
from molt.cli.target_python import TargetPythonVersion


def _build_cache_variant(
    *,
    profile: str,
    runtime_cargo: str,
    backend_cargo: str,
    emit: str,
    stdlib_split: bool,
    codegen_env: str,
    linked: bool,
    target_python: TargetPythonVersion,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    partition_mode: bool = False,
    backend_binary_identity: str = "",
    external_static_packages_digest: str = "",
    runtime_intrinsic_symbols_digest: str = "",
    capability_config_digest: str = "",
) -> str:
    """Build a cache variant key from build configuration.

    Changes to any parameter produce a different variant, ensuring cache
    entries for different build configurations never collide.

    ``stdlib_profile`` MUST be the concrete runtime artifact tier selected from
    the user-facing intent before cache setup. It is part of the variant because
    each concrete tier compiles the molt-runtime hub with different Cargo
    features. Two builds whose reachable stdlib IR happens to be identical would
    otherwise collide on the same ``stdlib_shared.o`` (and main backend object),
    so a smaller-tier build could silently reuse a larger-tier object and vice
    versa - a stale cache hit that yields the wrong runtime surface or a
    duplicate/missing-symbol link.

    ``backend_binary_identity`` MUST be part of the variant: it is the stat-based
    identity (path + mtime + size) of the backend binary that will compile these
    objects (see ``_backend_binary_identity``). The variant flows into every
    ``.o`` cache key (stdlib-shared, module, per-function), so binding it here
    makes the cache key change whenever the backend binary changes — closing the
    Finding #4 (design 20 §4.1) confound where a rebuilt backend with different
    codegen silently linked stale objects compiled by the prior binary. The
    backend *source-tree* fingerprint (``_cache_fingerprint``) does not catch
    this when source mtimes are reset by git/worktree ops or when two
    same-source builds produce different binaries.

    ``runtime_intrinsic_symbols_digest`` MUST be part of native binary cache
    identity because the app object embeds the per-app intrinsic resolver. The
    resolver's relocation set is computed against the linked runtime staticlib's
    exact `molt_*` symbol authority; a stale app object emitted against a
    different set can either miss required intrinsics or reference absent ones.
    """
    parts = [
        f"profile={profile}",
        f"runtime_cargo={runtime_cargo}",
        f"backend_cargo={backend_cargo}",
        f"emit={emit}",
        f"stdlib_split={int(stdlib_split)}",
        f"stdlib_profile={_normalize_runtime_stdlib_profile(stdlib_profile)}",
        f"codegen_env={codegen_env}",
        f"target_python={target_python.tag}",
    ]
    if linked:
        parts.append("linked=1")
    if partition_mode:
        parts.append("partitioned=v1")
    if backend_binary_identity:
        parts.append(f"backend_bin={backend_binary_identity}")
    if external_static_packages_digest:
        parts.append(f"external_static_packages={external_static_packages_digest}")
    if runtime_intrinsic_symbols_digest:
        parts.append(f"runtime_intrinsics={runtime_intrinsic_symbols_digest}")
    if capability_config_digest:
        parts.append(f"capability_config={capability_config_digest}")
    return ";".join(parts)


def _prepare_backend_cache_setup(
    *,
    cache_enabled: bool,
    ir: Mapping[str, Any],
    target: str,
    target_triple: str | None,
    profile: str,
    runtime_cargo_profile: str,
    backend_cargo_profile: str,
    emit_mode: str,
    is_wasm: bool,
    linked: bool,
    project_root: Path,
    cache_dir: str | None,
    output_artifact: Path,
    warnings: list[str],
    entry_module: str,
    module_graph_metadata: _ModuleGraphMetadata,
    target_python: TargetPythonVersion,
    stdlib_profile: str | None = DEFAULT_RUNTIME_STDLIB_PROFILE,
    native_artifact_plan: _ExternalPackageNativeArtifactPlan = (
        _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
    ),
    runtime_intrinsic_symbols_digest: str = "",
    capabilities_list: Sequence[str] | None = None,
    capability_profiles: Sequence[str] | None = None,
    manifest_env_vars: Mapping[str, str] | None = None,
    capability_config_digest: str | None = None,
) -> _BackendCacheSetup:
    split_stdlib_object = _native_stdlib_object_split_enabled(
        target=target,
        emit_mode=emit_mode,
    )
    stdlib_module_symbols = _stdlib_module_symbols(module_graph_metadata)
    stdlib_module_symbols_json = (
        _encode_stdlib_module_symbols(stdlib_module_symbols)
        if split_stdlib_object
        else None
    )
    # Bind the cache key to the backend binary the daemon will run, so a rebuilt
    # backend with different codegen never silently reuses .o objects compiled by
    # the prior binary (Finding #4, design 20 §4.1). Resolve the binary path via
    # the same feature mapping the build dispatch uses, so the stamped identity
    # matches the actual daemon executable for this target/profile.
    backend_bin = _backend_bin_path(
        project_root,
        backend_cargo_profile,
        _backend_features_for_build_target(target=target, is_wasm=is_wasm),
    )
    backend_binary_identity = _backend_binary_identity(backend_bin)
    if capability_config_digest is None:
        capability_config_digest = _build_inputs._capability_config_cache_digest(
            capabilities_list=capabilities_list,
            capability_profiles=capability_profiles,
            manifest_env_vars=manifest_env_vars,
        )
    cache_variant = _build_cache_variant(
        profile=profile,
        runtime_cargo=runtime_cargo_profile,
        backend_cargo=backend_cargo_profile,
        emit=emit_mode,
        stdlib_split=split_stdlib_object,
        codegen_env=_backend_codegen_env_digest(is_wasm=is_wasm),
        linked=linked,
        target_python=target_python,
        stdlib_profile=stdlib_profile,
        backend_binary_identity=backend_binary_identity,
        external_static_packages_digest=native_artifact_plan.digest(),
        runtime_intrinsic_symbols_digest=runtime_intrinsic_symbols_digest,
        capability_config_digest=capability_config_digest,
    )
    if not cache_enabled:
        # Even with cache disabled, compute stdlib_object_path so the
        # daemon can partition stdlib functions into stdlib_shared.o and
        # the linker can resolve them.  Without this, the daemon strips
        # stdlib functions but the linker never sees stdlib_shared.o.
        _nocache_stdlib_path = None
        _nocache_stdlib_key = None
        _nocache_stdlib_manifest = None
        if split_stdlib_object:
            _nocache_stdlib_key = _shared_stdlib_cache_key(
                ir,
                entry_module=entry_module,
                stdlib_module_symbols=stdlib_module_symbols,
                target_triple=target_triple,
                cache_variant=cache_variant,
            )
            _nocache_stdlib_manifest = _shared_stdlib_manifest(
                cache_key=_nocache_stdlib_key,
                cache_variant=cache_variant,
                target_triple=target_triple,
            )
            _nocache_cache_root = _resolve_cache_root(project_root, cache_dir)
            try:
                _nocache_cache_root.mkdir(parents=True, exist_ok=True)
            except OSError:
                pass
            _nocache_stub_path = _nocache_cache_root / "__nocache__.o"
            _nocache_stdlib_path = _stdlib_object_cache_path(
                _nocache_stub_path, _nocache_stdlib_key
            )
            if _nocache_stdlib_path is not None:
                _validate_shared_stdlib_cache_contract(
                    _nocache_stdlib_path,
                    project_root,
                    _nocache_stdlib_key,
                    expected_manifest=_nocache_stdlib_manifest,
                    target_triple=target_triple,
                    stdlib_module_symbols=stdlib_module_symbols,
                )
        return _BackendCacheSetup(
            cache_enabled=False,
            cache_key=None,
            function_cache_key=None,
            cache_path=None,
            function_cache_path=None,
            stdlib_object_path=_nocache_stdlib_path,
            stdlib_object_cache_key=_nocache_stdlib_key,
            stdlib_object_manifest=_nocache_stdlib_manifest,
            cache_candidates=(),
            cache_hit=False,
            cache_hit_tier=None,
            stdlib_module_symbols_json=stdlib_module_symbols_json,
            stdlib_module_symbols=frozenset(stdlib_module_symbols),
        )
    module_cache_payload_ir = _cache_ir_payload_ir(ir)
    backend_cache_payload_ir = _cache_backend_payload_ir(ir)
    cache_key = _cache_key(
        ir,
        target,
        target_triple,
        cache_variant,
        payload_ir=module_cache_payload_ir,
    )
    function_cache_key = _function_cache_key(
        ir,
        target,
        target_triple,
        cache_variant,
        payload_ir=backend_cache_payload_ir,
    )
    cache_root = _resolve_cache_root(project_root, cache_dir)
    try:
        cache_root.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        warnings.append(f"Cache disabled: {exc}")
        return _BackendCacheSetup(
            cache_enabled=False,
            cache_key=cache_key,
            function_cache_key=function_cache_key,
            cache_path=None,
            function_cache_path=None,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            stdlib_object_manifest=None,
            cache_candidates=(),
            cache_hit=False,
            cache_hit_tier=None,
            stdlib_module_symbols_json=stdlib_module_symbols_json,
            stdlib_module_symbols=frozenset(stdlib_module_symbols),
        )
    stdlib_object_path = None
    stdlib_object_cache_key = None
    stdlib_object_manifest = None
    if split_stdlib_object:
        stdlib_object_cache_key = _shared_stdlib_cache_key(
            ir,
            entry_module=entry_module,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
            cache_variant=cache_variant,
        )
        stdlib_object_manifest = _shared_stdlib_manifest(
            cache_key=stdlib_object_cache_key,
            cache_variant=cache_variant,
            target_triple=target_triple,
        )
    ext = "wasm" if is_wasm else "o"
    cache_path = _backend_cache_artifact_path(
        cache_root,
        cache_key,
        ext=ext,
        stdlib_object_cache_key=stdlib_object_cache_key,
        is_wasm=is_wasm,
    )
    function_cache_path = None
    if function_cache_key and function_cache_key != cache_key:
        function_cache_path = _backend_cache_artifact_path(
            cache_root,
            function_cache_key,
            ext=ext,
            stdlib_object_cache_key=stdlib_object_cache_key,
            is_wasm=is_wasm,
        )
    if split_stdlib_object and stdlib_object_cache_key is not None:
        assert cache_path is not None
        stdlib_object_path = _stdlib_object_cache_path(
            cache_path, stdlib_object_cache_key
        )
        if stdlib_object_path is not None:
            _validate_shared_stdlib_cache_contract(
                stdlib_object_path,
                project_root,
                stdlib_object_cache_key,
                expected_manifest=stdlib_object_manifest,
                target_triple=target_triple,
                stdlib_module_symbols=stdlib_module_symbols,
            )
    cache_candidates: list[tuple[str, Path]] = []
    if cache_path is not None:
        cache_candidates.append(("module", cache_path))
    if function_cache_path is not None and function_cache_path != cache_path:
        cache_candidates.append(("function", function_cache_path))
    cache_hit, cache_hit_tier = _try_cached_backend_candidates(
        project_root=project_root,
        cache_candidates=cache_candidates,
        output_artifact=output_artifact,
        is_wasm=is_wasm,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        cache_path=cache_path,
        stdlib_object_path=stdlib_object_path,
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        warnings=warnings,
    )
    return _BackendCacheSetup(
        cache_enabled=True,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        cache_path=cache_path,
        function_cache_path=function_cache_path,
        stdlib_object_path=stdlib_object_path,
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        cache_candidates=tuple(cache_candidates),
        cache_hit=cache_hit,
        cache_hit_tier=cache_hit_tier,
        stdlib_module_symbols_json=stdlib_module_symbols_json,
        stdlib_module_symbols=frozenset(stdlib_module_symbols),
    )
