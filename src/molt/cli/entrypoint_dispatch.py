from __future__ import annotations

from molt.compilation_target import TargetKind, target_name

import argparse
import contextlib
import os
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Any, Callable, Literal, Mapping, Sequence, cast

# Each command imports its implementation when it runs: `molt build` reaches
# its early-cutoff check without loading the extension, packaging, script and
# frontend stacks (HF-250). Only helpers that the parser and the build path
# load anyway stay here.
from molt.cli.arg_helpers import _strip_leading_double_dash
from molt.cli.build_request import BuildRequest
from molt.cli.build_output_layout import _BUILD_PROFILE_CHOICES
from molt.cli.config_resolution import (
    _coerce_bool,
    _select_codegen_backend,
    _select_capability_input,
)
from molt.cli.output import fail as _fail
from molt.cli.progress import present_build

if TYPE_CHECKING:
    from molt.cli.build_request import BuildProfile


from molt.cli.build_request_resolution import _configured_build_target


def _dispatch_entrypoint_command(
    args: argparse.Namespace,
    *,
    parser: argparse.ArgumentParser,
    build_fn: Callable[[BuildRequest], int],
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
        from molt.cli import quality_commands as _quality_commands

        return _quality_commands._internal_batch_build_server(
            json_output=args.json,
            verbose=args.verbose,
            build_fn=build_fn,
        )

    if args.command == "internal-runtime-wasm-build":
        from molt.cli import runtime_build as _runtime_build

        return _runtime_build._prebuild_runtime_wasm(
            project_root=config_root,
            kind=cast(Literal["shared", "reloc", "both"], args.kind),
            required_link_features=frozenset(args.runtime_feature),
            json_output=args.json,
            build_profile=cast("BuildProfile", args.build_profile),
            cargo_timeout=args.cargo_timeout,
            simd_enabled=not args.no_simd,
            freestanding=args.freestanding,
            stdlib_profile=args.stdlib_profile,
            verbose=args.verbose,
        )

    if args.command == "internal-backend-build":
        target, codegen_backend, backend_error = _select_codegen_backend(
            _configured_build_target(args.target, build_cfg), args.backend
        )
        if backend_error is not None:
            return _fail(backend_error, args.json, command="internal-backend-build")
        # Deferred: the backend admission stack is not part of CLI startup.
        from molt.cli import backend_build as _backend_build

        return _backend_build._prebuild_backend_binary(
            project_root=config_root,
            target=target,
            codegen_backend=codegen_backend,
            json_output=args.json,
            cargo_timeout=args.cargo_timeout,
            verbose=args.verbose,
        )

    if args.command == "debug":
        from molt.cli import debug_helpers as _debug_helpers

        return _debug_helpers._handle_debug_command(args)

    if args.command == "queue":
        from molt.cli.queue_cli import handle_queue_command

        return handle_queue_command(args)

    if args.command == "build":
        build_fn = present_build(
            build_fn,
            mode=getattr(args, "progress", "auto"),
            headless=getattr(args, "headless", False),
            quiet=getattr(args, "quiet", False),
            json_output=args.json,
        )
        from molt.cli.build_request_resolution import resolve_build_request

        try:
            request = resolve_build_request(
                args,
                build_cfg=build_cfg,
                cfg_capabilities=cfg_capabilities,
                env=os.environ,
            )
        except ValueError as exc:
            return _fail(str(exc), args.json, command="build")
        return build_fn(request)
    if args.command == "factgraph":
        from molt.cli import factgraph as _factgraph

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
            from molt.cli import extension_commands as _extension_commands

            deterministic = (
                args.deterministic
                if args.deterministic is not None
                else _coerce_bool(extension_cfg.get("deterministic"), True)
            )
            capabilities = _select_capability_input(
                args.capabilities, extension_cfg.get("capabilities"), cfg_capabilities
            )
            molt_abi = (
                args.molt_abi
                or extension_cfg.get("molt_abi")
                or extension_cfg.get("molt-abi")
            )
            source_plan = extension_cfg.get("source_plan") or extension_cfg.get(
                "source-plan"
            )
            source_plan_target = extension_cfg.get(
                "source_plan_target"
            ) or extension_cfg.get("source-plan-target")
            source_plan_source_root = extension_cfg.get(
                "source_plan_source_root"
            ) or extension_cfg.get("source-plan-source-root")
            source_plan_build_root = extension_cfg.get(
                "source_plan_build_root"
            ) or extension_cfg.get("source-plan-build-root")
            source_plan_compile_commands = extension_cfg.get(
                "source_plan_compile_commands"
            ) or extension_cfg.get("source-plan-compile-commands")
            source_plan_exclude_linked_static_libraries = (
                args.source_plan_exclude_linked_static_libraries
                or extension_cfg.get("source_plan_exclude_linked_static_libraries")
                or extension_cfg.get("source-plan-exclude-linked-static-libraries")
            )
            return _extension_commands.extension_build(
                project=args.project or extension_cfg.get("project"),
                out_dir=args.out_dir
                or extension_cfg.get("out_dir")
                or extension_cfg.get("out-dir"),
                module=args.module
                or extension_cfg.get("module")
                or extension_cfg.get("extension_module")
                or extension_cfg.get("extension-module"),
                molt_abi=molt_abi,
                capabilities=capabilities,
                provided_capsules=args.provided_capsules
                or extension_cfg.get("provided_capsules")
                or extension_cfg.get("provided-capsules"),
                python_export=args.python_export
                or extension_cfg.get("python_exports")
                or extension_cfg.get("python-exports"),
                callable_export_json=args.callable_export_json
                or extension_cfg.get("callable_exports")
                or extension_cfg.get("callable-exports"),
                support_file=args.support_file
                or extension_cfg.get("support_files")
                or extension_cfg.get("support-files"),
                deterministic=deterministic,
                python_version=args.python_version,
                target=args.target or extension_cfg.get("target"),
                source_plan=args.source_plan or source_plan,
                source_plan_target=args.source_plan_target or source_plan_target,
                source_plan_source_root=(
                    args.source_plan_source_root or source_plan_source_root
                ),
                source_plan_build_root=(
                    args.source_plan_build_root or source_plan_build_root
                ),
                source_plan_compile_commands=(
                    args.source_plan_compile_commands or source_plan_compile_commands
                ),
                source_plan_exclude_linked_static_libraries=(
                    source_plan_exclude_linked_static_libraries
                ),
                abi_tier=args.abi_tier
                or extension_cfg.get("abi_tier")
                or extension_cfg.get("abi-tier"),
                json_output=args.json,
                verbose=args.verbose,
            )
        if args.extension_command == "metadata":
            from molt.cli import extension_commands as _extension_commands

            return _extension_commands.extension_metadata(
                target=args.target,
                out_dir=args.out_dir,
                python_version=args.python_version,
                abi_tier=args.abi_tier,
                json_output=args.json,
                verbose=args.verbose,
            )
        if args.extension_command == "produce-set":
            from molt.cli.source_extension_producer import produce_source_extension_set

            return produce_source_extension_set(
                prepared=args.prepared,
                package=args.package,
                package_version=args.package_version,
                module_set=args.module_set,
                python_version=args.python_version,
                source=args.source,
                build_root=args.build_root,
                target=args.target,
                abi_tier=args.abi_tier,
                expected_identity_sha256=args.expected_identity_sha256,
                expected_candidate_identity_sha256=(
                    args.expected_candidate_identity_sha256
                ),
                json_output=args.json,
            )
        if args.extension_command == "attest-set-candidate":
            from molt.cli.source_extension_producer import (
                attest_source_extension_set_candidate,
            )

            return attest_source_extension_set_candidate(
                prepared=args.prepared,
                package=args.package,
                package_version=args.package_version,
                module_set=args.module_set,
                python_version=args.python_version,
                source=args.source,
                build_root=args.build_root,
                output=args.output,
                target=args.target,
                abi_tier=args.abi_tier,
                json_output=args.json,
            )
        if args.extension_command == "publish-set-candidate":
            from molt.cli.source_extension_candidate_promotion import (
                publish_source_extension_set_candidate,
            )

            return publish_source_extension_set_candidate(
                candidate=args.candidate,
                expected_incumbent_seal_sha256=(args.expected_incumbent_seal_sha256),
                expected_incumbent_identity_sha256=(
                    args.expected_incumbent_identity_sha256
                ),
                json_output=args.json,
            )
        if args.extension_command == "audit":
            from molt.cli.extension_audit import extension_audit

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
            require_artifact_file = args.require_artifact_file
            if not require_artifact_file:
                require_artifact_file = _coerce_bool(
                    extension_cfg.get("require_artifact_file")
                    or extension_cfg.get("require-artifact-file"),
                    False,
                )
            require_object_closure = args.require_object_closure
            if not require_object_closure:
                require_object_closure = _coerce_bool(
                    extension_cfg.get("require_object_closure")
                    or extension_cfg.get("require-object-closure"),
                    False,
                )
            return extension_audit(
                path=args.path,
                require_capabilities=require_capabilities,
                require_abi=require_abi,
                require_checksum=require_checksum,
                require_loader_kind=args.require_loader_kind
                or extension_cfg.get("require_loader_kind")
                or extension_cfg.get("require-loader-kind"),
                require_runtime_linkage=args.require_runtime_linkage
                or extension_cfg.get("require_runtime_linkage")
                or extension_cfg.get("require-runtime-linkage"),
                require_artifact_kind=args.require_artifact_kind
                or extension_cfg.get("require_artifact_kind")
                or extension_cfg.get("require-artifact-kind"),
                require_artifact_file=require_artifact_file,
                require_object_closure=require_object_closure,
                require_python_export=args.require_python_export,
                require_callable_export=args.require_callable_export,
                json_output=args.json,
                verbose=args.verbose,
            )
        if args.extension_command == "seal":
            from molt.cli.extension_seal import extension_seal

            return extension_seal(
                path=args.path,
                out_dir=args.out_dir,
                python_export=args.python_export,
                callable_export_json=args.callable_export_json,
                support_file=args.support_file
                or extension_cfg.get("support_files")
                or extension_cfg.get("support-files"),
                json_output=args.json,
                verbose=args.verbose,
            )
        if args.extension_command == "scan":
            from molt.cli.extension_scan import extension_scan

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
                exclude_dirs=args.exclude_dir,
                fail_on_missing=fail_on_missing,
                json_output=args.json,
                verbose=args.verbose,
            )
        return _fail(
            "Missing extension subcommand "
            "(build|metadata|produce-set|attest-set-candidate|publish-set-candidate|audit|seal|scan).",
            args.json,
            command="extension",
        )
    if args.command == "check":
        from molt.cli import typecheck as _typecheck

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
        from molt.cli import script_commands as _script_commands
        from molt.cli.arg_helpers import _build_args_has_cache_flag
        from molt.cli.wrapper_build import _build_args_has_python_version_flag

        presentation = {
            "mode": getattr(args, "progress", "auto"),
            "headless": getattr(args, "headless", False),
            "quiet": getattr(args, "quiet", False),
            "json_output": args.json,
        }
        run_script = present_build(_script_commands.run_script, **presentation)
        run_cross = present_build(_script_commands._run_script_cross, **presentation)
        build_args = _strip_leading_double_dash(args.build_arg)
        if getattr(args, "quiet", False):
            build_args.append("--quiet")
        if getattr(args, "headless", False):
            build_args.append("--headless")
        progress_mode = getattr(args, "progress", "auto")
        if progress_mode != "auto":
            build_args.extend(("--progress", progress_mode))
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
        run_target = getattr(args, "target", None) or run_cfg.get("target")
        from molt.cli.build_request_resolution import (
            parse_build_arguments,
            resolve_build_request,
        )

        try:
            build_selection = parse_build_arguments(
                build_args, file_path=args.file, module=args.module
            )
            if run_target is not None and build_selection.target is None:
                build_args.extend(["--target", run_target])
                build_selection.target = run_target
            effective_build = resolve_build_request(
                build_selection,
                build_cfg=build_cfg,
                cfg_capabilities=cfg_capabilities,
                env=os.environ,
            )
            run_target = effective_build.target
        except ValueError as exc:
            return _fail(str(exc), args.json, command="run")
        run_profile = (
            ("release" if getattr(args, "release", False) else None)
            or args.profile
            or run_cfg.get("profile")
            or run_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if run_profile is not None and run_profile not in _BUILD_PROFILE_CHOICES:
            return _fail(
                f"Invalid run profile: {run_profile}",
                args.json,
                command="run",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(run_cfg.get("trusted"), False)
        capabilities = _select_capability_input(
            args.capabilities, run_cfg.get("capabilities"), cfg_capabilities
        )
        if run_target in {
            TargetKind.WASM_FREESTANDING,
            TargetKind.WASM_UNKNOWN,
            TargetKind.RUST,
        }:
            return _fail(
                f"molt run cannot execute target {target_name(run_target)!r}; "
                "use molt build to produce its artifact.",
                args.json,
                command="run",
            )
        if run_target in {TargetKind.WASM, TargetKind.WASI, TargetKind.LUAU}:
            # Inject --target into build_args so run_script_cross handles it
            if not any(a.startswith("--target") for a in build_args):
                build_args.extend(["--target", target_name(run_target)])
            return run_cross(
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
                cast("BuildProfile | None", run_profile),
                audit_log=getattr(args, "audit_log", None),
                io_mode=getattr(args, "io_mode", None),
            )
        if run_target is TargetKind.MLIR:
            # MLIR target: build to get MLIR text (no JIT in run mode yet).
            if not any(a.startswith("--target") for a in build_args):
                build_args.extend(["--target", "mlir"])
            return run_cross(
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
                cast("BuildProfile | None", run_profile),
                audit_log=getattr(args, "audit_log", None),
                io_mode=getattr(args, "io_mode", None),
            )
        return run_script(
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
            cast("BuildProfile | None", run_profile),
            audit_log=getattr(args, "audit_log", None),
            io_mode=getattr(args, "io_mode", None),
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
        from molt.cli import script_commands as _script_commands
        from molt.cli.wrapper_build import _build_args_has_python_version_flag
        from molt.target_python import _parse_target_python_version

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
        if (
            compare_profile is not None
            and compare_profile not in _BUILD_PROFILE_CHOICES
        ):
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
        capabilities = _select_capability_input(
            args.capabilities,
            compare_cfg.get("capabilities"),
            run_cfg.get("capabilities"),
            cfg_capabilities,
        )
        return _script_commands.compare(
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
            cast("BuildProfile | None", compare_profile),
        )
    if args.command == "parity-run":
        from molt.cli import script_commands as _script_commands

        python_exe = args.python or args.python_version
        return _script_commands.parity_run(
            args.file,
            args.module,
            python_exe,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            args.timing,
        )
    if args.command == "test":
        from molt.cli import quality_commands as _quality_commands

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
        if test_profile is not None and test_profile not in _BUILD_PROFILE_CHOICES:
            return _fail(
                f"Invalid test profile: {test_profile}",
                args.json,
                command="test",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(test_cfg.get("trusted"), False)
        return _quality_commands.test(
            args.suite,
            args.path,
            args.python_version,
            pytest_args,
            cast("BuildProfile | None", test_profile),
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "diff":
        from molt.cli import script_commands as _script_commands

        diff_profile = (
            args.profile
            or diff_cfg.get("profile")
            or diff_cfg.get("build_profile")
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "dev"
        )
        if diff_profile is not None and diff_profile not in _BUILD_PROFILE_CHOICES:
            return _fail(
                f"Invalid diff profile: {diff_profile}",
                args.json,
                command="diff",
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(diff_cfg.get("trusted"), False)
        return _script_commands.diff(
            args.path,
            args.python_version,
            cast("BuildProfile | None", diff_profile),
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "bench":
        from molt.cli import quality_commands as _quality_commands

        return _quality_commands.bench(
            args.wasm,
            _strip_leading_double_dash(args.bench_args),
            args.bench_script,
            args.json,
            args.verbose,
        )
    if args.command == "profile":
        from molt.cli import quality_commands as _quality_commands

        return _quality_commands.profile(
            _strip_leading_double_dash(args.profile_args),
            args.json,
            args.verbose,
        )
    if args.command == "lint":
        from molt.cli import quality_commands as _quality_commands

        return _quality_commands.lint(args.json, args.verbose)
    if args.command == "setup":
        from molt.cli.setup_readiness import setup

        if args.install_cli_dependencies:
            return _fail(
                "Use the packaged molt launcher with 'setup --install-cli-dependencies' "
                "and no other flags; source checkouts manage dependencies with uv sync.",
                json_output=args.json,
                command="setup",
            )
        return setup(args.json, args.verbose, args.strict)
    if args.command == "doctor":
        from molt.cli.setup_readiness import doctor

        return doctor(args.json, args.verbose, args.strict)
    if args.command == "dx":
        from molt.cli.dx_cli import handle_dx_command

        return handle_dx_command(args)
    if args.command == "update":
        from molt.cli.toolchain_validation import update_repo

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
        from molt.cli.toolchain_validation import validate

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
        from molt.cli.package_distribution import package

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
        capabilities = _select_capability_input(args.capabilities, cfg_capabilities)
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
        from molt.cli.package_distribution import publish, verify
        from molt.cli.package_registry import _is_remote_registry

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
        capabilities = _select_capability_input(
            args.capabilities, publish_cfg.get("capabilities"), cfg_capabilities
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
        from molt.cli.package_distribution import verify

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
            _select_capability_input(args.capabilities, cfg_capabilities),
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
        from molt.cli.deps import deps

        return deps(args.include_dev, args.json, args.verbose)
    if args.command == "install":
        from molt.cli.deps import install, install_add

        pkgs = args.packages or []
        if pkgs and pkgs[0] == args._install_add_command:
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
        from molt.cli.deps import vendor

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
        from molt.cli.maintenance import clean

        return clean(
            args.json,
            args.verbose,
            apply=args.apply,
            kill_processes=args.kill_processes,
            extra_paths=args.extra_path,
            list_paths=args.list_paths,
        )
    if args.command == "config":
        from molt.cli.maintenance import show_config

        return show_config(config_root, config, args.json, args.verbose)
    if args.command == "completion":
        from molt.cli.arg_helpers import completion

        return completion(args.shell, args.json, args.verbose, parser=parser)

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
        from molt.cli import script_commands as _script_commands

        deploy_build_profile = args.build_profile
        if getattr(args, "release", False) and not deploy_build_profile:
            deploy_build_profile = "release"
        return _script_commands._deploy(
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
