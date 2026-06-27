from __future__ import annotations

import argparse
import contextlib
import os
import sys
from pathlib import Path
from typing import Any, Callable, Literal, Mapping, Sequence, cast

from molt.cli import commands as _commands
from molt.cli import debug_helpers as _debug_helpers
from molt.cli import factgraph as _factgraph
from molt.cli import typecheck as _typecheck
from molt.cli.arg_helpers import (
    _build_args_has_cache_flag,
    _strip_leading_double_dash,
    completion,
)
from molt.cli.build_output_layout import (
    _BUILD_PROFILE_CHOICES,
    _DEPLOY_PROFILE_DEFAULTS,
)
from molt.cli.config_resolution import _coerce_bool, resolve_stdlib_profile
from molt.cli.deps import deps, install, install_add, vendor
from molt.cli.dx_cli import handle_dx_command
from molt.cli.extension_audit import extension_audit
from molt.cli.extension_scan import extension_scan
from molt.cli.maintenance import clean, show_config
from molt.cli.models import BuildProfile
from molt.cli.native_toolchain import _run_bolt_post_link
from molt.cli.output import fail as _fail
from molt.cli.package_distribution import package, publish, verify
from molt.cli.package_registry import _is_remote_registry
from molt.cli.target_python import _parse_target_python_version
from molt.cli.setup_readiness import doctor, setup
from molt.cli.toolchain_validation import update_repo, validate
from molt.cli.wrapper_build import _build_args_has_python_version_flag


