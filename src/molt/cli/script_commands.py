"""Run, deploy, compare, parity, and differential CLI commands."""

from __future__ import annotations

import contextlib
import json
import shlex
import shutil
import sys
import time
from pathlib import Path
from typing import Any
from molt.cli import build_inputs as _build_inputs
from molt.cli.arg_helpers import (
    _build_args_has_cache_flag,
    _build_args_has_capabilities_flag,
    _build_args_has_profile_flag,
    _build_args_has_trusted_flag,
    _extract_emit_arg,
    _resolve_binary_output,
)
from molt.cli.atomic_io import (
    _atomic_copy_file,
)
from molt.cli.capability_spec import (
    CapabilityInput,
    _materialize_capabilities_arg,
    _parse_capabilities,
)
from molt.cli.command_runtime import (
    _CLI_MEMORY_GUARD_PREFIX,
    _CROSS_MEMORY_GUARD_PREFIX,
    _DIFF_MEMORY_GUARD_PREFIX,
    _run_completed_command,
)
from molt.cli.env_paths import _base_env
from molt.cli.models import (
    BuildProfile,
)
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import (
    _find_molt_root,
    _find_project_root,
    _require_molt_root,
)
from molt.target_python import (
    _parse_target_python_version,
)
from molt.python_interpreter import (
    PythonInterpreterError,
    format_python_command,
    resolve_python_selector,
)
from molt.cli.wrapper_build import (
    _build_args_has_python_version_flag,
    _run_wrapper_build,
)

from molt.cli.process_execution import (
    _format_duration,
    _run_command,
    _run_command_timed,
)


