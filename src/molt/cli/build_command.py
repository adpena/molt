"""``molt build`` as a library call.

`build` checks the request, then runs the three build stages: input
preparation (`build_inputs`), the frontend (`frontend_pipeline`) and the
backend and link (`build_pipeline`). It imports all three, so only commands
that build load it; `molt.cli` re-exports `build` lazily.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Mapping

from molt.backend_environment import CodegenSelection
from molt.backend_executable_names import DEFAULT_CODEGEN_BACKEND
from molt.cli import build_inputs as _build_inputs
from molt.cli import build_pipeline as _build_pipeline
from molt.cli import frontend_pipeline as _frontend_pipeline
from molt.cli.cache_fingerprints import _source_tree_fingerprint_transaction
from molt.cli.config_resolution import _resolve_build_config, resolve_stdlib_profile
from molt.cli.output import fail as _fail

if TYPE_CHECKING:
    from molt.backend_executable_names import CodegenBackend
    from molt.capability_policy import CapabilityInput
    from molt.cli import factgraph as _factgraph
    from molt.cli.models import (
        BuildProfile,
        EmitMode,
        FallbackPolicy,
        ParseCodec,
        Target,
        TypeHintPolicy,
    )


def build(
    file_path: str | None,
    target: Target = "native",
    parse_codec: ParseCodec = "msgpack",
    type_hint_policy: TypeHintPolicy = "check",
    fallback_policy: FallbackPolicy = "error",
    type_facts_path: str | None = None,
    pgo_profile: str | None = None,
    runtime_feedback: str | None = None,
    output: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    cache: bool = True,
    cache_dir: str | None = None,
    cache_report: bool = False,
    sysroot: str | None = None,
    emit_ir: str | None = None,
    emit: EmitMode | None = None,
    out_dir: str | None = None,
    profile: BuildProfile = "release",
    linked: bool = False,
    linked_output: str | None = None,
    require_linked: bool = False,
    respect_pythonpath: bool = False,
    module: str | None = None,
    diagnostics: bool | None = None,
    diagnostics_file: str | None = None,
    diagnostics_verbosity: str | None = None,
    portable: bool = False,
    wasm_opt_level: str = "Oz",
    precompile: bool = False,
    wasm_profile: str = "auto",
    snapshot: bool = False,
    stdlib_profile: str | None = None,
    tree_shake: bool = True,
    lib_paths: list[str] | None = None,
    split_runtime: bool = False,
    capability_manifest: str | None = None,
    require_signed_manifest: bool = False,
    audit_log: str | None = None,
    io_mode: str | None = None,
    python_version: str | None = None,
    build_config: Mapping[str, Any] | None = None,
    bolt: bool = False,
    bolt_training_cmd: str | None = None,
    fact_graph_request: _factgraph.FactGraphRequest | None = None,
    codegen_backend: CodegenBackend = DEFAULT_CODEGEN_BACKEND,
) -> int:
    if isinstance(profile, bool):
        profile = "release"
    if profile not in {"dev", "release"}:
        return _fail(f"Invalid build profile: {profile}", json_output, command="build")
    if bolt_training_cmd is not None and not bolt:
        return _fail(
            "BOLT training command requires BOLT optimization.",
            json_output,
            command="build",
        )
    if bolt and profile != "release":
        return _fail(
            "BOLT requires the release build profile.",
            json_output,
            command="build",
        )
    if bolt and target in {"wasm", "wasm-freestanding", "rust", "luau", "mlir"}:
        return _fail(
            f"BOLT requires a native Linux ELF target, not {target!r}.",
            json_output,
            command="build",
        )
    # Resolve `stdlib_profile` through the ONE config authority. The CLI
    # dispatcher already resolves it before calling `build()`, but direct
    # library callers may pass `None`; resolving here (honoring the flag value,
    # `MOLT_STDLIB_PROFILE`, the `[tool.molt.build]` config, and the single
    # default) guarantees one concrete value that is passed both to module-graph
    # construction and to the runtime build, so the closure reader and the
    # staticlib selector can never disagree.
    stdlib_profile, _ = resolve_stdlib_profile(
        flag=stdlib_profile,
        build_cfg=(
            _resolve_build_config(dict(build_config))
            if build_config is not None
            else None
        ),
    )
    # Every value this call resolves travels as a parameter. The backend
    # process receives its share in its own environment mapping
    # (CodegenSelection.environment); build() never writes os.environ, so an
    # in-process caller (library use, the batch build server, a test) cannot
    # leak one build's choices into later work.
    codegen = CodegenSelection(
        backend=codegen_backend,
        portable=portable,
        wasm_profile=(
            wasm_profile
            if target in {"wasm", "wasm-freestanding"} and wasm_profile
            else None
        ),
    )
    with _source_tree_fingerprint_transaction():
        if file_path and module:
            return _fail(
                "Use a file path or --module, not both.", json_output, command="build"
            )
        prepared_build_inputs, prepared_build_inputs_error = (
            _build_inputs._prepare_build_inputs(
                file_path=file_path,
                module=module,
                diagnostics=diagnostics,
                diagnostics_file=diagnostics_file,
                diagnostics_verbosity=diagnostics_verbosity,
                json_output=json_output,
                target=target,
                deterministic=deterministic,
                deterministic_warn=deterministic_warn,
                sysroot=sysroot,
                profile=profile,
                pgo_profile=pgo_profile,
                runtime_feedback=runtime_feedback,
                capabilities=capabilities,
                capability_manifest=capability_manifest,
                require_signed_manifest=require_signed_manifest,
                trusted=trusted,
                audit_log=audit_log,
                io_mode=io_mode,
                respect_pythonpath=respect_pythonpath,
                lib_paths=lib_paths or [],
                python_version=python_version,
                build_config=build_config,
                codegen=codegen,
            )
        )
        if prepared_build_inputs_error is not None:
            return prepared_build_inputs_error
        assert prepared_build_inputs is not None
        (
            prepared_build_preamble,
            prepared_build_roots,
            prepared_build_config,
            resolved_build_entry,
        ) = prepared_build_inputs
        prepared_frontend_pipeline_bundle, prepared_frontend_pipeline_error = (
            _frontend_pipeline._prepare_frontend_pipeline(
                prepared_build_preamble=prepared_build_preamble,
                prepared_build_roots=prepared_build_roots,
                prepared_build_config=prepared_build_config,
                resolved_build_entry=resolved_build_entry,
                parse_codec=parse_codec,
                type_hint_policy=type_hint_policy,
                fallback_policy=fallback_policy,
                profile=profile,
                json_output=json_output,
                target=target,
                verbose=verbose,
                out_dir=out_dir,
                trusted=trusted,
                split_runtime=split_runtime,
                require_linked=require_linked,
                linked=linked,
                linked_output=linked_output,
                emit=emit,
                output=output,
                emit_ir=emit_ir,
                type_facts_path=type_facts_path,
                tree_shake=tree_shake,
                stdlib_profile=stdlib_profile,
            )
        )
        if prepared_frontend_pipeline_error is not None:
            return prepared_frontend_pipeline_error
        assert prepared_frontend_pipeline_bundle is not None
        return _build_pipeline._run_build_pipeline(
            prepared_build_preamble=prepared_build_preamble,
            prepared_build_roots=prepared_build_roots,
            prepared_build_config=prepared_build_config,
            resolved_build_entry=resolved_build_entry,
            prepared_frontend_pipeline_bundle=prepared_frontend_pipeline_bundle,
            profile=profile,
            json_output=json_output,
            target=target,
            cache_dir=cache_dir,
            cache=cache,
            cache_report=cache_report,
            deterministic=deterministic,
            trusted=trusted,
            verbose=verbose,
            require_linked=require_linked,
            wasm_opt_level=wasm_opt_level,
            precompile=precompile,
            snapshot=snapshot,
            stdlib_profile=stdlib_profile,
            bolt_requested=bolt,
            bolt_training_cmd=bolt_training_cmd,
            fact_graph_request=fact_graph_request,
        )