def _dispatch_entrypoint_command(
    args: argparse.Namespace,
    *,
    build_fn: Callable[..., int],
    config_root: Path,
    config: Mapping[str, Any],
    build_cfg: Mapping[str, Any],
    run_cfg: Mapping[str, Any],
    compare_cfg: Mapping[str, Any],
    test_cfg: Mapping[str, Any],
    diff_cfg: Mapping[str, Any],
    extension_cfg: Mapping[str, Any],
    publish_cfg: Mapping[str, Any],
    cfg_capabilities: Any,
) -> int:
    if args.command == "internal-batch-build-server":
        return _commands._internal_batch_build_server(
            json_output=args.json,
            verbose=args.verbose,
            build_fn=build_fn,
        )

    if args.command == "debug":
        return _debug_helpers._handle_debug_command(args)

    if args.command == "build":
        target = args.target or build_cfg.get("target") or "native"
        codec = args.codec or build_cfg.get("codec") or "msgpack"
        type_hints = args.type_hints or build_cfg.get("type_hints") or "check"
        fallback = args.fallback or build_cfg.get("fallback") or "error"
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
            args.pgo_profile
            or build_cfg.get("pgo_profile")
            or build_cfg.get("pgo-profile")
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
            return _fail(
                f"Invalid build profile or platform profile: {profile_arg}",
                args.json,
                command="build",
            )
        if platform_arg is not None:
            if deploy_profile is not None and deploy_profile != platform_arg:
                return _fail(
                    "Conflicting deployment profiles: "
                    f"--profile {deploy_profile} and --platform {platform_arg}",
                    args.json,
                    command="build",
                )
            deploy_profile = platform_arg
        if (
            cli_profile_build_profile is not None
            and args.build_profile is not None
            and cli_profile_build_profile != args.build_profile
        ):
            return _fail(
                "Conflicting build profiles: "
                f"--profile {cli_profile_build_profile} and "
                f"--build-profile {args.build_profile}",
                args.json,
                command="build",
            )
        cli_build_profile = args.build_profile or cli_profile_build_profile
        if (
            getattr(args, "release", False)
            and cli_build_profile is not None
            and cli_build_profile != "release"
        ):
            return _fail(
                f"Conflicting build profiles: --release and {cli_build_profile}",
                args.json,
                command="build",
            )
        build_profile = (
            ("release" if getattr(args, "release", False) else None)
            or cli_build_profile
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "release"
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
                build_cfg.get("require_linked") or build_cfg.get("require-linked"),
                False,
            )
        type_facts = args.type_facts or build_cfg.get("type_facts")
        deterministic = (
            args.deterministic
            if args.deterministic is not None
            else _coerce_bool(build_cfg.get("deterministic"), True)
        )
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
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
            else _coerce_bool(build_cfg.get("cache"), True)
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
                build_cfg.get("respect_pythonpath")
                or build_cfg.get("respect-pythonpath"),
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
            diagnostics_file_raw.strip()
            if isinstance(diagnostics_file_raw, str)
            else None
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
        capabilities = (
            args.capabilities or build_cfg.get("capabilities") or cfg_capabilities
        )
        cfg_lib_paths = build_cfg.get("lib_paths") or build_cfg.get("lib-paths") or []
        if isinstance(cfg_lib_paths, str):
            cfg_lib_paths = [cfg_lib_paths]
        lib_paths: list[str] = list(args.lib_path) + list(cfg_lib_paths)
        if args.file and args.module:
            return _fail(
                "Use a file path or --module, not both.", args.json, command="build"
            )

        wasm_opt_level_raw = getattr(args, "wasm_opt_level", "Oz")
        wasm_opt_level = (
            wasm_opt_level_raw if isinstance(wasm_opt_level_raw, str) else "Oz"
        )
        precompile = bool(getattr(args, "precompile", False))
        wasm_profile_raw = getattr(args, "wasm_profile", "full")
        wasm_profile = wasm_profile_raw if isinstance(wasm_profile_raw, str) else "full"
        stdlib_profile_raw = getattr(args, "stdlib_profile", None)
        stdlib_profile_flag = (
            stdlib_profile_raw if isinstance(stdlib_profile_raw, str) else None
        )
        deploy_defaults_for_profile = (
            _DEPLOY_PROFILE_DEFAULTS.get(deploy_profile)
            if deploy_profile is not None
            else None
        )
        # `stdlib_profile` is resolved through the ONE config authority
        # (`config_resolution.resolve_stdlib_profile`). The module-graph closure
        # reads `MOLT_STDLIB_PROFILE` directly (`_ensure_core_stdlib_modules`),
        # while the runtime-staticlib selector consumes this resolved value;
        # routing both through one resolver + one default (and re-exporting the
        # result to the env inside `build()`) is what makes a closure/staticlib
        # desync (env-only `full` vs a `micro` staticlib -> link failure on
        # undefined `molt_pbkdf2_hmac`/`molt_scrypt`) unexpressible.
        stdlib_profile, _stdlib_profile_source = resolve_stdlib_profile(
            flag=stdlib_profile_flag,
            build_cfg=build_cfg,
            deploy_defaults=deploy_defaults_for_profile,
        )

        if deploy_profile and deploy_profile in _DEPLOY_PROFILE_DEFAULTS:
            defaults = _DEPLOY_PROFILE_DEFAULTS[deploy_profile]
            # Only apply defaults for arguments that weren't explicitly set
            if args.wasm_opt_level == "Oz" and "wasm_opt_level" not in sys.argv:
                # wasm_opt_level has argparse default "Oz"; check if user passed it
                _wasm_opt_explicitly_set = any(
                    a.startswith("--wasm-opt-level") for a in sys.argv
                )
                if not _wasm_opt_explicitly_set:
                    default_wasm_opt_level = defaults.get("wasm_opt_level")
                    if isinstance(default_wasm_opt_level, str):
                        wasm_opt_level = default_wasm_opt_level
            if not any(a == "--precompile" for a in sys.argv):
                precompile = bool(defaults.get("precompile", precompile))
            if not any(a.startswith("--wasm-profile") for a in sys.argv):
                default_wasm_profile = defaults.get("wasm_profile")
                if isinstance(default_wasm_profile, str):
                    wasm_profile = default_wasm_profile

        # `--target llvm` is an alias for "native binary, LLVM backend": the
        # LLVM backend emits host-native objects, so the runtime staticlib and
        # the entire native link path are identical to `--target native`; the
        # only difference is the codegen backend.  Canonicalize it to the
        # `native` target (so every downstream `target == "native"` branch -
        # runtime triple, stdlib object split, native link driver - fires) and
        # route the backend selection through MOLT_BACKEND below.  Without this,
        # "llvm" leaks into the cargo `--target` slot, which expects a rustc
        # target triple, and the runtime build fails with "could not find
        # specification for target \"llvm\"".
        if target == "llvm":
            if backend_choice not in {"auto", "llvm"}:
                return _fail(
                    "`--target llvm` selects the LLVM backend; it conflicts "
                    f"with `--backend {backend_choice}`. Use `--target native "
                    "--backend llvm` to mix, or drop one flag.",
                    args.json,
                    command="build",
                )
            backend_choice = "llvm"
            target = "native"
        # --backend: resolve effective backend and propagate via MOLT_BACKEND.
        # "auto" defaults to cranelift for all builds. LLVM remains opt-in
        # until its end-to-end parity and operational tooling are on the same
        # footing as the default Cranelift lane.
        effective_backend = backend_choice
        if effective_backend == "auto":
            effective_backend = "cranelift"
        os.environ["MOLT_BACKEND"] = effective_backend

        build_rc = build_fn(
            args.file,
            target,
            codec,
            type_hints,
            fallback,
            type_facts,
            pgo_profile,
            runtime_feedback,
            output,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
            trusted,
            capabilities,
            cache,
            cache_dir,
            cache_report,
            sysroot,
            emit_ir,
            emit,
            out_dir,
            build_profile,
            linked,
            linked_output_path,
            require_linked,
            respect_pythonpath,
            args.module,
            diagnostics,
            diagnostics_file,
            diagnostics_verbosity,
            portable=getattr(args, "portable", False),
            wasm_opt_level=wasm_opt_level,
            precompile=precompile,
            wasm_profile=wasm_profile,
            snapshot=getattr(args, "snapshot", False),
            stdlib_profile=stdlib_profile,
            lib_paths=lib_paths or None,
            split_runtime=getattr(args, "split_runtime", False),
            capability_manifest=getattr(args, "capability_manifest", None),
            require_signed_manifest=getattr(args, "require_signed_manifest", False),
            audit_log=getattr(args, "audit_log", None),
            io_mode=getattr(args, "io_mode", None),
            type_gate=getattr(args, "type_gate", False),
            python_version=getattr(args, "python_version", None),
            build_config=build_cfg,
        )

        # --bolt: post-link BOLT optimization for native targets.
        bolt_requested = getattr(args, "bolt", False)
        if bolt_rc := _run_bolt_post_link(
            bolt_requested=bolt_requested,
            bolt_training_cmd=getattr(args, "bolt_training_cmd", None),
            target=target,
            output=output,
            out_dir=out_dir,
            build_rc=build_rc,
            json_output=args.json,
        ):
            return bolt_rc
        return build_rc
    if args.command == "factgraph":
        return _factgraph.run_factgraph_command(
            args=args,
            build=build_fn,
            build_config=build_cfg,
            config_capabilities=cfg_capabilities,
            coerce_bool=_coerce_bool,
            fail=_fail,
        )
    if args.command == "extension":
        if args.extension_command == "build":
            deterministic = (
                args.deterministic
                if args.deterministic is not None
                else _coerce_bool(extension_cfg.get("deterministic"), True)
            )
            capabilities = (
                args.capabilities
                or extension_cfg.get("capabilities")
                or cfg_capabilities
            )
            molt_abi = (
                args.molt_abi
                or extension_cfg.get("molt_abi")
                or extension_cfg.get("molt-abi")
            )
            return _commands.extension_build(
                project=args.project or extension_cfg.get("project"),
                out_dir=args.out_dir
                or extension_cfg.get("out_dir")
                or extension_cfg.get("out-dir"),
                molt_abi=molt_abi,
                capabilities=capabilities,
                deterministic=deterministic,
                target=args.target or extension_cfg.get("target"),
                json_output=args.json,
                verbose=args.verbose,
            )
        if args.extension_command == "audit":
            require_abi = (
                args.require_abi
                or extension_cfg.get("require_abi")
                or extension_cfg.get("require-abi")
            )
            require_capabilities = args.require_capabilities
            if not require_capabilities:
                require_capabilities = _coerce_bool(
                    extension_cfg.get("require_capabilities")
                    or extension_cfg.get("require-capabilities"),
                    False,
                )
            require_checksum = args.require_checksum
            if not require_checksum:
                require_checksum = _coerce_bool(
                    extension_cfg.get("require_checksum")
                    or extension_cfg.get("require-checksum"),
                    False,
                )
            return extension_audit(
                path=args.path,
                require_capabilities=require_capabilities,
                require_abi=require_abi,
                require_checksum=require_checksum,
                json_output=args.json,
                verbose=args.verbose,
            )
        if args.extension_command == "scan":
            fail_on_missing = args.fail_on_missing
            if not fail_on_missing:
                fail_on_missing = _coerce_bool(
                    extension_cfg.get("scan_fail_on_missing")
                    or extension_cfg.get("scan-fail-on-missing"),
                    False,
                )
            return extension_scan(
                project=args.project or extension_cfg.get("project"),
                sources=args.source,
                fail_on_missing=fail_on_missing,
                json_output=args.json,
                verbose=args.verbose,
            )
        return _fail(
            "Missing extension subcommand (build|audit|scan).",
            args.json,
            command="extension",
        )
    if args.command == "check":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        return _typecheck.check(
            args.path,
            args.output,
            args.strict,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
        )
    if args.command == "run":
        build_args = _strip_leading_double_dash(args.build_arg)
        if args.rebuild and not _build_args_has_cache_flag(build_args):
            build_args.append("--no-cache")
        # Forward --backend to the build subprocess when specified.
        run_backend = getattr(args, "backend", None)
        if run_backend and not any(a.startswith("--backend") for a in build_args):
            build_args.extend(["--backend", run_backend])
        run_python_version = (
            getattr(args, "python_version", None)
            or run_cfg.get("python_version")
            or run_cfg.get("python-version")
        )
        if run_python_version and not _build_args_has_python_version_flag(build_args):
            build_args.extend(["--python-version", str(run_python_version)])
        run_target = getattr(args, "target", None) or run_cfg.get("target") or "native"
        run_profile = (
            ("release" if getattr(args, "release", False) else None)
            or args.profile
            or run_cfg.get("profile")
            or run_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if run_profile is not None and run_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid run profile: {run_profile}",
                args.json,
                command="run",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(run_cfg.get("trusted"), False)
        capabilities = (
            args.capabilities or run_cfg.get("capabilities") or cfg_capabilities
        )
        if run_target in ("wasm", "luau"):
            # Inject --target into build_args so run_script_cross handles it
            if not any(a.startswith("--target") for a in build_args):
                build_args.extend(["--target", run_target])
            return _commands._run_script_cross(
                run_target,
                args.file,
                args.module,
                _strip_leading_double_dash(args.script_args),
                args.json,
                args.verbose,
                args.timing,
                trusted,
                capabilities,
                getattr(args, "capability_manifest", None),
                getattr(args, "require_signed_manifest", False),
                build_args,
                cast(BuildProfile | None, run_profile),
                audit_log=getattr(args, "audit_log", None),
                io_mode=getattr(args, "io_mode", None),
                type_gate=getattr(args, "type_gate", False),
            )
        if run_target == "mlir":
            # MLIR target: build to get MLIR text (no JIT in run mode yet).
            if not any(a.startswith("--target") for a in build_args):
                build_args.extend(["--target", "mlir"])
            return _commands._run_script_cross(
                run_target,
                args.file,
                args.module,
                _strip_leading_double_dash(args.script_args),
                args.json,
                args.verbose,
                args.timing,
                trusted,
                capabilities,
                getattr(args, "capability_manifest", None),
                getattr(args, "require_signed_manifest", False),
                build_args,
                cast(BuildProfile | None, run_profile),
                audit_log=getattr(args, "audit_log", None),
                io_mode=getattr(args, "io_mode", None),
                type_gate=getattr(args, "type_gate", False),
            )
        return _commands.run_script(
            args.file,
            args.module,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            args.timing,
            trusted,
            capabilities,
            getattr(args, "capability_manifest", None),
            getattr(args, "require_signed_manifest", False),
            build_args,
            cast(BuildProfile | None, run_profile),
            audit_log=getattr(args, "audit_log", None),
            io_mode=getattr(args, "io_mode", None),
            type_gate=getattr(args, "type_gate", False),
        )
    if args.command == "repl":
        from molt.repl import run_repl

        molt_cmd: str | Sequence[str]
        if args.molt_cmd:
            molt_cmd = args.molt_cmd
        else:
            molt_cmd = [sys.executable, "-m", "molt.cli"]
        return run_repl(
            capabilities=args.capabilities,
            io_mode=args.io_mode,
            molt_cmd=molt_cmd,
            timeout_sec=args.timeout_sec,
        )
    if args.command == "compare":
        python_exe = args.python or args.python_version
        build_args = _strip_leading_double_dash(args.build_arg)
        compare_target_python = args.python_version
        if compare_target_python is None and args.python:
            with contextlib.suppress(ValueError):
                compare_target_python = _parse_target_python_version(args.python).short
        if compare_target_python and not _build_args_has_python_version_flag(
            build_args
        ):
            build_args.extend(["--python-version", compare_target_python])
        compare_profile = (
            args.profile
            or compare_cfg.get("profile")
            or compare_cfg.get("build_profile")
            or run_cfg.get("profile")
            or run_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if compare_profile is not None and compare_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid compare profile: {compare_profile}",
                args.json,
                command="compare",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(
                compare_cfg.get("trusted", run_cfg.get("trusted")),
                False,
            )
        capabilities = (
            args.capabilities
            or compare_cfg.get("capabilities")
            or run_cfg.get("capabilities")
            or cfg_capabilities
        )
        return _commands.compare(
            args.file,
            args.module,
            python_exe,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            trusted,
            capabilities,
            build_args,
            args.rebuild,
            cast(BuildProfile | None, compare_profile),
        )
    if args.command == "parity-run":
        python_exe = args.python or args.python_version
        return _commands.parity_run(
            args.file,
            args.module,
            python_exe,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            args.timing,
        )
    if args.command == "test":
        pytest_args = _strip_leading_double_dash(args.pytest_args)
        if args.suite == "dev" and (args.path or pytest_args) and args.verbose:
            print("Ignoring extra args for suite=dev.")
        test_profile = (
            args.profile
            or test_cfg.get("profile")
            or test_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if test_profile is not None and test_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid test profile: {test_profile}",
                args.json,
                command="test",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(test_cfg.get("trusted"), False)
        return _commands.test(
            args.suite,
            args.path,
            args.python_version,
            pytest_args,
            cast(BuildProfile | None, test_profile),
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "diff":
        diff_profile = (
            args.profile
            or diff_cfg.get("profile")
            or diff_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if diff_profile is not None and diff_profile not in {"dev", "release"}:
            return _fail(
                f"Invalid diff profile: {diff_profile}",
                args.json,
                command="diff",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(diff_cfg.get("trusted"), False)
        return _commands.diff(
            args.path,
            args.python_version,
            cast(BuildProfile | None, diff_profile),
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "bench":
        return _commands.bench(
            args.wasm,
            _strip_leading_double_dash(args.bench_args),
            args.bench_script,
            args.json,
            args.verbose,
        )
    if args.command == "profile":
        return _commands.profile(
            _strip_leading_double_dash(args.profile_args),
            args.json,
            args.verbose,
        )
    if args.command == "lint":
        return _commands.lint(args.json, args.verbose)
    if args.command == "setup":
        return setup(args.json, args.verbose, args.strict)
    if args.command == "doctor":
        return doctor(args.json, args.verbose, args.strict)
    if args.command == "dx":
        return handle_dx_command(args)
    if args.command == "update":
        include_manifests = args.manifests or args.all
        return update_repo(
            json_output=args.json,
            verbose=args.verbose,
            check_only=args.check,
            include_toolchains=args.toolchains,
            include_locks=args.locks,
            include_manifests=include_manifests,
        )
    if args.command == "validate":
        return validate(
            suite=cast(
                Literal["full", "smoke", "commands", "conformance", "bench"],
                args.suite,
            ),
            backend=cast(
                Literal["all", "native", "llvm", "wasm", "luau"],
                args.backend,
            ),
            profile=cast(Literal["all", "dev", "release"], args.profile),
            json_output=args.json,
            verbose=args.verbose,
            check_only=args.check,
            summary_out=args.summary_out,
        )
    if args.command == "package":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        capabilities = args.capabilities or cfg_capabilities
        sbom_enabled = args.sbom
        if sbom_enabled is None:
            sbom_enabled = True
        return package(
            args.artifact,
            args.manifest,
            args.output,
            json_output=args.json,
            verbose=args.verbose,
            deterministic=deterministic,
            deterministic_warn=deterministic_warn,
            capabilities=capabilities,
            sbom=sbom_enabled,
            sbom_output=args.sbom_output,
            sbom_format=args.sbom_format,
            signature=args.signature,
            signature_output=args.signature_output,
            sign=args.sign,
            signer=args.signer,
            signing_key=args.signing_key,
            signing_identity=args.signing_identity,
        )
    if args.command == "publish":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(
                publish_cfg.get("deterministic") or build_cfg.get("deterministic"),
                True,
            )
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                publish_cfg.get("deterministic_warn")
                or publish_cfg.get("deterministic-warn")
                or build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        explicit_require = args.require_signature is not None
        explicit_verify = args.verify_signature is not None
        require_signature = args.require_signature
        if require_signature is None:
            require_signature = _coerce_bool(
                publish_cfg.get("require_signature")
                or publish_cfg.get("require-signature")
                or os.environ.get("MOLT_REQUIRE_SIGNATURE"),
                False,
            )
        verify_signature = args.verify_signature
        if verify_signature is None:
            verify_signature = _coerce_bool(
                publish_cfg.get("verify_signature")
                or publish_cfg.get("verify-signature")
                or os.environ.get("MOLT_VERIFY_SIGNATURE"),
                False,
            )
        if explicit_require and not require_signature and not explicit_verify:
            verify_signature = False
        trusted_signers = (
            args.trusted_signers
            or publish_cfg.get("trusted_signers")
            or publish_cfg.get("trusted-signers")
            or os.environ.get("MOLT_TRUSTED_SIGNERS")
        )
        if _is_remote_registry(args.registry):
            if not explicit_require:
                require_signature = True
            if not explicit_verify and require_signature:
                verify_signature = True
            if trusted_signers is None and (require_signature or verify_signature):
                return _fail(
                    "Remote publish requires --trusted-signers or MOLT_TRUSTED_SIGNERS "
                    "(disable with --no-require-signature/--no-verify-signature).",
                    args.json,
                    command="publish",
                )
        capabilities = (
            args.capabilities or publish_cfg.get("capabilities") or cfg_capabilities
        )
        return publish(
            args.package,
            args.registry,
            args.dry_run,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
            capabilities,
            require_signature,
            verify_signature,
            trusted_signers,
            args.signer,
            args.signing_key,
            args.registry_token,
            args.registry_user,
            args.registry_password,
            args.registry_timeout,
        )
    if args.command == "verify":
        require_signature = args.require_signature
        if require_signature is None:
            require_signature = False
        verify_signature = args.verify_signature
        if verify_signature is None:
            verify_signature = False
        return verify(
            args.package,
            args.manifest,
            args.artifact,
            args.require_checksum,
            args.json,
            args.verbose,
            args.require_deterministic,
            args.capabilities or cfg_capabilities,
            require_signature,
            verify_signature,
            args.trusted_signers,
            args.signer,
            args.signing_key,
            args.require_extension_capabilities,
            args.require_extension_abi,
            args.extension_metadata,
        )
    if args.command == "deps":
        return deps(args.include_dev, args.json, args.verbose)
    if args.command == "install":
        pkgs = args.packages or []
        if pkgs and pkgs[0] == "add":
            add_pkgs = pkgs[1:]
            if not add_pkgs:
                return _fail(
                    "molt install add requires at least one package name.",
                    args.json,
                    command="install",
                )
            return install_add(
                add_pkgs,
                json_output=args.json,
                verbose=args.verbose,
            )
        return install(
            packages=pkgs or None,
            requirements=args.requirements,
            json_output=args.json,
            verbose=args.verbose,
            sync=args.sync,
        )
    if args.command == "vendor":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        return vendor(
            args.include_dev,
            args.json,
            args.verbose,
            args.output,
            args.dry_run,
            args.allow_non_tier_a,
            args.extras,
            deterministic,
            deterministic_warn,
        )
    if args.command == "clean":
        return clean(
            args.json,
            args.verbose,
            apply=args.apply,
            kill_processes=args.kill_processes,
            extra_paths=args.extra_path,
            list_paths=args.list_paths,
        )
    if args.command == "config":
        return show_config(config_root, config, args.json, args.verbose)
    if args.command == "completion":
        return completion(args.shell, args.json, args.verbose)

    if args.command == "harness":
        from molt.harness import main as harness_main

        harness_args = [getattr(args, "profile", "standard")]
        if getattr(args, "no_fail_fast", False):
            harness_args.append("--no-fail-fast")
        if getattr(args, "verbose", False):
            harness_args.append("--verbose")
        if getattr(args, "json", False):
            harness_args.append("--json")
        return harness_main(harness_args)

    if args.command == "deploy":
        deploy_build_profile = args.build_profile
        if getattr(args, "release", False) and not deploy_build_profile:
            deploy_build_profile = "release"
        return _commands._deploy(
            platform=args.platform,
            file_path=args.file,
            module=args.module,
            build_profile=deploy_build_profile,
            output=args.output,
            out_dir=args.out_dir,
            roblox_project=getattr(args, "roblox_project", None),
            wrangler_args=getattr(args, "wrangler_args", ""),
            dry_run=args.dry_run,
            build_args=_strip_leading_double_dash(args.build_arg),
            json_output=args.json,
            verbose=args.verbose,
        )

    return 2