def _run_script_cross(
    target: str,
    file_path: str | None,
    module: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    timing: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    capability_manifest: str | None = None,
    require_signed_manifest: bool = False,
    build_args: list[str] | None = None,
    build_profile: BuildProfile | None = None,
    audit_log: str | None = None,
    io_mode: str | None = None,
    type_gate: bool = False,
) -> int:
    """Build with a cross target (wasm or luau) and run with the appropriate runtime."""
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.", json_output, command="run"
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="run")

    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    build_args = list(build_args or [])
    resolved_build_entry, resolved_build_entry_error = (
        _build_inputs._resolve_wrapper_build_entry(
            file_path=file_path,
            module=module,
            project_root=project_root,
            json_output=json_output,
            command="run",
            build_args=build_args,
        )
    )
    if resolved_build_entry_error is not None:
        return resolved_build_entry_error
    assert resolved_build_entry is not None
    molt_root = _find_molt_root(project_root, Path.cwd())
    source_path = resolved_build_entry.source_path
    env = _base_env(
        project_root,
        source_path if file_path else None,
        molt_root=molt_root,
    )
    if file_path:
        env.update(_build_inputs._collect_env_overrides(file_path))
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if capabilities is not None:
        parsed, _profiles, _source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="run",
            )
        if parsed is not None:
            env["MOLT_CAPABILITIES"] = ",".join(parsed)

    if capability_manifest is not None:
        from molt.capability_manifest import load_manifest

        try:
            manifest_obj = load_manifest(
                capability_manifest, require_signed=require_signed_manifest
            )
            env.update(manifest_obj.to_env_vars())
        except Exception as e:
            return _fail(
                f"Invalid capability manifest: {e}",
                json_output,
                command="run",
            )

    # --audit-log flag (overrides manifest audit config)
    if audit_log is not None:
        env.update(_build_inputs._parse_audit_log_flag(audit_log))

    # --io-mode flag (overrides manifest io config)
    if io_mode is not None:
        env.update(_build_inputs._parse_io_mode_flag(io_mode))

    # --type-gate flag
    env.update(_build_inputs._parse_type_gate_flag(type_gate))

    capabilities_tmp: Path | None = None
    if build_profile is not None and not _build_args_has_profile_flag(build_args):
        build_args.extend(["--build-profile", build_profile])
    if trusted and not _build_args_has_trusted_flag(build_args):
        build_args.append("--trusted")
    if capabilities is not None and not _build_args_has_capabilities_flag(build_args):
        cap_arg, capabilities_tmp = _materialize_capabilities_arg(capabilities)
        build_args.extend(["--capabilities", cap_arg])

    if not json_output:
        target_label = (
            "WASM" if target == "wasm" else "MLIR" if target == "mlir" else "Luau"
        )
        print(f"Building for {target_label}...", file=sys.stderr)
    try:
        build_contract, t_build, build_error = _run_wrapper_build(
            file_path=file_path,
            module=module,
            build_args=build_args,
            env=env,
            project_root=project_root,
            json_output=json_output,
            command="run",
            verbose=verbose,
            resolved_build_entry=resolved_build_entry,
            memory_guard_prefix=_CROSS_MEMORY_GUARD_PREFIX,
        )
    finally:
        if capabilities_tmp is not None:
            try:
                capabilities_tmp.unlink()
            except OSError:
                pass
    if build_error is not None:
        return build_error
    assert build_contract is not None

    if target == "wasm":
        run_artifact = build_contract.consumer_output
        if not run_artifact.exists():
            return _fail(
                f"WASM artifact not found: {run_artifact}\n"
                "Hint: the build may have succeeded but placed output elsewhere. "
                "Try `molt build --target wasm --verbose` to see the output path.",
                json_output,
                command="run",
            )
        wasmtime = shutil.which("wasmtime")
        if wasmtime is None:
            return _fail(
                "wasmtime not found on PATH. Install it: https://wasmtime.dev\n"
                "Hint: curl https://wasmtime.dev/install.sh -sSf | bash",
                json_output,
                command="run",
            )
        run_cmd = [wasmtime, "run", str(run_artifact), "--", *script_args]
    elif target == "luau":
        luau_artifact = build_contract.artifacts.get(
            "luau", build_contract.consumer_output
        )
        if not luau_artifact.exists():
            return _fail(
                f"Luau artifact not found: {luau_artifact}\n"
                "Hint: the build may have succeeded but placed output elsewhere. "
                "Try `molt build --target luau --verbose` to see the output path.",
                json_output,
                command="run",
            )
        lune = shutil.which("lune")
        if lune is None:
            return _fail(
                "lune not found on PATH. Install it: https://lune-org.github.io/docs/getting-started/installation\n"
                "Hint: cargo install lune",
                json_output,
                command="run",
            )
        run_cmd = [lune, "run", str(luau_artifact), "--", *script_args]
    elif target == "mlir":
        # MLIR target: the build phase produces .mlir text. There is no
        # separate run phase -- the MLIR output is the artifact.
        mlir_artifact = build_contract.consumer_output
        if not mlir_artifact.exists():
            return _fail(
                f"MLIR artifact not found: {mlir_artifact}\n"
                "Hint: the build may have succeeded but placed output elsewhere. "
                "Try `molt build --target mlir --verbose` to see the output path.",
                json_output,
                command="run",
            )
        if not json_output:
            print(f"MLIR output: {mlir_artifact}", file=sys.stderr)
        if timing and not json_output:
            print(
                f"\n--- timing: build {t_build:.3f}s ---",
                file=sys.stderr,
            )
        return 0
    else:
        return _fail(f"Unsupported cross target: {target}", json_output, command="run")

    if not json_output and verbose:
        print(f"Running: {shlex.join(run_cmd)}", file=sys.stderr)

    t_run_start = time.monotonic()
    run_res = _run_completed_command(
        run_cmd,
        env=env,
        cwd=project_root,
        capture_output=False,
        memory_guard_prefix=_CROSS_MEMORY_GUARD_PREFIX,
    )
    t_run = getattr(run_res, "elapsed_s", None)
    if t_run is None:
        t_run = time.monotonic() - t_run_start

    if timing and not json_output:
        print(
            f"\n--- timing: build {t_build:.3f}s | run {t_run:.3f}s | "
            f"total {t_build + t_run:.3f}s ---",
            file=sys.stderr,
        )
    return run_res.returncode


