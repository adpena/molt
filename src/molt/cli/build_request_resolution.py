"""Pure effective build intent from the shared command parser and configuration."""

from __future__ import annotations
import argparse
from functools import lru_cache
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any
from molt.cli.build_request import BuildRequest
from molt.cli.run_tier import RunTier, jit_tier_refusal, jit_tier_refusal_message
from molt.compilation_target import TargetKind
from molt.cli.build_output_layout import (
    _BUILD_PROFILE_CHOICES,
    _DEPLOY_PROFILE_DEFAULTS,
)
from molt.cli.config_resolution import (
    _coerce_bool,
    _select_capability_input,
    _select_codegen_backend,
    resolve_stdlib_profile,
    _load_molt_config,
    _resolve_build_config,
    _resolve_capabilities_config,
)
from molt.wasm_optimization import WASM_OPT_DEV_DEFAULT, WASM_OPT_RELEASE_DEFAULT


_DEFAULT_REQUEST = BuildRequest(file_path=None)


def _configured_build_target(target: str | None, build_cfg: Mapping[str, Any]) -> str:
    if target is not None:
        return target
    return build_cfg.get("target", TargetKind.NATIVE.value)


def resolve_build_request(
    args: argparse.Namespace,
    *,
    build_cfg: Mapping[str, Any],
    cfg_capabilities: Any,
    env: Mapping[str, str],
) -> BuildRequest:
    target = _configured_build_target(args.target, build_cfg)
    codec = args.codec or build_cfg.get("codec") or _DEFAULT_REQUEST.parse_codec
    type_hints = (
        args.type_hints
        or build_cfg.get("type_hints")
        or _DEFAULT_REQUEST.type_hint_policy
    )
    output = args.output or build_cfg.get("output")
    out_dir = args.out_dir or build_cfg.get("out_dir") or build_cfg.get("out-dir")
    sysroot = (
        args.sysroot
        or build_cfg.get("sysroot")
        or build_cfg.get("sysroot_path")
        or build_cfg.get("sysroot-path")
    )
    emit = args.emit or build_cfg.get("emit")
    emit_ir = args.emit_ir or build_cfg.get("emit_ir") or build_cfg.get("emit-ir")
    pgo_profile = (
        args.pgo_profile or build_cfg.get("pgo_profile") or build_cfg.get("pgo-profile")
    )
    runtime_feedback = (
        args.runtime_feedback
        or build_cfg.get("runtime_feedback")
        or build_cfg.get("runtime-feedback")
    )
    profile_arg = getattr(args, "profile", None)
    platform_arg = getattr(args, "platform", None)
    cli_profile_build_profile: str | None = None
    deploy_profile: str | None = None
    if profile_arg in _BUILD_PROFILE_CHOICES:
        cli_profile_build_profile = profile_arg
    elif profile_arg in _DEPLOY_PROFILE_DEFAULTS:
        deploy_profile = profile_arg
    elif profile_arg is not None:
        raise ValueError(f"Invalid build profile or platform profile: {profile_arg}")
    if platform_arg is not None:
        if deploy_profile is not None and deploy_profile != platform_arg:
            raise ValueError(
                f"Conflicting deployment profiles: --profile {deploy_profile} and --platform {platform_arg}"
            )
        deploy_profile = platform_arg
    if (
        cli_profile_build_profile is not None
        and args.build_profile is not None
        and (cli_profile_build_profile != args.build_profile)
    ):
        raise ValueError(
            f"Conflicting build profiles: --profile {cli_profile_build_profile} and --build-profile {args.build_profile}"
        )
    cli_build_profile = args.build_profile or cli_profile_build_profile
    if (
        getattr(args, "release", False)
        and cli_build_profile is not None
        and (cli_build_profile != "release")
    ):
        raise ValueError(
            f"Conflicting build profiles: --release and {cli_build_profile}"
        )
    build_profile = (
        ("release" if getattr(args, "release", False) else None)
        or cli_build_profile
        or build_cfg.get("profile")
        or build_cfg.get("build_profile")
        or _DEFAULT_REQUEST.profile
    )
    backend_choice = getattr(args, "backend", "auto") or "auto"
    linked_output_path = (
        args.linked_output
        or build_cfg.get("linked_output")
        or build_cfg.get("linked-output")
    )
    require_linked = args.require_linked
    if require_linked is None:
        require_linked = _coerce_bool(
            build_cfg.get("require_linked") or build_cfg.get("require-linked"), False
        )
    type_facts = args.type_facts or build_cfg.get("type_facts")
    deterministic = (
        args.deterministic
        if args.deterministic is not None
        else _coerce_bool(
            build_cfg.get("deterministic"), _DEFAULT_REQUEST.deterministic
        )
    )
    deterministic_warn = args.deterministic_warn
    if deterministic_warn is None:
        deterministic_warn = _coerce_bool(
            build_cfg.get("deterministic_warn") or build_cfg.get("deterministic-warn"),
            False,
        )
    trusted = args.trusted
    if trusted is None:
        trusted = _coerce_bool(build_cfg.get("trusted"), False)
    linked = args.linked
    if linked is None:
        linked = _coerce_bool(build_cfg.get("linked"), False)
    cache = (
        args.cache
        if args.cache is not None
        else _coerce_bool(build_cfg.get("cache"), _DEFAULT_REQUEST.cache)
    )
    if args.rebuild:
        cache = False
    cache_dir = (
        args.cache_dir or build_cfg.get("cache_dir") or build_cfg.get("cache-dir")
    )
    cache_report = args.cache_report or _coerce_bool(
        build_cfg.get("cache_report") or build_cfg.get("cache-report"), False
    )
    respect_pythonpath = args.respect_pythonpath
    if respect_pythonpath is None:
        respect_pythonpath = _coerce_bool(
            build_cfg.get("respect_pythonpath") or build_cfg.get("respect-pythonpath"),
            False,
        )
    diagnostics = args.diagnostics
    if diagnostics is None:
        diagnostics_cfg = build_cfg.get("diagnostics")
        if diagnostics_cfg is None:
            diagnostics_cfg = build_cfg.get("build_diagnostics")
        if diagnostics_cfg is None:
            diagnostics_cfg = build_cfg.get("build-diagnostics")
        if diagnostics_cfg is not None:
            diagnostics = _coerce_bool(diagnostics_cfg, False)
    diagnostics_file_raw = (
        args.diagnostics_file
        or build_cfg.get("diagnostics_file")
        or build_cfg.get("diagnostics-file")
        or build_cfg.get("build_diagnostics_file")
        or build_cfg.get("build-diagnostics-file")
    )
    diagnostics_file = (
        diagnostics_file_raw.strip() if isinstance(diagnostics_file_raw, str) else None
    )
    if diagnostics_file == "":
        diagnostics_file = None
    diagnostics_verbosity = (
        args.diagnostics_verbosity
        or build_cfg.get("diagnostics_verbosity")
        or build_cfg.get("diagnostics-verbosity")
        or build_cfg.get("build_diagnostics_verbosity")
        or build_cfg.get("build-diagnostics-verbosity")
    )
    capabilities = _select_capability_input(
        args.capabilities, build_cfg.get("capabilities"), cfg_capabilities
    )
    cfg_lib_paths = build_cfg.get("lib_paths") or build_cfg.get("lib-paths") or []
    if isinstance(cfg_lib_paths, str):
        cfg_lib_paths = [cfg_lib_paths]
    lib_paths: list[str] = list(args.lib_path) + list(cfg_lib_paths)
    if args.file and args.module:
        raise ValueError("Use a file path or --module, not both.")
    wasm_opt_level_raw = getattr(args, "wasm_opt_level", None)
    wasm_opt_level = (
        wasm_opt_level_raw
        if isinstance(wasm_opt_level_raw, str)
        else WASM_OPT_DEV_DEFAULT
        if build_profile == "dev"
        else WASM_OPT_RELEASE_DEFAULT
    )
    precompile = bool(getattr(args, "precompile", False))
    wasm_profile_raw = getattr(args, "wasm_profile", "auto")
    wasm_profile = (
        wasm_profile_raw
        if isinstance(wasm_profile_raw, str)
        else _DEFAULT_REQUEST.wasm_profile
    )
    stdlib_profile_raw = getattr(args, "stdlib_profile", None)
    stdlib_profile_flag = (
        stdlib_profile_raw if isinstance(stdlib_profile_raw, str) else None
    )
    deploy_defaults_for_profile = (
        _DEPLOY_PROFILE_DEFAULTS.get(deploy_profile)
        if deploy_profile is not None
        else None
    )
    split_runtime = bool(
        getattr(args, "split_runtime", False)
        or (
            deploy_defaults_for_profile is not None
            and deploy_defaults_for_profile.get("split_runtime", False)
        )
    )
    stdlib_profile, _stdlib_profile_source = resolve_stdlib_profile(
        flag=stdlib_profile_flag,
        build_cfg=build_cfg,
        deploy_defaults=deploy_defaults_for_profile,
        env=env,
    )
    if deploy_profile and deploy_profile in _DEPLOY_PROFILE_DEFAULTS:
        defaults = _DEPLOY_PROFILE_DEFAULTS[deploy_profile]
        if wasm_opt_level_raw is None and build_profile == "release":
            default_wasm_opt_level = defaults.get("wasm_opt_level")
            if isinstance(default_wasm_opt_level, str):
                wasm_opt_level = default_wasm_opt_level
        if args.precompile is None:
            precompile = bool(defaults.get("precompile", precompile))
        if args.wasm_profile is None:
            default_wasm_profile = defaults.get("wasm_profile")
            if isinstance(default_wasm_profile, str):
                wasm_profile = default_wasm_profile
    selected_target, codegen_backend, backend_error = _select_codegen_backend(
        target, backend_choice
    )
    if backend_error is not None:
        raise ValueError(backend_error)
    if emit == RunTier.JIT.build_emit:
        # The JIT tier's one admission, before any frontend work.
        refusal = jit_tier_refusal(
            target=selected_target, codegen_backend=codegen_backend
        )
        if refusal is not None:
            raise ValueError(jit_tier_refusal_message(refusal))
    bolt_requested = getattr(args, "bolt", False)
    return BuildRequest(
        file_path=args.file,
        target=selected_target,
        parse_codec=codec,
        type_hint_policy=type_hints,
        type_facts_path=type_facts,
        pgo_profile=pgo_profile,
        runtime_feedback=runtime_feedback,
        output=output,
        json_output=args.json,
        verbose=args.verbose,
        deterministic=deterministic,
        deterministic_warn=deterministic_warn,
        trusted=trusted,
        capabilities=capabilities,
        cache=cache,
        cache_dir=cache_dir,
        cache_report=cache_report,
        sysroot=sysroot,
        emit_ir=emit_ir,
        emit=emit,
        out_dir=out_dir,
        profile=build_profile,
        linked=linked,
        linked_output=linked_output_path,
        require_linked=require_linked,
        respect_pythonpath=respect_pythonpath,
        module=args.module,
        diagnostics=diagnostics,
        capture_link_inputs=args.capture_link_inputs,
        diagnostics_file=diagnostics_file,
        diagnostics_verbosity=diagnostics_verbosity,
        portable=getattr(args, "portable", False),
        wasm_opt_level=wasm_opt_level,
        precompile=precompile,
        wasm_profile=wasm_profile,
        snapshot=getattr(args, "snapshot", False),
        stdlib_profile=stdlib_profile,
        lib_paths=lib_paths or None,
        split_runtime=split_runtime,
        capability_manifest=getattr(args, "capability_manifest", None),
        require_signed_manifest=getattr(args, "require_signed_manifest", False),
        audit_log=getattr(args, "audit_log", None),
        io_mode=getattr(args, "io_mode", None),
        python_version=getattr(args, "python_version", None),
        build_config=build_cfg,
        bolt=bolt_requested,
        bolt_training_cmd=getattr(args, "bolt_training_cmd", None),
        codegen_backend=codegen_backend,
    )


@lru_cache(maxsize=1)
def _build_argument_parser() -> argparse.ArgumentParser:
    from molt.cli.entrypoint_parser import _build_build_parser

    return _build_build_parser()


def parse_build_arguments(
    build_args: Sequence[str],
    *,
    file_path: str | None = None,
    module: str | None = None,
) -> argparse.Namespace:
    parser = _build_argument_parser()
    argv = ["build", *build_args]
    if module is not None:
        argv.extend(["--module", module])
    if file_path is not None:
        argv.append(file_path)
    try:
        args = parser.parse_args(argv)
    except SystemExit as exc:
        raise ValueError("Invalid build arguments") from exc
    return args


def resolve_build_arguments(
    build_args: Sequence[str],
    *,
    file_path: str | None,
    module: str | None,
    project_root: Path,
    env: Mapping[str, str],
) -> BuildRequest:
    """Read configuration once and parse with the full public build grammar."""
    args = parse_build_arguments(build_args, file_path=file_path, module=module)
    config = _load_molt_config(project_root)
    return resolve_build_request(
        args,
        build_cfg=_resolve_build_config(config),
        cfg_capabilities=_resolve_capabilities_config(config),
        env=env,
    )