def _deploy(
    platform: str,
    file_path: str | None,
    module: str | None,
    build_profile: str | None,
    output: str | None,
    out_dir: str | None,
    roblox_project: str | None,
    wrangler_args: str,
    dry_run: bool,
    build_args: list[str],
    json_output: bool,
    verbose: bool,
) -> int:
    """Build and deploy to the specified platform."""
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.", json_output, command="deploy"
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="deploy")

    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    build_cmd_args = list(build_args)
    resolved_build_entry, resolved_build_entry_error = (
        _build_inputs._resolve_wrapper_build_entry(
            file_path=file_path,
            module=module,
            project_root=project_root,
            json_output=json_output,
            command="deploy",
            build_args=build_cmd_args,
        )
    )
    if resolved_build_entry_error is not None:
        return resolved_build_entry_error
    assert resolved_build_entry is not None
    molt_root = _find_molt_root(project_root, Path.cwd())
    env = _base_env(
        project_root,
        resolved_build_entry.source_path if file_path else None,
        molt_root=molt_root,
    )
    if file_path:
        env.update(_build_inputs._collect_env_overrides(file_path))

    # Construct build command
    if platform == "cloudflare":
        if not any(a.startswith("--target") for a in build_cmd_args):
            build_cmd_args.extend(["--target", "wasm"])
        if not any(
            a.startswith("--profile") or a.startswith("--platform")
            for a in build_cmd_args
        ):
            build_cmd_args.extend(["--profile", "cloudflare"])
        if not any(a == "--split-runtime" for a in build_cmd_args):
            build_cmd_args.append("--split-runtime")
    elif platform == "roblox":
        if not any(a.startswith("--target") for a in build_cmd_args):
            build_cmd_args.extend(["--target", "luau"])

    if build_profile and not _build_args_has_profile_flag(build_cmd_args):
        build_cmd_args.extend(["--build-profile", build_profile])
    if output:
        build_cmd_args.extend(["--output", output])
    if out_dir:
        build_cmd_args.extend(["--out-dir", out_dir])
    if verbose:
        build_cmd_args.append("--verbose")

    if not json_output:
        platform_label = "Cloudflare Workers" if platform == "cloudflare" else "Roblox"
        print(f"Building for {platform_label}...", file=sys.stderr)
    build_contract, _t_build, build_error = _run_wrapper_build(
        file_path=file_path,
        module=module,
        build_args=build_cmd_args,
        env=env,
        project_root=project_root,
        json_output=json_output,
        command="deploy",
        verbose=verbose,
        resolved_build_entry=resolved_build_entry,
        memory_guard_prefix=_CROSS_MEMORY_GUARD_PREFIX,
    )
    if build_error is not None:
        return build_error
    assert build_contract is not None

    if dry_run:
        if not json_output:
            print("Build succeeded (dry run, skipping deploy).", file=sys.stderr)
        return 0

    if platform == "cloudflare":
        wrangler = shutil.which("wrangler")
        if wrangler is None:
            return _fail(
                "wrangler not found on PATH. Install it:\n"
                "  npm install -g wrangler\n"
                "  # or: npx wrangler deploy",
                json_output,
                command="deploy",
            )
        bundle_root = build_contract.bundle_root
        if bundle_root is None:
            return _fail(
                "Build JSON missing bundle_root for Cloudflare deploy.",
                json_output,
                command="deploy",
            )
        wrangler_config = build_contract.artifacts.get("wrangler_config")
        if wrangler_config is None:
            return _fail(
                "Build JSON missing wrangler_config for Cloudflare deploy.",
                json_output,
                command="deploy",
            )
        if not bundle_root.is_dir():
            return _fail(
                f"Cloudflare bundle root not found: {bundle_root}",
                json_output,
                command="deploy",
            )
        if not wrangler_config.exists():
            return _fail(
                f"Cloudflare wrangler config not found: {wrangler_config}",
                json_output,
                command="deploy",
            )
        deploy_cmd_parts = [wrangler, "deploy", "--config", str(wrangler_config)]
        if wrangler_args:
            deploy_cmd_parts.extend(shlex.split(wrangler_args))
        if not json_output:
            print("Deploying with wrangler...", file=sys.stderr)
            if verbose:
                print(
                    f"Deploy command: {shlex.join(deploy_cmd_parts)}", file=sys.stderr
                )
        return _run_command(
            deploy_cmd_parts,
            env=env,
            cwd=bundle_root,
            json_output=json_output,
            label="deploy",
            memory_guard_prefix=_CROSS_MEMORY_GUARD_PREFIX,
        )

    elif platform == "roblox":
        if roblox_project is None:
            if not json_output:
                print(
                    "Build succeeded. Use --roblox-project <dir> to auto-copy "
                    "Luau output into your Roblox project.",
                    file=sys.stderr,
                )
            return 0
        roblox_dir = Path(roblox_project)
        if not roblox_dir.is_dir():
            return _fail(
                f"Roblox project directory not found: {roblox_dir}",
                json_output,
                command="deploy",
            )
        luau_artifact = build_contract.artifacts.get(
            "luau", build_contract.consumer_output
        )
        if not luau_artifact.exists():
            return _fail(
                f"Luau artifact not found at {luau_artifact}. "
                "Build may have placed it elsewhere; check --verbose output.",
                json_output,
                command="deploy",
            )
        dest = roblox_dir / luau_artifact.name
        try:
            _atomic_copy_file(luau_artifact, dest)
        except OSError as exc:
            return _fail(
                f"Failed to publish Roblox Luau artifact: {exc}",
                json_output,
                command="deploy",
            )
        if not json_output:
            print(f"Copied {luau_artifact.name} -> {dest}", file=sys.stderr)
        return 0

    return _fail(f"Unknown deploy platform: {platform}", json_output, command="deploy")


def run_script(
    file_path: str | None,
    module: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    timing: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    capability_manifest: str | None = None,
    require_signed_manifest: bool = False,
    build_args: list[str] | None = None,
    build_profile: BuildProfile | None = None,
    audit_log: str | None = None,
    io_mode: str | None = None,
    type_gate: bool = False,
) -> int:
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.", json_output, command="run"
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="run")
    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    build_args = list(build_args or [])
    resolved_build_entry, resolved_build_entry_error = (
        _build_inputs._resolve_wrapper_build_entry(
            file_path=file_path,
            module=module,
            project_root=project_root,
            json_output=json_output,
            command="run",
            build_args=build_args,
        )
    )
    if resolved_build_entry_error is not None:
        return resolved_build_entry_error
    assert resolved_build_entry is not None
    molt_root = _find_molt_root(project_root, Path.cwd())
    source_path = resolved_build_entry.source_path
    env = _base_env(
        project_root,
        source_path if file_path else None,
        molt_root=molt_root,
    )
    if file_path:
        env.update(_build_inputs._collect_env_overrides(file_path))
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if capabilities is not None:
        parsed, _profiles, _source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="run",
            )
        if parsed is not None:
            env["MOLT_CAPABILITIES"] = ",".join(parsed)

    if capability_manifest is not None:
        from molt.capability_manifest import load_manifest

        try:
            manifest_obj = load_manifest(
                capability_manifest, require_signed=require_signed_manifest
            )
            env.update(manifest_obj.to_env_vars())
        except Exception as e:
            return _fail(
                f"Invalid capability manifest: {e}",
                json_output,
                command="run",
            )

    # --audit-log flag (overrides manifest audit config)
    if audit_log is not None:
        env.update(_build_inputs._parse_audit_log_flag(audit_log))

    # --io-mode flag (overrides manifest io config)
    if io_mode is not None:
        env.update(_build_inputs._parse_io_mode_flag(io_mode))

    # --type-gate flag
    env.update(_build_inputs._parse_type_gate_flag(type_gate))

    capabilities_tmp: Path | None = None
    if build_profile is not None and not _build_args_has_profile_flag(build_args):
        build_args.extend(["--build-profile", build_profile])
    if trusted and not _build_args_has_trusted_flag(build_args):
        build_args.append("--trusted")
    if capabilities is not None and not _build_args_has_capabilities_flag(build_args):
        cap_arg, capabilities_tmp = _materialize_capabilities_arg(capabilities)
        build_args.extend(["--capabilities", cap_arg])
    try:
        build_contract, build_duration_s, build_error = _run_wrapper_build(
            file_path=file_path,
            module=module,
            build_args=build_args,
            env=env,
            project_root=project_root,
            json_output=json_output,
            command="run",
            verbose=verbose,
            resolved_build_entry=resolved_build_entry,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
    finally:
        if capabilities_tmp is not None:
            try:
                capabilities_tmp.unlink()
            except OSError:
                pass
    if build_error is not None:
        return build_error
    assert build_contract is not None
    emit_arg = _extract_emit_arg(build_args)
    if emit_arg and emit_arg != "bin":
        return _fail(
            f"Compiled run requires emit=bin (got {emit_arg})",
            json_output,
            command="run",
        )
    output_binary = _resolve_binary_output(str(build_contract.consumer_output))
    if output_binary is None:
        return _fail(
            f"Compiled binary not found at {build_contract.consumer_output}.",
            json_output,
            command="run",
        )
    if timing:
        run_res = _run_command_timed(
            [str(output_binary), *script_args],
            env=env,
            cwd=project_root,
            verbose=verbose,
            capture_output=json_output,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
        if json_output:
            data: dict[str, Any] = {
                "returncode": run_res.returncode,
                "timing": {
                    "build_s": build_duration_s,
                    "run_s": run_res.duration_s,
                    "total_s": build_duration_s + run_res.duration_s,
                },
            }
            if run_res.stdout:
                data["stdout"] = run_res.stdout
            if run_res.stderr:
                data["stderr"] = run_res.stderr
            payload = _json_payload(
                "run",
                "ok" if run_res.returncode == 0 else "error",
                data=data,
            )
            _emit_json(payload, json_output=True)
        else:
            print("Timing (compiled):", file=sys.stderr)
            print(f"- build: {_format_duration(build_duration_s)}", file=sys.stderr)
            print(
                f"- run: {_format_duration(run_res.duration_s)}",
                file=sys.stderr,
            )
            total = build_duration_s + run_res.duration_s
            print(f"- total: {_format_duration(total)}", file=sys.stderr)
        return run_res.returncode
    return _run_command(
        [str(output_binary), *script_args],
        env=env,
        cwd=project_root,
        json_output=json_output,
        verbose=verbose,
        label="run",
        memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
    )


def compare(
    file_path: str | None,
    module: str | None,
    python_exe: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    build_args: list[str] | None = None,
    rebuild: bool = False,
    build_profile: BuildProfile | None = None,
) -> int:
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.",
            json_output,
            command="compare",
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="compare")
    source_path: Path | None = None
    if file_path:
        source_path = Path(file_path)
        if not source_path.exists():
            return _fail(
                f"File not found: {source_path}", json_output, command="compare"
            )
    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    molt_root = _find_molt_root(project_root, Path.cwd())
    env = _base_env(project_root, source_path, molt_root=molt_root)
    if file_path:
        env.update(_build_inputs._collect_env_overrides(file_path))
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if capabilities is not None:
        parsed, _profiles, _source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="compare",
            )
        if parsed is not None:
            env["MOLT_CAPABILITIES"] = ",".join(parsed)

    requested_python_selector = python_exe
    requested_target_python = None
    if requested_python_selector is not None:
        with contextlib.suppress(ValueError):
            requested_target_python = _parse_target_python_version(
                requested_python_selector
            )
    try:
        python_command = resolve_python_selector(
            python_exe,
            env=env,
            cwd=project_root,
        )
    except (PythonInterpreterError, ValueError) as exc:
        return _fail(str(exc), json_output, command="compare")
    if module:
        cpy_cmd = [*python_command, "-m", module, *script_args]
    else:
        cpy_cmd = [*python_command, str(source_path), *script_args]
    cpy_res = _run_command_timed(
        cpy_cmd,
        env=env,
        cwd=project_root,
        verbose=verbose,
        capture_output=True,
        memory_guard_prefix=_DIFF_MEMORY_GUARD_PREFIX,
    )

    build_args = list(build_args or [])
    if requested_target_python is not None and not _build_args_has_python_version_flag(
        build_args
    ):
        build_args.extend(["--python-version", requested_target_python.short])
    capabilities_tmp: Path | None = None
    if build_profile is not None and not _build_args_has_profile_flag(build_args):
        build_args.extend(["--build-profile", build_profile])
    if rebuild and not _build_args_has_cache_flag(build_args):
        build_args.append("--no-cache")
    if trusted and not _build_args_has_trusted_flag(build_args):
        build_args.append("--trusted")
    if capabilities is not None and not _build_args_has_capabilities_flag(build_args):
        cap_arg, capabilities_tmp = _materialize_capabilities_arg(capabilities)
        build_args.extend(["--capabilities", cap_arg])
    emit_arg = _extract_emit_arg(build_args)
    if emit_arg and emit_arg != "bin":
        return _fail(
            f"Compare requires emit=bin (got {emit_arg})",
            json_output,
            command="compare",
        )
    build_python_command = (
        python_command if requested_target_python is not None else (sys.executable,)
    )
    build_cmd = [
        *build_python_command,
        "-m",
        "molt.cli",
        "build",
        "--json",
        *build_args,
    ]
    if module:
        build_cmd.extend(["--module", module])
    else:
        assert file_path is not None
        build_cmd.append(file_path)
    try:
        build_res = _run_command_timed(
            build_cmd,
            env=env,
            cwd=project_root,
            verbose=verbose,
            capture_output=True,
            memory_guard_prefix=_DIFF_MEMORY_GUARD_PREFIX,
        )
    finally:
        if capabilities_tmp is not None:
            try:
                capabilities_tmp.unlink()
            except OSError:
                pass
    if build_res.returncode != 0:
        if json_output:
            data: dict[str, Any] = {
                "returncode": build_res.returncode,
                "timing": {"build_s": build_res.duration_s},
            }
            if build_res.stdout:
                data["build_stdout"] = build_res.stdout
            if build_res.stderr:
                data["build_stderr"] = build_res.stderr
            payload = _json_payload(
                "compare",
                "error",
                data=data,
                errors=["build failed"],
            )
            _emit_json(payload, json_output=True)
        else:
            err = build_res.stderr or build_res.stdout
            if err:
                print(err, end="", file=sys.stderr)
        return build_res.returncode

    try:
        build_payload = json.loads(build_res.stdout.strip() or "{}")
    except json.JSONDecodeError:
        return _fail(
            "Failed to parse build JSON output.", json_output, command="compare"
        )
    output_str = build_payload.get("data", {}).get("output") or build_payload.get(
        "output"
    )
    if not output_str:
        return _fail(
            "Build output missing in JSON payload.", json_output, command="compare"
        )
    output_path = _resolve_binary_output(output_str)
    if output_path is None:
        return _fail(
            f"Compiled binary not found at {output_str}.",
            json_output,
            command="compare",
        )

    molt_res = _run_command_timed(
        [str(output_path), *script_args],
        env=env,
        cwd=project_root,
        verbose=verbose,
        capture_output=True,
        memory_guard_prefix=_DIFF_MEMORY_GUARD_PREFIX,
    )

    stdout_match = cpy_res.stdout == molt_res.stdout
    stderr_match = cpy_res.stderr == molt_res.stderr
    exit_match = cpy_res.returncode == molt_res.returncode
    compare_ok = stdout_match and stderr_match and exit_match

    if json_output:
        data = {
            "entry": str(source_path),
            "python": format_python_command(python_command),
            "output": str(output_path),
            "returncodes": {
                "cpython": cpy_res.returncode,
                "molt": molt_res.returncode,
                "build": build_res.returncode,
            },
            "match": {
                "stdout": stdout_match,
                "stderr": stderr_match,
                "exitcode": exit_match,
            },
            "timing": {
                "cpython_run_s": cpy_res.duration_s,
                "molt_build_s": build_res.duration_s,
                "molt_run_s": molt_res.duration_s,
                "molt_total_s": build_res.duration_s + molt_res.duration_s,
            },
            "cpython_stdout": cpy_res.stdout,
            "cpython_stderr": cpy_res.stderr,
            "molt_stdout": molt_res.stdout,
            "molt_stderr": molt_res.stderr,
        }
        payload = _json_payload(
            "compare",
            "ok" if compare_ok else "error",
            data=data,
        )
        _emit_json(payload, json_output=True)
        return 0 if compare_ok else 1

    print("Compare (CPython vs Molt):")
    print(
        f"- CPython run: {_format_duration(cpy_res.duration_s)} "
        f"(rc={cpy_res.returncode})"
    )
    print(f"- Molt build: {_format_duration(build_res.duration_s)}")
    print(
        f"- Molt run: {_format_duration(molt_res.duration_s)} "
        f"(rc={molt_res.returncode})"
    )
    total = build_res.duration_s + molt_res.duration_s
    print(f"- Molt total: {_format_duration(total)}")
    if cpy_res.duration_s > 0 and molt_res.duration_s > 0:
        speedup = cpy_res.duration_s / molt_res.duration_s
        print(f"- Molt speedup (run): {speedup:.2f}x")
    print(
        "- Output match: "
        f"stdout={'yes' if stdout_match else 'no'}, "
        f"stderr={'yes' if stderr_match else 'no'}, "
        f"exitcode={'yes' if exit_match else 'no'}"
    )
    if not compare_ok:
        if not stdout_match:
            print(
                f"- Stdout mismatch: CPython={len(cpy_res.stdout)} bytes, "
                f"Molt={len(molt_res.stdout)} bytes"
            )
        if not stderr_match:
            print(
                f"- Stderr mismatch: CPython={len(cpy_res.stderr)} bytes, "
                f"Molt={len(molt_res.stderr)} bytes"
            )
        if not exit_match:
            print(
                f"- Exitcode mismatch: CPython={cpy_res.returncode}, "
                f"Molt={molt_res.returncode}"
            )
        if verbose:
            print("CPython stdout:")
            print(cpy_res.stdout, end="" if cpy_res.stdout.endswith("\n") else "\n")
            print("Molt stdout:")
            print(molt_res.stdout, end="" if molt_res.stdout.endswith("\n") else "\n")
            print("CPython stderr:", file=sys.stderr)
            print(
                cpy_res.stderr,
                end="" if cpy_res.stderr.endswith("\n") else "\n",
                file=sys.stderr,
            )
            print("Molt stderr:", file=sys.stderr)
            print(
                molt_res.stderr,
                end="" if molt_res.stderr.endswith("\n") else "\n",
                file=sys.stderr,
            )
    return 0 if compare_ok else 1


def parity_run(
    file_path: str | None,
    module: str | None,
    python_exe: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    timing: bool = False,
) -> int:
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.",
            json_output,
            command="parity-run",
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="parity-run")

    source_path: Path | None = None
    if file_path:
        source_path = Path(file_path)
        if not source_path.exists():
            return _fail(
                f"File not found: {source_path}",
                json_output,
                command="parity-run",
            )

    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    molt_root = _find_molt_root(project_root, Path.cwd())
    env = _base_env(project_root, source_path, molt_root=molt_root)
    if file_path:
        env.update(_build_inputs._collect_env_overrides(file_path))

    try:
        python_command = resolve_python_selector(
            python_exe,
            env=env,
            cwd=project_root,
        )
    except (PythonInterpreterError, ValueError) as exc:
        return _fail(str(exc), json_output, command="parity-run")
    if module:
        command = [*python_command, "-m", module, *script_args]
    else:
        command = [*python_command, str(source_path), *script_args]

    if timing:
        run_res = _run_command_timed(
            command,
            env=env,
            cwd=project_root,
            verbose=verbose,
            capture_output=json_output,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
        if json_output:
            data: dict[str, Any] = {
                "python": format_python_command(python_command),
                "entry": module if module is not None else str(source_path),
                "returncode": run_res.returncode,
                "timing": {"cpython_run_s": run_res.duration_s},
            }
            if run_res.stdout:
                data["stdout"] = run_res.stdout
            if run_res.stderr:
                data["stderr"] = run_res.stderr
            payload = _json_payload(
                "parity-run",
                "ok" if run_res.returncode == 0 else "error",
                data=data,
            )
            _emit_json(payload, json_output=True)
        else:
            print("Timing (CPython parity-run):", file=sys.stderr)
            print(
                f"- run: {_format_duration(run_res.duration_s)} "
                f"(rc={run_res.returncode})",
                file=sys.stderr,
            )
        return run_res.returncode

    return _run_command(
        command,
        env=env,
        cwd=project_root,
        json_output=json_output,
        verbose=verbose,
        label="parity-run",
        memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
    )


def diff(
    file_path: str | None,
    python_version: str | None,
    build_profile: BuildProfile | None = None,
    trusted: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "diff")
    if root_error is not None:
        return root_error
    env = _base_env(root, molt_root=root)
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    cmd = [sys.executable, "tests/molt_diff.py"]
    if python_version:
        cmd.extend(["--python-version", python_version])
    if build_profile is not None:
        cmd.extend(["--build-profile", build_profile])
    if file_path:
        cmd.append(file_path)
    return _run_command(
        cmd,
        env=env,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="diff",
        memory_guard_prefix=_DIFF_MEMORY_GUARD_PREFIX,
    )
