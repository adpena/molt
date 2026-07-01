from __future__ import annotations

import contextlib
import datetime as dt
import hashlib
import io
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from typing import Any, Mapping, Sequence, cast

from molt.dx import DxConfigError, DxProject
from molt.cli import build_inputs as _build_inputs
from molt.cli import source_extensions as _source_extensions
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
    _atomic_write_bytes,
    _atomic_write_json,
    _atomic_zip_file,
)
from molt.cli.backend_cache import _native_object_global_symbol_sets
from molt.cli.capability_spec import (
    CapabilityInput,
    _materialize_capabilities_arg,
    _parse_capabilities,
    _parse_capabilities_spec,
)
from molt.cli.command_runtime import (
    _CLI_MEMORY_GUARD_PREFIX,
    _CROSS_MEMORY_GUARD_PREFIX,
    _DIFF_MEMORY_GUARD_PREFIX,
    _run_completed_command,
)
from molt.cli.config_resolution import (
    DEFAULT_STDLIB_PROFILE,
    STDLIB_PROFILE_CHOICES,
    _config_value,
)
from molt.cli.c_api_symbols import is_c_api_symbol
from molt.cli.deps import _load_toml, _normalize_name
from molt.cli.env_overrides import temporary_env_overrides as _temporary_env_overrides
from molt.cli.env_paths import _base_env
from molt.cli.extension_manifest import (
    _MOLT_C_API_VERSION_RE,
    _coerce_str_list,
    _default_molt_c_api_version,
    _extension_binary_suffix,
    _host_target_triple,
    _infer_module_attr_callable_export_payloads,
    _manifest_callable_exports,
    _manifest_dotted_name_tuple,
    _manifest_support_file_payloads,
    _module_parts,
    _normalize_effects,
    _wheel_record_line,
    _wheel_token,
    _wheel_version_token,
    _write_zip_member,
)
from molt.cli.external_native import (
    _source_recompiled_external_package_root,
    _wasm_static_link_runtime_symbols_for_imports,
)
from molt.cli.file_hashing import _sha256_file
from molt.cli.lockfiles import _check_lockfiles
from molt.cli.models import (
    BuildProfile,
    EmitMode,
    FallbackPolicy,
    ParseCodec,
    Target,
    TypeHintPolicy,
    _TimedResult,
)
from molt.cli.native_toolchain import _zig_target_query
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import (
    _find_molt_root,
    _find_project_root,
    _require_molt_root,
)
from molt.cli.target_python import _parse_target_python_version
from molt.cli.setup_readiness import _ensure_rustup_target
from molt.cli.source_extension_toolchain import (
    _materialize_source_extension_target_metadata,
    _normalize_source_extension_abi_tier,
    _normalize_source_extension_metadata_target,
    _resolve_source_extension_wasm_toolchain,
    _source_extension_include_dirs_for_abi_tier,
    _source_extension_python_header_for_abi_tier,
)
from molt.cli.wrapper_build import (
    _build_args_has_python_version_flag,
    _run_wrapper_build,
)
from molt.cli.wasm_toolchain import normalize_wasi_sysroot, resolve_wasi_sysroot
from molt.wasm_artifact import read_wasm_function_exports, read_wasm_imports


def _resolve_python_exe(python_exe: str | None) -> str:
    if not python_exe:
        return sys.executable
    if python_exe[0].isdigit() and os.sep not in python_exe:
        python_exe = f"python{python_exe}"
    if os.sep in python_exe or Path(python_exe).is_absolute():
        candidate = Path(python_exe)
        if candidate.exists():
            return python_exe
        base_exe = getattr(sys, "_base_executable", "")
        if base_exe and Path(base_exe).exists():
            return base_exe
    return python_exe


def _sysroot_arg_value(args: list[str]) -> str | None:
    index = 0
    while index < len(args):
        part = args[index]
        if part in {"--sysroot", "-isysroot"}:
            if index + 1 < len(args):
                return args[index + 1]
            return ""
        if part.startswith("--sysroot="):
            return part.split("=", 1)[1]
        index += 1
    return None


def _extension_source_text_by_path(source_paths: list[Path]) -> dict[Path, str]:
    return {
        source_path: source_path.read_text(encoding="utf-8", errors="replace")
        for source_path in source_paths
    }


def _shared_library_defines_symbol(path: Path, symbol: str) -> tuple[bool, str | None]:
    symbol_sets = _native_object_global_symbol_sets(path)
    if symbol_sets is not None:
        defined, _undefined = symbol_sets
        if symbol in defined or f"_{symbol}" in defined:
            return True, None
        if defined:
            preview = ", ".join(sorted(defined)[:8])
            suffix = "" if len(defined) <= 8 else ", ..."
            return (
                False,
                f"symbol {symbol!r} missing from shared object (defined: {preview}{suffix})",
            )

    failures: list[str] = []
    export_commands = (
        ("llvm-readobj", "--coff-exports", str(path)),
        ("llvm-objdump", "-p", str(path)),
        ("dumpbin", "/EXPORTS", str(path)),
        ("objdump", "-p", str(path)),
    )
    for candidate in export_commands:
        tool = candidate[0]
        if shutil.which(tool) is None:
            continue
        try:
            result = _run_completed_command(
                list(candidate),
                cwd=path.parent,
                env=None,
                capture_output=True,
                timeout=10,
                memory_guard_prefix="MOLT_BUILD",
            )
        except (OSError, subprocess.SubprocessError) as exc:
            failures.append(f"{tool}: {exc}")
            continue
        text = "\n".join(part for part in (result.stdout, result.stderr) if part)
        if result.returncode == 0 and (symbol in text or f"_{symbol}" in text):
            return True, None
        detail = text.strip().splitlines()[:1]
        failures.append(f"{tool}: exit {result.returncode} {' '.join(detail)}")
    if failures:
        return False, "unable to confirm exported init symbol: " + "; ".join(failures)
    return False, "unable to inspect exported symbols with nm/llvm-nm or export-table tools"


def _run_command(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    json_output: bool = False,
    verbose: bool = False,
    label: str | None = None,
    warnings: list[str] | None = None,
    memory_guard_prefix: str | None = None,
) -> int:
    cmd = [str(part) for part in cmd]
    if verbose and not json_output:
        print(f"Running: {shlex.join(cmd)}", file=sys.stderr)
    capture = json_output
    result = _run_completed_command(
        cmd,
        env=env,
        cwd=cwd,
        capture_output=capture,
        memory_guard_prefix=memory_guard_prefix,
    )
    if json_output:
        data: dict[str, Any] = {"returncode": result.returncode}
        if result.stdout:
            data["stdout"] = result.stdout
        if result.stderr:
            data["stderr"] = result.stderr
        payload = _json_payload(
            label or cmd[0],
            "ok" if result.returncode == 0 else "error",
            data=data,
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    return result.returncode


def _run_command_timed(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    verbose: bool = False,
    capture_output: bool = False,
    memory_guard_prefix: str | None = None,
) -> _TimedResult:
    cmd = [str(part) for part in cmd]
    if verbose:
        print(f"Running: {shlex.join(cmd)}", file=sys.stderr)
    start = time.perf_counter()
    result = _run_completed_command(
        cmd,
        env=env,
        cwd=cwd,
        capture_output=capture_output,
        memory_guard_prefix=memory_guard_prefix,
    )
    duration = getattr(result, "elapsed_s", None)
    if duration is None:
        duration = time.perf_counter() - start
    return _TimedResult(
        result.returncode,
        result.stdout or "",
        result.stderr or "",
        duration,
    )


def _format_duration(seconds: float) -> str:
    if seconds < 0:
        seconds = 0.0
    if seconds < 0.001:
        return f"{seconds * 1_000_000:.0f} µs"
    if seconds < 1:
        return f"{seconds * 1000:.1f} ms"
    if seconds < 60:
        return f"{seconds:.3f} s"
    return f"{seconds / 60:.2f} min"


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
    python_exe = _resolve_python_exe(python_exe)
    if module:
        cpy_cmd = [python_exe, "-m", module, *script_args]
    else:
        cpy_cmd = [python_exe, str(source_path), *script_args]
    cpy_res = _run_command_timed(
        cpy_cmd,
        env=env,
        cwd=project_root,
        verbose=verbose,
        capture_output=True,
        memory_guard_prefix=_DIFF_MEMORY_GUARD_PREFIX,
    )

    build_args = list(build_args or [])
    if (
        requested_python_selector is not None
        and not _build_args_has_python_version_flag(build_args)
    ):
        with contextlib.suppress(ValueError):
            build_args.extend(
                [
                    "--python-version",
                    _parse_target_python_version(requested_python_selector).short,
                ]
            )
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
    build_cmd = [
        sys.executable,
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
            "python": python_exe,
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

    python_exe = _resolve_python_exe(python_exe)
    if module:
        command = [python_exe, "-m", module, *script_args]
    else:
        command = [python_exe, str(source_path), *script_args]

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
                "python": python_exe,
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


def _normalize_internal_batch_stdlib_profile(
    params: Mapping[str, Any],
) -> tuple[str | None, str | None]:
    raw = params.get("stdlib_profile", DEFAULT_STDLIB_PROFILE)
    if not isinstance(raw, str):
        return None, "stdlib_profile must be a string"
    if raw not in STDLIB_PROFILE_CHOICES:
        choices = "', '".join(STDLIB_PROFILE_CHOICES)
        return None, f"stdlib_profile must be one of '{choices}'"
    return raw, None


def _internal_batch_build_server(
    *,
    json_output: bool = False,
    verbose: bool = False,
    build_fn: Any | None = None,
) -> int:
    del json_output
    del verbose

    def _emit_response(payload: dict[str, Any]) -> None:
        sys.stdout.write(json.dumps(payload, sort_keys=True) + "\n")
        sys.stdout.flush()

    for raw_line in sys.stdin:
        if not raw_line.strip():
            continue
        req_id: Any = None
        try:
            request = json.loads(raw_line)
        except json.JSONDecodeError as exc:
            _emit_response(
                {
                    "id": None,
                    "ok": False,
                    "error": f"invalid request JSON: {exc}",
                }
            )
            continue
        if not isinstance(request, dict):
            _emit_response(
                {"id": None, "ok": False, "error": "request must be an object"}
            )
            continue
        req_id = request.get("id")
        op = request.get("op")
        if op == "ping":
            _emit_response({"id": req_id, "ok": True, "pong": True})
            continue
        if op == "shutdown":
            _emit_response({"id": req_id, "ok": True, "shutdown": True})
            return 0
        if op != "build":
            _emit_response(
                {"id": req_id, "ok": False, "error": f"unsupported op: {op!r}"}
            )
            continue

        params = request.get("params")
        if not isinstance(params, dict):
            _emit_response({"id": req_id, "ok": False, "error": "missing build params"})
            continue
        env_overrides_raw = params.get("env_overrides", {})
        if not isinstance(env_overrides_raw, dict) or any(
            not isinstance(key, str) or not isinstance(value, str)
            for key, value in env_overrides_raw.items()
        ):
            _emit_response(
                {
                    "id": req_id,
                    "ok": False,
                    "error": "env_overrides must be a string->string object",
                }
            )
            continue
        env_overrides: dict[str, str] = dict(env_overrides_raw)
        stdlib_profile, stdlib_profile_error = _normalize_internal_batch_stdlib_profile(
            params
        )
        if stdlib_profile_error is not None:
            _emit_response(
                {
                    "id": req_id,
                    "ok": False,
                    "error": stdlib_profile_error,
                }
            )
            continue
        assert stdlib_profile is not None
        env_overrides["MOLT_STDLIB_PROFILE"] = stdlib_profile
        stdout_buf = io.StringIO()
        stderr_buf = io.StringIO()
        try:
            with _temporary_env_overrides(env_overrides):
                with redirect_stdout(stdout_buf), redirect_stderr(stderr_buf):
                    if build_fn is None:
                        from molt import cli as _cli

                        active_build_fn = _cli.build
                    else:
                        active_build_fn = build_fn
                    rc = active_build_fn(
                        file_path=params.get("file_path"),
                        target=cast(Target, params.get("target", "native")),
                        parse_codec=cast(ParseCodec, params.get("codec", "msgpack")),
                        type_hint_policy=cast(
                            TypeHintPolicy, params.get("type_hints", "check")
                        ),
                        fallback_policy=cast(
                            FallbackPolicy, params.get("fallback", "error")
                        ),
                        type_facts_path=params.get("type_facts"),
                        pgo_profile=params.get("pgo_profile"),
                        runtime_feedback=params.get("runtime_feedback"),
                        output=params.get("output"),
                        json_output=bool(params.get("json_output", False)),
                        verbose=bool(params.get("verbose", False)),
                        deterministic=bool(params.get("deterministic", True)),
                        deterministic_warn=bool(
                            params.get("deterministic_warn", False)
                        ),
                        trusted=bool(params.get("trusted", False)),
                        capabilities=params.get("capabilities"),
                        cache=bool(params.get("cache", True)),
                        cache_dir=params.get("cache_dir"),
                        cache_report=bool(params.get("cache_report", False)),
                        sysroot=params.get("sysroot"),
                        emit_ir=params.get("emit_ir"),
                        emit=cast(EmitMode | None, params.get("emit")),
                        out_dir=params.get("out_dir"),
                        profile=cast(BuildProfile, params.get("profile", "dev")),
                        linked=bool(params.get("linked", False)),
                        linked_output=params.get("linked_output"),
                        require_linked=bool(params.get("require_linked", False)),
                        respect_pythonpath=bool(
                            params.get("respect_pythonpath", False)
                        ),
                        module=params.get("module"),
                        diagnostics_verbosity=params.get("diagnostics_verbosity"),
                        python_version=params.get("python_version"),
                        stdlib_profile=stdlib_profile,
                    )
        except Exception as exc:  # pragma: no cover - defensive server hardening
            _emit_response(
                {
                    "id": req_id,
                    "ok": False,
                    "error": f"batch build server exception: {exc}",
                    "stdout": stdout_buf.getvalue(),
                    "stderr": stderr_buf.getvalue(),
                }
            )
            continue
        _emit_response(
            {
                "id": req_id,
                "ok": rc == 0,
                "returncode": rc,
                "stdout": stdout_buf.getvalue(),
                "stderr": stderr_buf.getvalue(),
            }
        )
    return 0


def lint(json_output: bool = False, verbose: bool = False) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "lint")
    if root_error is not None:
        return root_error
    project = DxProject(root)
    try:
        env = project.canonical_env()
        project.require_project_python("lint", env)
        commands = project.split_command_sequence(
            project.commands().get("lint"),
            "lint",
            env=env,
        )
    except DxConfigError as exc:
        if json_output:
            _emit_json(
                _json_payload("lint", "error", errors=[str(exc)]),
                json_output=True,
            )
        else:
            print(f"lint: {exc}", file=sys.stderr)
        return 2
    results: list[dict[str, Any]] = []
    for cmd in commands:
        if verbose and not json_output:
            print(f"Running: {shlex.join(cmd)}", file=sys.stderr)
        result = _run_completed_command(
            [str(part) for part in cmd],
            cwd=root,
            env=env,
            capture_output=json_output,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
        result_data: dict[str, Any] = {
            "cmd": cmd,
            "returncode": result.returncode,
        }
        if json_output:
            if result.stdout:
                result_data["stdout"] = result.stdout
            if result.stderr:
                result_data["stderr"] = result.stderr
        results.append(result_data)
        if result.returncode != 0:
            if json_output:
                _emit_json(
                    _json_payload(
                        "lint",
                        "error",
                        data={"commands": results},
                    ),
                    json_output=True,
                )
            return result.returncode
    if json_output:
        _emit_json(
            _json_payload(
                "lint",
                "ok",
                data={"returncode": 0, "commands": results},
            ),
            json_output=True,
        )
    return 0


def test(
    suite: str,
    file_path: str | None,
    python_version: str | None,
    pytest_args: list[str],
    build_profile: BuildProfile | None = None,
    trusted: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "test")
    if root_error is not None:
        return root_error
    env = _base_env(root, molt_root=root)
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if suite == "dev":
        cmd = [sys.executable, "tools/dev.py", "test"]
    elif suite == "diff":
        cmd = [sys.executable, "tests/molt_diff.py"]
        if python_version:
            cmd.extend(["--python-version", python_version])
        if build_profile is not None:
            cmd.extend(["--build-profile", build_profile])
        if file_path:
            cmd.append(file_path)
    else:
        cmd = ["uv", "run", "--python", "3.12", "pytest", "-q"]
        if file_path:
            cmd.append(file_path)
        cmd.extend(pytest_args)
    return _run_command(
        cmd,
        env=env,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="test",
        memory_guard_prefix="MOLT_DIFF" if suite == "diff" else "MOLT_TEST_SUITE",
    )


def bench(
    wasm: bool,
    bench_args: list[str],
    bench_script: list[str] | None = None,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "bench")
    if root_error is not None:
        return root_error
    tool = "tools/bench_wasm.py" if wasm else "tools/bench.py"
    cmd = [sys.executable, tool]
    for script in bench_script or []:
        cmd.extend(["--script", script])
    cmd.extend(bench_args)
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="bench",
        memory_guard_prefix="MOLT_BENCH",
    )


def profile(
    profile_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "profile")
    if root_error is not None:
        return root_error
    cmd = [sys.executable, "tools/profile.py", *profile_args]
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="profile",
        memory_guard_prefix="MOLT_BENCH",
    )


def extension_metadata(
    *,
    target: str | None = None,
    out_dir: str | None = None,
    abi_tier: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    del verbose
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "extension-metadata")
    if root_error is not None:
        return root_error
    assert root is not None
    try:
        target_triple = _normalize_source_extension_metadata_target(target)
        normalized_abi_tier = _normalize_source_extension_abi_tier(abi_tier)
    except ValueError as exc:
        return _fail(str(exc), json_output, command="extension-metadata")

    output_root = Path(out_dir).expanduser() if out_dir else Path("dist")
    if not output_root.is_absolute():
        output_root = (Path.cwd() / output_root).absolute()
    metadata, errors = _materialize_source_extension_target_metadata(
        molt_root=root,
        out_dir=output_root,
        target_triple=target_triple,
        abi_tier=normalized_abi_tier,
    )
    if metadata is None:
        return _fail(
            "Source-extension target metadata errors: " + "; ".join(errors),
            json_output,
            command="extension-metadata",
        )
    data = dict(metadata.payload)
    data["paths"] = dict(data["paths"])
    data["paths"]["out_dir"] = str(metadata.out_dir)
    data["paths"]["pkg_config_dir"] = str(metadata.pkg_config_dir)
    data["paths"]["python_pc"] = str(metadata.python_pc)
    data["paths"]["meson_cross"] = str(metadata.meson_cross)
    data["paths"]["sidecar"] = str(metadata.sidecar)
    data["digest"] = metadata.digest
    if json_output:
        _emit_json(_json_payload("extension-metadata", "ok", data=data), True)
    else:
        print(f"Wrote source-extension target metadata: {metadata.sidecar}")
        print(f"Meson cross file: {metadata.meson_cross}")
        print(f"Python pkg-config: {metadata.python_pc}")
    return 0


def _source_plan_include_paths_for_abi(
    include_paths: Sequence[Path],
    *,
    python_header: Path,
) -> list[Path]:
    selected_python_header = python_header.resolve()
    filtered: list[Path] = []
    for include_path in include_paths:
        candidate_python_header = (include_path / "Python.h").resolve()
        if (
            candidate_python_header.is_file()
            and candidate_python_header != selected_python_header
        ):
            continue
        filtered.append(include_path)
    return filtered


def _source_plan_abi_include_order(
    abi_include_roots: Sequence[Path],
    *,
    python_header: Path,
) -> tuple[Path, list[Path]]:
    python_include_root = python_header.resolve().parent
    fallback_roots: list[Path] = []
    seen = {python_include_root}
    for include_root in abi_include_roots:
        resolved = include_root.resolve()
        if resolved in seen:
            continue
        seen.add(resolved)
        fallback_roots.append(include_root)
    return python_include_root, fallback_roots


def _target_pointer_size_bytes(target_triple: str | None) -> int | None:
    target = (target_triple or "").lower()
    if target.startswith("wasm32"):
        return 4
    if target.startswith("wasm64"):
        return 8
    return None


def _source_plan_target_fact_overlay_include_paths(
    include_paths: Sequence[Path],
    *,
    build_tmp: Path,
    target_triple: str | None,
) -> list[Path]:
    pointer_size = _target_pointer_size_bytes(target_triple)
    if pointer_size is None:
        return []
    for include_path in include_paths:
        numpy_config = include_path / "_numpyconfig.h"
        if not numpy_config.is_file():
            continue
        try:
            text = numpy_config.read_text(encoding="utf-8")
        except OSError:
            continue
        if (
            "#define NPY_SIZEOF_PY_INTPTR_T -1" not in text
            and "#define NPY_SIZEOF_PY_LONG_LONG -1" not in text
        ):
            continue
        overlay_dir = build_tmp / "source_plan_target_facts" / "numpy_core"
        overlay_dir.mkdir(parents=True, exist_ok=True)
        (overlay_dir / "_numpyconfig.h").write_text(
            "\n".join(
                [
                    "#ifndef MOLT_SOURCE_EXTENSION_NUMPY_TARGET_FACTS_OVERLAY_H",
                    "#define MOLT_SOURCE_EXTENSION_NUMPY_TARGET_FACTS_OVERLAY_H",
                    '#include_next "_numpyconfig.h"',
                    "#if defined(NPY_SIZEOF_PY_INTPTR_T) && NPY_SIZEOF_PY_INTPTR_T < 0",
                    "#undef NPY_SIZEOF_PY_INTPTR_T",
                    f"#define NPY_SIZEOF_PY_INTPTR_T {pointer_size}",
                    "#endif",
                    "#if defined(NPY_SIZEOF_PY_LONG_LONG) && NPY_SIZEOF_PY_LONG_LONG < 0",
                    "#undef NPY_SIZEOF_PY_LONG_LONG",
                    "#define NPY_SIZEOF_PY_LONG_LONG 8",
                    "#endif",
                    "#endif",
                    "",
                ]
            ),
            encoding="utf-8",
        )
        return [overlay_dir]
    return []


_SOURCE_EXTENSION_CPP_SUFFIXES = {".cc", ".cpp", ".cxx", ".c++", ".mm"}


def _tool_basename_variant(path: str, *, basename: str) -> str:
    candidate = Path(path)
    replacement = basename + candidate.suffix
    if candidate.parent == Path("."):
        return replacement
    return str(candidate.with_name(replacement))


def _source_extension_compile_command_for_source(
    *,
    source_path: Path,
    cc_cmd: Sequence[str],
) -> list[str]:
    command = list(cc_cmd)
    if source_path.suffix.lower() not in _SOURCE_EXTENSION_CPP_SUFFIXES or not command:
        return command

    tool = Path(command[0]).name.lower()
    if tool in {"zig", "zig.exe"}:
        if len(command) >= 2 and command[1] in {"cc", "c++"}:
            command[1] = "c++"
        else:
            command.insert(1, "c++")
        return command

    if tool in {"clang", "clang.exe"}:
        variant = _tool_basename_variant(command[0], basename="clang++")
        resolved = shutil.which(variant) or shutil.which(Path(variant).name)
        if resolved is not None:
            command[0] = resolved
        else:
            command[0] = variant
    return command


def _extension_export_package(module_parts: list[str]) -> str:
    return module_parts[0]


def _extension_export_config_errors(errors: list[str]) -> list[str]:
    return [
        error.replace("extension_manifest.json", "tool.molt.extension", 1)
        for error in errors
    ]


def _extension_manifest_public_exports(
    extension_meta: Mapping[str, Any],
    *,
    package: str,
    errors: list[str],
) -> tuple[list[str], list[dict[str, Any]]]:
    export_manifest = {
        "python_exports": extension_meta.get("python_exports")
        or extension_meta.get("python-exports"),
        "callable_exports": extension_meta.get("callable_exports")
        or extension_meta.get("callable-exports"),
    }
    export_errors: list[str] = []
    python_exports = _manifest_dotted_name_tuple(
        export_manifest,
        "python_exports",
        package=package,
        errors=export_errors,
    )
    callable_exports = _manifest_callable_exports(
        export_manifest,
        package=package,
        errors=export_errors,
    )
    errors.extend(_extension_export_config_errors(export_errors))
    return (
        list(python_exports),
        [export.digest_payload() for export in callable_exports],
    )


def extension_build(
    project: str | None = None,
    out_dir: str | None = None,
    module: str | None = None,
    molt_abi: str | None = None,
    capabilities: CapabilityInput | None = None,
    provided_capsules: str | list[str] | None = None,
    python_export: str | list[str] | None = None,
    callable_export_json: str | list[str] | None = None,
    support_file: Any = None,
    deterministic: bool = True,
    profile: BuildProfile = "release",
    target: str | None = None,
    source_plan: str | None = None,
    source_plan_target: str | None = None,
    source_plan_source_root: str | None = None,
    source_plan_build_root: str | None = None,
    source_plan_compile_commands: str | None = None,
    abi_tier: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    project_root = Path(project).expanduser() if project else Path.cwd()
    if not project_root.is_absolute():
        project_root = (Path.cwd() / project_root).absolute()
    if not project_root.exists() or not project_root.is_dir():
        return _fail(
            f"Project directory not found: {project_root}",
            json_output,
            command="extension-build",
        )

    pyproject = _load_toml(project_root / "pyproject.toml")
    project_meta = pyproject.get("project")
    extension_meta_raw = _config_value(pyproject, ["tool", "molt", "extension"])
    errors: list[str] = []
    warnings: list[str] = []
    cli_extension_config = any(
        value is not None
        for value in (
            module,
            source_plan,
            source_plan_target,
            source_plan_source_root,
            source_plan_build_root,
            source_plan_compile_commands,
            abi_tier,
            provided_capsules,
            python_export,
            callable_export_json,
            support_file,
        )
    )

    if not isinstance(project_meta, dict):
        return _fail(
            "pyproject.toml must contain a [project] table.",
            json_output,
            command="extension-build",
        )
    if not isinstance(extension_meta_raw, dict):
        if cli_extension_config:
            extension_meta: dict[str, Any] = {}
        else:
            return _fail(
                "pyproject.toml must contain [tool.molt.extension], or pass "
                "--module and --source-plan for an upstream build-plan target.",
                json_output,
                command="extension-build",
            )
    else:
        extension_meta = extension_meta_raw

    if source_plan is not None and module is None and "module" not in extension_meta:
        return _fail(
            "--source-plan requires --module when [tool.molt.extension].module "
            "is not configured.",
            json_output,
            command="extension-build",
        )

    cli_python_exports = _coerce_str_list(
        python_export,
        "--python-export",
        errors,
    )
    callable_export_json_items = _coerce_str_list(
        callable_export_json,
        "--callable-export-json",
        errors,
    )
    cli_callable_exports: list[dict[str, Any]] = []
    for index, item in enumerate(callable_export_json_items):
        try:
            payload = json.loads(item)
        except json.JSONDecodeError as exc:
            errors.append(f"--callable-export-json[{index}] must be JSON: {exc}")
            continue
        if not isinstance(payload, dict):
            errors.append(f"--callable-export-json[{index}] must be a JSON object")
            continue
        cli_callable_exports.append(payload)
    extension_export_meta: dict[str, Any] = dict(extension_meta)
    if cli_python_exports:
        extension_export_meta["python_exports"] = cli_python_exports
    if cli_callable_exports:
        extension_export_meta["callable_exports"] = cli_callable_exports
    raw_support_files: list[Any] = []
    extension_support_files = extension_meta.get("support_files") or extension_meta.get(
        "support-files"
    )
    if extension_support_files is not None:
        if isinstance(extension_support_files, list):
            raw_support_files.extend(extension_support_files)
        else:
            errors.append("tool.molt.extension.support_files must be a list")
    if support_file is not None:
        if isinstance(support_file, str):
            raw_support_files.append(support_file)
        elif isinstance(support_file, list):
            raw_support_files.extend(support_file)
        else:
            errors.append("--support-file must be a string or list")

    project_name = project_meta.get("name")
    project_version = project_meta.get("version")
    if not isinstance(project_name, str) or not project_name.strip():
        errors.append("project.name must be a non-empty string")
    if not isinstance(project_version, str) or not project_version.strip():
        errors.append("project.version must be a non-empty string")

    module_name = module or extension_meta.get("module")
    if not isinstance(module_name, str):
        errors.append("tool.molt.extension.module or --module must be a string")
        module_name = ""
    module_parts = _module_parts(module_name)
    if module_parts is None:
        errors.append("tool.molt.extension.module must be a dotted Python identifier")
        module_parts = ["extension"]
    support_files = _manifest_support_file_payloads(
        raw_support_files,
        field_name="tool.molt.extension.support_files",
        root=project_root,
        errors=errors,
    )

    source_paths: list[Path] = []
    include_paths: list[Path] = []
    compile_args: list[str] = []
    link_args: list[str] = []
    loaded_source_plan: _source_extensions._SourceExtensionBuildPlan | None = None
    source_plan_config_raw = extension_meta.get("source_plan") or extension_meta.get(
        "source-plan"
    )
    source_plan_config: dict[str, Any] | None = None
    if source_plan_config_raw is not None:
        if not isinstance(source_plan_config_raw, dict):
            errors.append("tool.molt.extension.source_plan must be a table")
        else:
            source_plan_config = dict(source_plan_config_raw)
    if source_plan is not None:
        source_plan_config = dict(source_plan_config or {})
        source_plan_config["path"] = source_plan
    if source_plan_target is not None:
        source_plan_config = dict(source_plan_config or {})
        source_plan_config["target"] = source_plan_target
    if source_plan_source_root is not None:
        source_plan_config = dict(source_plan_config or {})
        source_plan_config["source_root"] = source_plan_source_root
    if source_plan_build_root is not None:
        source_plan_config = dict(source_plan_config or {})
        source_plan_config["build_root"] = source_plan_build_root
    if source_plan_compile_commands is not None:
        source_plan_config = dict(source_plan_config or {})
        source_plan_config["compile_commands"] = source_plan_compile_commands

    if source_plan_config is None:
        raw_sources = _coerce_str_list(
            extension_meta.get("sources"),
            "tool.molt.extension.sources",
            errors,
            allow_empty=False,
        )
        if not raw_sources:
            errors.append(
                "tool.molt.extension.sources must include at least one source"
            )
        for entry in raw_sources:
            source_path = Path(entry).expanduser()
            if not source_path.is_absolute():
                source_path = (project_root / source_path).absolute()
            if not source_path.exists() or not source_path.is_file():
                errors.append(f"source file not found: {source_path}")
                continue
            source_paths.append(source_path)

        include_dirs_raw = _coerce_str_list(
            extension_meta.get("include_dirs") or extension_meta.get("include-dirs"),
            "tool.molt.extension.include_dirs",
            errors,
        )
        for entry in include_dirs_raw:
            include_path = Path(entry).expanduser()
            if not include_path.is_absolute():
                include_path = (project_root / include_path).absolute()
            include_paths.append(include_path)

        compile_args = _coerce_str_list(
            extension_meta.get("extra_compile_args")
            or extension_meta.get("extra-compile-args"),
            "tool.molt.extension.extra_compile_args",
            errors,
        )
        link_args = _coerce_str_list(
            extension_meta.get("extra_link_args")
            or extension_meta.get("extra-link-args"),
            "tool.molt.extension.extra_link_args",
            errors,
        )
    else:
        manual_authority_fields = [
            field
            for field in (
                "sources",
                "include_dirs",
                "include-dirs",
                "extra_compile_args",
                "extra-compile-args",
                "extra_link_args",
                "extra-link-args",
            )
            if field in extension_meta
        ]
        if manual_authority_fields:
            errors.append(
                "tool.molt.extension.source_plan plus compile_commands.json is "
                "the source/arg authority; remove parallel manual fields: "
                + ", ".join(sorted(manual_authority_fields))
            )
        if not errors:
            loaded_source_plan, source_plan_errors = (
                _source_extensions._load_source_extension_build_plan(
                    project_root=project_root,
                    module_name=module_name,
                    plan_config=source_plan_config,
                )
            )
            errors.extend(source_plan_errors)
        if loaded_source_plan is not None:
            source_paths = [
                unit.source_path for unit in loaded_source_plan.compile_units
            ]
            include_paths = list(loaded_source_plan.include_dirs)
            compile_args = list(loaded_source_plan.compile_args)
            link_args = list(loaded_source_plan.link_args)

    provided_capsules_input: str | list[str] | None = provided_capsules
    if provided_capsules_input is None:
        configured_provided_capsules = extension_meta.get(
            "provided_capsules"
        ) or extension_meta.get("provided-capsules")
    else:
        configured_provided_capsules = provided_capsules_input
    provided_capsules_tuple = tuple(
        sorted(
            set(
                _coerce_str_list(
                    configured_provided_capsules,
                    "tool.molt.extension.provided_capsules",
                    errors,
                )
            )
        )
    )
    try:
        normalized_abi_tier = _normalize_source_extension_abi_tier(
            abi_tier or extension_meta.get("abi_tier") or extension_meta.get("abi-tier")
        )
    except ValueError as exc:
        errors.append(str(exc))
        normalized_abi_tier = "source-compat"

    effects = _normalize_effects(extension_meta.get("effects"))
    determinism_mode = "deterministic" if deterministic else "nondet"
    determinism_raw = extension_meta.get("determinism")
    if determinism_raw is not None:
        if not isinstance(determinism_raw, str):
            errors.append(
                "tool.molt.extension.determinism must be 'deterministic' or 'nondet'"
            )
        else:
            normalized = determinism_raw.strip().lower()
            if normalized not in {"deterministic", "nondet"}:
                errors.append(
                    "tool.molt.extension.determinism must be 'deterministic' or "
                    "'nondet'"
                )
            else:
                determinism_mode = normalized
    if deterministic:
        determinism_mode = "deterministic"

    requested_target = (target or "native").strip()
    if not requested_target:
        requested_target = "native"
    normalized_target = requested_target.lower()
    runtime_target_triple: str | None = None
    manifest_target_triple = _host_target_triple()
    wasm_static_link = False
    if normalized_target == "native":
        runtime_target_triple = None
    elif normalized_target == "wasm":
        wasm_static_link = True
        runtime_target_triple = "wasm32-wasip1"
        manifest_target_triple = runtime_target_triple
    elif normalized_target.startswith("wasm32"):
        if any(ch.isspace() for ch in requested_target):
            errors.append("target must be 'native', 'wasm', or a Rust target triple")
        wasm_static_link = True
        runtime_target_triple = normalized_target
        manifest_target_triple = normalized_target
    else:
        if any(ch.isspace() for ch in requested_target):
            errors.append("target must be 'native', 'wasm', or a Rust target triple")
        runtime_target_triple = normalized_target
        manifest_target_triple = normalized_target
    if loaded_source_plan is not None:
        errors.extend(
            _source_extensions._validate_source_extension_build_plan_target(
                loaded_source_plan,
                target_triple=runtime_target_triple,
            )
        )

    capability_input: CapabilityInput | None = capabilities
    if capability_input is None:
        cfg_capabilities = extension_meta.get("capabilities")
        if isinstance(cfg_capabilities, (str, list, dict)):
            capability_input = cfg_capabilities
    if capability_input is None:
        errors.append(
            "Missing extension capabilities: set tool.molt.extension.capabilities "
            "or pass --capabilities."
        )
    capabilities_list: list[str] = []
    capability_profiles: list[str] = []
    if capability_input is not None:
        spec = _parse_capabilities_spec(capability_input)
        if spec.errors:
            errors.append("Invalid capabilities: " + ", ".join(spec.errors))
        else:
            capabilities_list = spec.capabilities or []
            capability_profiles = spec.profiles
    python_exports, callable_exports = _extension_manifest_public_exports(
        extension_export_meta,
        package=_extension_export_package(module_parts),
        errors=errors,
    )
    source_recompiled_root = _source_recompiled_external_package_root(
        ".".join(module_parts)
    )
    if wasm_static_link and source_recompiled_root and not (
        python_exports or callable_exports
    ):
        errors.append(
            "WASM source-recompiled extension builds for "
            f"{source_recompiled_root!r} must declare tool.molt.extension."
            "python_exports or tool.molt.extension.callable_exports; native "
            "artifact reachability is manifest-symbol custody, not package "
            "directory ancestry"
        )

    cwd_root = _find_project_root(Path.cwd())
    molt_root = _find_molt_root(project_root, cwd_root)
    root_error = _require_molt_root(molt_root, json_output, "extension-build")
    if root_error is not None:
        return root_error

    lock_error = _check_lockfiles(
        molt_root,
        json_output,
        warnings,
        deterministic,
        False,
        "extension-build",
    )
    if lock_error is not None:
        return lock_error

    default_abi = _default_molt_c_api_version(molt_root)
    abi_raw = molt_abi or extension_meta.get("molt_c_api_version") or default_abi
    if not isinstance(abi_raw, str):
        errors.append("molt ABI must be a string")
        abi_raw = default_abi
    abi_version = abi_raw.strip()
    if _MOLT_C_API_VERSION_RE.match(abi_version) is None:
        errors.append(
            "Invalid molt ABI version. Expected MAJOR[.MINOR[.PATCH]] "
            f"(got {abi_version!r})."
        )
    abi_major = abi_version.split(".", 1)[0] if abi_version else "0"
    abi_tag = f"molt_abi{abi_major}"

    if errors:
        return _fail(
            "Extension build configuration errors: " + "; ".join(errors),
            json_output,
            command="extension-build",
        )

    source_c_api_requirements: (
        _source_extensions._SourceExtensionCAPIRequirements | None
    ) = None
    if loaded_source_plan is None:
        try:
            source_text_by_path = _extension_source_text_by_path(source_paths)
            inferred_callable_exports = _infer_module_attr_callable_export_payloads(
                source_text_by_path.values(),
                python_exports=python_exports,
                explicit_callable_exports=callable_exports,
                effects=effects,
                deterministic=determinism_mode == "deterministic",
            )
            if inferred_callable_exports:
                callable_exports = [
                    *callable_exports,
                    *[dict(export) for export in inferred_callable_exports],
                ]
                callable_exports = sorted(
                    callable_exports,
                    key=lambda export: (
                        str(export.get("module")),
                        str(export.get("name")),
                    ),
                )
        except OSError as exc:
            return _fail(
                f"Failed scanning extension C/API source surface: {exc}",
                json_output,
                command="extension-build",
            )

    output_root = Path(out_dir).expanduser() if out_dir else Path("dist")
    if not output_root.is_absolute():
        output_root = (project_root / output_root).absolute()
    output_root.mkdir(parents=True, exist_ok=True)

    if runtime_target_triple:
        _ensure_rustup_target(runtime_target_triple, warnings)

    abi_include_roots = _source_extension_include_dirs_for_abi_tier(
        molt_root=molt_root,
        abi_tier=normalized_abi_tier,
    )
    missing_abi_include_roots = [
        include_root for include_root in abi_include_roots if not include_root.exists()
    ]
    if missing_abi_include_roots:
        return _fail(
            "Missing Molt ABI header roots: "
            + ", ".join(str(path) for path in missing_abi_include_roots),
            json_output,
            command="extension-build",
        )
    python_header = _source_extension_python_header_for_abi_tier(
        molt_root=molt_root,
        abi_tier=normalized_abi_tier,
    )
    if loaded_source_plan is None:
        source_c_api_requirements, capi_error = (
            _source_extensions._source_extension_required_c_api_by_source(
                molt_root=molt_root,
                source_paths=source_paths,
                python_header=python_header,
                definition_header_roots=[
                    *include_paths,
                    project_root,
                    *(source_path.parent for source_path in source_paths),
                ],
                compile_args_by_source={
                    source_path: compile_args for source_path in source_paths
                },
                preprocessor_defined_symbols=[
                    (
                        "MOLT_EXTENSION_WASM_STATIC_LINK"
                        if wasm_static_link
                        else "MOLT_EXTENSION_HOST_ABI"
                    )
                ],
            )
        )
        if capi_error is not None:
            return _fail(capi_error, json_output, command="extension-build")
        assert source_c_api_requirements is not None
        missing_c_api = list(source_c_api_requirements.missing_symbols)
        fail_fast_c_api = list(source_c_api_requirements.fail_fast_symbols)
        if missing_c_api or fail_fast_c_api:
            details: list[str] = []
            if missing_c_api:
                details.append("missing: " + ", ".join(missing_c_api[:16]))
            if fail_fast_c_api:
                details.append("fail-fast: " + ", ".join(fail_fast_c_api[:16]))
            return _fail(
                "Reachable source extension C/API symbols are unsupported ("
                + "; ".join(details)
                + ")",
                json_output,
                command="extension-build",
            )

    cc = os.environ.get("CC", "clang")
    cc_cmd = shlex.split(cc)
    if not cc_cmd:
        return _fail(
            "Compiler command is empty. Set CC or install clang.",
            json_output,
            command="extension-build",
        )
    wasi_sysroot: Path | None = None
    if wasm_static_link:
        if loaded_source_plan is not None:
            wasm_toolchain = _resolve_source_extension_wasm_toolchain()
            if not wasm_toolchain.ok:
                return _fail(
                    "WASM source-extension build requires a valid wasm compiler "
                    "and linker toolchain: " + wasm_toolchain.detail,
                    json_output,
                    command="extension-build",
                )
            cc_cmd = list(wasm_toolchain.compiler_cmd)
            target_arg = runtime_target_triple or "wasm32-wasip1"
            if wasm_toolchain.compiler_kind == "zig":
                normalized = _zig_target_query(target_arg)
                if normalized != target_arg:
                    warnings.append(
                        f"Zig target normalized to {normalized} from {target_arg}."
                    )
                target_arg = normalized
            cc_cmd.extend(["-target", target_arg])
            wasi_sysroot = wasm_toolchain.wasi_sysroot
        else:
            wasm_cc = os.environ.get("MOLT_WASM_CC")
            if wasm_cc:
                cc_cmd = shlex.split(wasm_cc)
            if not cc_cmd:
                return _fail(
                    "Compiler command is empty. Set MOLT_WASM_CC, CC, or install clang.",
                    json_output,
                    command="extension-build",
                )
            target_arg = runtime_target_triple or "wasm32-wasip1"
            has_target_arg = any(
                part == "-target" or part.startswith("--target") for part in cc_cmd
            )
            if not has_target_arg:
                tool_name = Path(cc_cmd[0]).name.lower()
                if tool_name in {"zig", "zig.exe"}:
                    cc_cmd.extend(["-target", target_arg])
                else:
                    cc_cmd.append(f"--target={target_arg}")
            explicit_sysroot = _sysroot_arg_value([*cc_cmd, *compile_args])
            if explicit_sysroot is None:
                wasi_sysroot = resolve_wasi_sysroot()
                if wasi_sysroot is None:
                    return _fail(
                        "WASM extension build requires a WASI sysroot containing "
                        "include/errno.h. Set MOLT_WASI_SYSROOT, WASI_SYSROOT, "
                        "or WASI_SDK_PATH.",
                        json_output,
                        command="extension-build",
                    )
                cc_cmd.append(f"--sysroot={wasi_sysroot}")
            else:
                wasi_sysroot = normalize_wasi_sysroot(explicit_sysroot)
                if wasi_sysroot is None:
                    return _fail(
                        "WASM extension build sysroot is invalid or incomplete: "
                        f"{explicit_sysroot!r} does not contain include/errno.h.",
                        json_output,
                        command="extension-build",
                    )
        if loaded_source_plan is None and "-wasm-enable-sjlj" not in [
            *cc_cmd,
            *compile_args,
        ]:
            cc_cmd.extend(["-mllvm", "-wasm-enable-sjlj"])
    elif runtime_target_triple:
        cross_cc = os.environ.get("MOLT_CROSS_CC")
        target_arg = runtime_target_triple
        if cross_cc:
            cc_cmd = shlex.split(cross_cc)
        elif shutil.which("zig"):
            cc_cmd = ["zig", "cc"]
            normalized = _zig_target_query(runtime_target_triple)
            if normalized != runtime_target_triple:
                warnings.append(
                    f"Zig target normalized to {normalized} from {runtime_target_triple}."
                )
            target_arg = normalized
        else:
            return _fail(
                "Cross-target extension build requires zig or MOLT_CROSS_CC "
                f"(missing for {runtime_target_triple}).",
                json_output,
                command="extension-build",
            )
        if not cc_cmd:
            return _fail(
                "Compiler command is empty. Set MOLT_CROSS_CC or install zig.",
                json_output,
                command="extension-build",
            )
        cc_cmd.extend(["-target", target_arg])

    dist_name = _normalize_name(str(project_name)).replace("-", "_")
    wheel_version = _wheel_version_token(str(project_version))
    target_triple = manifest_target_triple
    platform_tag = _wheel_token(target_triple)
    python_tag = "py3"
    wheel_name = (
        f"{dist_name}-{wheel_version}-{python_tag}-{abi_tag}-{platform_tag}.whl"
    )
    wheel_path = output_root / wheel_name

    build_env = os.environ.copy()
    # Supply-chain: always set SOURCE_DATE_EPOCH for release builds for reproducibility
    if deterministic or profile == "release":
        build_env.setdefault("SOURCE_DATE_EPOCH", "315532800")

    if wasm_static_link:
        module_rel = Path(*module_parts[:-1], module_parts[-1] + ".molt.wasm")
    else:
        module_rel = Path(
            *module_parts[:-1],
            module_parts[-1] + _extension_binary_suffix(runtime_target_triple),
        )
    init_symbol = f"PyInit_{module_parts[-1]}"
    compile_commands: list[list[str]] = []
    link_command: list[str] = []
    wasi_sysroot_path = str(wasi_sysroot) if wasm_static_link else None
    wasm_defined_symbols: list[str] = []
    wasm_import_symbols: list[str] = []

    with tempfile.TemporaryDirectory(prefix="molt_ext_build_", dir=output_root) as td:
        build_tmp = Path(td)
        object_paths: list[Path] = []
        object_facts: list[_source_extensions._SourceExtensionObjectFact] = []
        for idx, source_path in enumerate(source_paths):
            plan_unit = (
                loaded_source_plan.compile_units[idx]
                if loaded_source_plan is not None
                else None
            )
            unit_include_paths = (
                list(plan_unit.include_dirs) if plan_unit is not None else include_paths
            )
            if plan_unit is not None:
                unit_include_paths = _source_plan_include_paths_for_abi(
                    unit_include_paths,
                    python_header=python_header,
                )
                unit_include_paths = [
                    *_source_plan_target_fact_overlay_include_paths(
                        unit_include_paths,
                        build_tmp=build_tmp,
                        target_triple=runtime_target_triple,
                    ),
                    *unit_include_paths,
                ]
            unit_compile_args = (
                list(plan_unit.compile_args) if plan_unit is not None else compile_args
            )
            unit_cc_cmd = (
                _source_extension_compile_command_for_source(
                    source_path=source_path,
                    cc_cmd=cc_cmd,
                )
                if plan_unit is not None
                else list(cc_cmd)
            )
            object_path = build_tmp / f"{idx}_{source_path.stem}.o"
            cmd = [*unit_cc_cmd, "-c", str(source_path), "-o", str(object_path)]
            if wasm_static_link:
                cmd.append("-DMOLT_EXTENSION_WASM_STATIC_LINK=1")
            else:
                cmd.append("-DMOLT_EXTENSION_HOST_ABI=1")
            if plan_unit is not None:
                python_include_root, fallback_abi_include_roots = (
                    _source_plan_abi_include_order(
                        abi_include_roots,
                        python_header=python_header,
                    )
                )
                cmd.extend(["-I", str(python_include_root)])
                cmd.extend(["-I", str(project_root)])
                for include_path in unit_include_paths:
                    cmd.extend(["-I", str(include_path)])
                for include_path in fallback_abi_include_roots:
                    cmd.extend(["-I", str(include_path)])
            else:
                for include_path in abi_include_roots:
                    cmd.extend(["-I", str(include_path)])
                cmd.extend(["-I", str(project_root)])
                for include_path in unit_include_paths:
                    cmd.extend(["-I", str(include_path)])
            if os.name != "nt" and not wasm_static_link:
                cmd.append("-fPIC")
            if deterministic:
                prefix = str(project_root)
                cmd.append(f"-ffile-prefix-map={prefix}=.")
                cmd.append(f"-fdebug-prefix-map={prefix}=.")
            if plan_unit is not None:
                cmd.extend(
                    _source_extensions._source_extension_gc_compile_args(
                        target_triple=runtime_target_triple,
                    )
                )
                cmd.extend(
                    _source_extensions._source_extension_wasm_compile_args(
                        target_triple=runtime_target_triple,
                        cc_cmd=unit_cc_cmd,
                    )
                )
            cmd.extend(unit_compile_args)
            result = _run_completed_command(
                cmd,
                cwd=project_root,
                env=build_env,
                capture_output=True,
                memory_guard_prefix="MOLT_BUILD",
            )
            if result.returncode != 0:
                detail = result.stderr.strip() or result.stdout.strip()
                if not detail:
                    detail = f"compiler exited with code {result.returncode}"
                return _fail(
                    f"Failed compiling {source_path.name}: {detail}",
                    json_output,
                    command="extension-build",
                )
            compile_commands.append(cmd)
            object_paths.append(object_path)
            if loaded_source_plan is not None:
                object_fact, object_fact_error = (
                    _source_extensions._source_extension_object_fact(
                        source_path=source_path,
                        object_path=object_path,
                    )
                )
                if object_fact_error is not None:
                    return _fail(
                        object_fact_error,
                        json_output,
                        command="extension-build",
                    )
                assert object_fact is not None
                object_facts.append(object_fact)

        source_plan_object_closure: (
            _source_extensions._SourceExtensionObjectClosure | None
        ) = None
        if loaded_source_plan is not None:
            source_plan_object_closure, object_closure_errors = (
                _source_extensions._compute_source_extension_object_closure(
                    init_symbol=init_symbol,
                    object_facts=object_facts,
                )
            )
            if object_closure_errors:
                return _fail(
                    "Source extension object closure errors: "
                    + "; ".join(object_closure_errors),
                    json_output,
                    command="extension-build",
                )
            assert source_plan_object_closure is not None
            object_paths = [
                fact.object_path for fact in source_plan_object_closure.objects
            ]
            (
                source_c_api_requirements,
                capi_error,
            ) = _source_extensions._source_extension_required_c_api_by_source(
                molt_root=molt_root,
                source_paths=[
                    fact.source_path for fact in source_plan_object_closure.objects
                ],
                python_header=python_header,
                definition_header_roots=[
                    *loaded_source_plan.include_dirs,
                    *(
                        fact.source_path.parent
                        for fact in source_plan_object_closure.objects
                    ),
                ],
                compile_args_by_source={
                    unit.source_path: unit.compile_args
                    for unit in loaded_source_plan.compile_units
                },
                preprocessor_defined_symbols=[
                    (
                        "MOLT_EXTENSION_WASM_STATIC_LINK"
                        if wasm_static_link
                        else "MOLT_EXTENSION_HOST_ABI"
                    )
                ],
            )
            if capi_error is not None:
                return _fail(capi_error, json_output, command="extension-build")
            assert source_c_api_requirements is not None
            missing_c_api = list(source_c_api_requirements.missing_symbols)
            fail_fast_c_api = list(source_c_api_requirements.fail_fast_symbols)
            if missing_c_api or fail_fast_c_api:
                details: list[str] = []
                if missing_c_api:
                    details.append("missing: " + ", ".join(missing_c_api[:16]))
                if fail_fast_c_api:
                    details.append("fail-fast: " + ", ".join(fail_fast_c_api[:16]))
                return _fail(
                    "Reachable source extension C/API symbols are unsupported ("
                    + "; ".join(details)
                    + ")",
                    json_output,
                    command="extension-build",
                )

        built_extension = build_tmp / module_rel
        built_extension.parent.mkdir(parents=True, exist_ok=True)
        if wasm_static_link:
            if link_args and loaded_source_plan is None:
                warnings.append(
                    "Ignoring extra_link_args for wasm relocatable object output; "
                    "static-link custody is resolved by the final wasm linker."
                )
            if loaded_source_plan is None and len(object_paths) == 1:
                _atomic_copy_file(object_paths[0], built_extension)
            else:
                wasm_ld_cmd = shlex.split(os.environ.get("MOLT_WASM_LD", "wasm-ld"))
                if not wasm_ld_cmd:
                    return _fail(
                        "Compiler command is empty. Set MOLT_WASM_LD or install wasm-ld.",
                        json_output,
                        command="extension-build",
                    )
                link_command = [
                    *wasm_ld_cmd,
                    "-r",
                    *(
                        ["--allow-undefined", "--no-entry"]
                        if loaded_source_plan is not None
                        else []
                    ),
                    *[str(path) for path in object_paths],
                    "-o",
                    str(built_extension),
                ]
                if loaded_source_plan is not None:
                    link_command.extend(link_args)
                link_result = _run_completed_command(
                    link_command,
                    cwd=project_root,
                    env=build_env,
                    capture_output=True,
                    memory_guard_prefix="MOLT_BUILD",
                )
                if link_result.returncode != 0:
                    detail = link_result.stderr.strip() or link_result.stdout.strip()
                    if not detail:
                        detail = f"wasm linker exited with code {link_result.returncode}"
                    return _fail(
                        f"Failed linking wasm relocatable extension object: {detail}",
                        json_output,
                        command="extension-build",
                    )
        else:
            link_command = [*cc_cmd, "-shared"]
            link_command.extend(str(path) for path in object_paths)
            link_command.extend(["-o", str(built_extension)])
            if sys.platform == "darwin" and runtime_target_triple is None:
                link_command.extend(["-undefined", "dynamic_lookup"])
            elif (runtime_target_triple and "linux" in runtime_target_triple) or (
                runtime_target_triple is None and sys.platform.startswith("linux")
            ):
                link_command.append("-ldl")
            if loaded_source_plan is not None:
                link_command.extend(
                    _source_extensions._source_extension_gc_link_args(
                        cc_cmd=cc_cmd,
                        target_triple=runtime_target_triple,
                    )
                )
            link_command.extend(link_args)
            link_result = _run_completed_command(
                link_command,
                cwd=project_root,
                env=build_env,
                capture_output=True,
                memory_guard_prefix="MOLT_BUILD",
            )
            if link_result.returncode != 0:
                detail = link_result.stderr.strip() or link_result.stdout.strip()
                if not detail:
                    detail = f"linker exited with code {link_result.returncode}"
                return _fail(
                    f"Failed linking extension module: {detail}",
                    json_output,
                    command="extension-build",
                )

        if not built_extension.exists():
            return _fail(
                "Link succeeded but extension artifact is missing.",
                json_output,
                command="extension-build",
            )
        if wasm_static_link:
            if source_plan_object_closure is not None:
                wasm_defined_symbols = sorted(
                    {
                        symbol
                        for fact in source_plan_object_closure.objects
                        for symbol in fact.defined_symbols
                    }
                )
                wasm_import_symbols = list(source_plan_object_closure.runtime_symbols)
            else:
                try:
                    wasm_defined_symbols = sorted(
                        {
                            export.name
                            for export in read_wasm_function_exports(built_extension)
                        }
                    )
                    wasm_import_symbols = sorted(
                        {
                            wasm_import.name
                            for wasm_import in read_wasm_imports(built_extension)
                        }
                    )
                except (OSError, UnicodeDecodeError, ValueError, IndexError) as exc:
                    return _fail(
                        "Built wasm extension artifact is not a readable wasm object: "
                        f"{exc}",
                        json_output,
                        command="extension-build",
                    )
            direct_symbols = sorted(
                {
                    str(export.get("symbol"))
                    for export in callable_exports
                    if export.get("binding") == "direct_symbol"
                    and isinstance(export.get("symbol"), str)
                    and str(export.get("symbol")).strip()
                }
            )
            missing_direct_symbols = [
                symbol for symbol in direct_symbols if symbol not in wasm_defined_symbols
            ]
            if missing_direct_symbols:
                return _fail(
                    "WASM extension direct_symbol callable export(s) missing from "
                    f"function exports: {', '.join(missing_direct_symbols)}",
                    json_output,
                    command="extension-build",
                )
            _atomic_copy_file(built_extension, output_root / module_rel)
        else:
            symbol_ok, symbol_error = _shared_library_defines_symbol(
                built_extension,
                init_symbol,
            )
            if not symbol_ok:
                return _fail(
                    f"Linked extension is not importable: {symbol_error}",
                    json_output,
                    command="extension-build",
                )

        extension_bytes = built_extension.read_bytes()
        extension_archive_path = module_rel.as_posix()
        runtime_linkage = "static_link" if wasm_static_link else "host_resolved"
        artifact_kind = (
            "wasm_relocatable_object" if wasm_static_link else "shared_library"
        )
        build_payload: dict[str, Any] = {
            "compiler": cc_cmd,
            "compiler_target": runtime_target_triple or "native",
            "wasi_sysroot": wasi_sysroot_path,
            "runtime_linkage": runtime_linkage,
            "artifact_kind": artifact_kind,
            "include_dirs": [str(path) for path in abi_include_roots]
            + [str(project_root)]
            + [str(path) for path in include_paths],
            "python_header": str(python_header),
            "extra_compile_args": compile_args,
            "extra_link_args": link_args,
        }
        manifest_payload: dict[str, Any] = {
            "schema_version": 1,
            "name": str(project_name),
            "version": str(project_version),
            "module": ".".join(module_parts),
            "sources": [str(path) for path in source_paths],
            "molt_c_api_version": abi_version,
            "abi_tag": abi_tag,
            "abi_tier": normalized_abi_tier,
            "python_tag": python_tag,
            "target_triple": target_triple,
            "platform_tag": platform_tag,
            "loader_kind": "libmolt_source",
            "init_symbol": init_symbol,
            "runtime_linkage": runtime_linkage,
            "artifact_kind": artifact_kind,
            "provided_capsules": list(provided_capsules_tuple),
            "capabilities": capabilities_list,
            "capability_profiles": capability_profiles,
            "deterministic": deterministic,
            "determinism": determinism_mode,
            "effects": effects,
            "wheel": wheel_name,
            "extension": extension_archive_path,
            "support_files": [entry.digest_payload() for entry in support_files],
            "build": build_payload,
        }
        if source_c_api_requirements is not None:
            build_payload["source_c_api_scan"] = (
                source_c_api_requirements.manifest_payload()
            )
        if loaded_source_plan is not None:
            manifest_payload["source_plan"] = loaded_source_plan.manifest_payload()
            build_payload["source_plan_digest"] = loaded_source_plan.digest
            build_payload["object_count"] = len(object_facts)
            build_payload["linked_object_count"] = len(object_paths)
            assert source_plan_object_closure is not None
            build_payload["object_closure_sha256"] = (
                source_plan_object_closure.closure_sha256
            )
            assert source_c_api_requirements is not None
            manifest_payload["object_closure"] = (
                source_plan_object_closure.manifest_payload(
                    required_c_api_by_source=(
                        source_c_api_requirements.required_by_source
                    ),
                    required_capsules_by_source=(
                        source_c_api_requirements.required_capsules_by_source
                    ),
                    project_generated_c_api_by_source=(
                        source_c_api_requirements.project_generated_c_api_by_source
                    ),
                    project_generated_c_api_prefixes=(
                        source_c_api_requirements.project_generated_c_api_prefixes
                    ),
                )
            )
        elif wasm_static_link:
            object_closure: dict[str, Any] = {
                "defined_symbols": wasm_defined_symbols,
                "undefined_symbols": wasm_import_symbols,
            }
            runtime_symbols = _wasm_static_link_runtime_symbols_for_imports(
                wasm_import_symbols
            )
            if runtime_symbols:
                object_closure["runtime_symbols"] = list(runtime_symbols)
            required_c_api_symbols = sorted(
                set(
                    symbol
                    for symbols in (
                        source_c_api_requirements.required_by_source.values()
                        if source_c_api_requirements is not None
                        else ()
                    )
                    for symbol in symbols
                )
                | {
                    symbol
                    for symbol in wasm_import_symbols
                    if is_c_api_symbol(symbol)
                }
            )
            if required_c_api_symbols:
                object_closure["required_c_api_symbols"] = required_c_api_symbols
            if source_c_api_requirements is not None:
                required_capsules = sorted(
                    {
                        capsule
                        for capsules in (
                            source_c_api_requirements.required_capsules_by_source.values()
                        )
                        for capsule in capsules
                    }
                )
                if required_capsules:
                    object_closure["required_capsules"] = required_capsules
                project_generated_symbols = sorted(
                    {
                        symbol
                        for symbols in (
                            source_c_api_requirements.project_generated_c_api_by_source.values()
                        )
                        for symbol in symbols
                    }
                )
                if project_generated_symbols:
                    object_closure["project_generated_c_api_symbols"] = (
                        project_generated_symbols
                    )
                if source_c_api_requirements.project_generated_c_api_prefixes:
                    object_closure["project_generated_c_api_prefixes"] = list(
                        source_c_api_requirements.project_generated_c_api_prefixes
                    )
            manifest_payload["object_closure"] = object_closure
        if python_exports:
            manifest_payload["python_exports"] = python_exports
        if callable_exports:
            manifest_payload["callable_exports"] = callable_exports
        if not support_files:
            manifest_payload.pop("support_files", None)
        manifest_bytes = (
            json.dumps(manifest_payload, sort_keys=True, indent=2).encode("utf-8")
            + b"\n"
        )

        dist_info = f"{dist_name}-{wheel_version}.dist-info"
        wheel_metadata = "\n".join(
            [
                "Wheel-Version: 1.0",
                "Generator: molt extension build",
                "Root-Is-Purelib: false",
                f"Tag: {python_tag}-{abi_tag}-{platform_tag}",
                "",
            ]
        ).encode("utf-8")
        package_metadata = "\n".join(
            [
                "Metadata-Version: 2.1",
                f"Name: {project_name}",
                f"Version: {project_version}",
                "Summary: Molt C extension package",
                "",
            ]
        ).encode("utf-8")

        wheel_entries: list[tuple[str, bytes]] = [
            (extension_archive_path, extension_bytes),
            ("extension_manifest.json", manifest_bytes),
            (f"{dist_info}/WHEEL", wheel_metadata),
            (f"{dist_info}/METADATA", package_metadata),
        ]
        for support in support_files:
            wheel_entries.append(
                (support.rel_path, support.source_path.read_bytes())
            )
        record_path = f"{dist_info}/RECORD"
        record_lines = [_wheel_record_line(path, data) for path, data in wheel_entries]
        record_lines.append(f"{record_path},,")
        record_bytes = ("\n".join(record_lines) + "\n").encode("utf-8")

        with _atomic_zip_file(wheel_path) as zf:
            for path, data in wheel_entries:
                _write_zip_member(zf, path, data)
            _write_zip_member(zf, record_path, record_bytes)

    wheel_sha = _sha256_file(wheel_path)
    extension_sha = hashlib.sha256(extension_bytes).hexdigest()
    sidecar_payload = dict(manifest_payload)
    sidecar_payload["wheel_sha256"] = wheel_sha
    sidecar_payload["extension_sha256"] = extension_sha
    if deterministic:
        sidecar_payload["generated_at_utc"] = "1970-01-01T00:00:00Z"
    else:
        sidecar_payload["generated_at_utc"] = (
            dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()
        )
    manifest_path = output_root / "extension_manifest.json"
    _atomic_write_json(manifest_path, sidecar_payload, sort_keys=True, indent=2)
    extracted_extension_path = output_root / extension_archive_path
    _atomic_write_bytes(extracted_extension_path, extension_bytes)
    extracted_package_init_files: list[Path] = []
    for index in range(1, len(module_parts)):
        source_init = project_root.joinpath(*module_parts[:index], "__init__.py")
        if not source_init.exists() or not source_init.is_file():
            continue
        dest_init = output_root.joinpath(*module_parts[:index], "__init__.py")
        _atomic_copy_file(source_init, dest_init)
        extracted_package_init_files.append(dest_init)
    extracted_support_files: list[Path] = []
    for support in support_files:
        dest_support = output_root / Path(support.rel_path)
        _atomic_copy_file(support.source_path, dest_support)
        extracted_support_files.append(dest_support)
    artifact_manifest_payload = dict(sidecar_payload)
    artifact_manifest_payload["extension"] = extracted_extension_path.name
    artifact_manifest_path = extracted_extension_path.with_name(
        extracted_extension_path.name + ".extension_manifest.json"
    )
    _atomic_write_json(
        artifact_manifest_path,
        artifact_manifest_payload,
        sort_keys=True,
        indent=2,
    )

    if json_output:
        payload = _json_payload(
            "extension-build",
            "ok",
            data={
                "project": str(project_root),
                "wheel": str(wheel_path),
                "manifest": str(manifest_path),
                "extension_artifact": str(extracted_extension_path),
                "artifact_manifest": str(artifact_manifest_path),
                "extracted_package_init_files": [
                    str(path) for path in extracted_package_init_files
                ],
                "extracted_support_files": [
                    str(path) for path in extracted_support_files
                ],
                "module": ".".join(module_parts),
                "molt_c_api_version": abi_version,
                "abi_tag": abi_tag,
                "abi_tier": normalized_abi_tier,
                "target_triple": target_triple,
                "build_target": runtime_target_triple or "native",
                "platform_tag": platform_tag,
                "runtime_linkage": runtime_linkage,
                "artifact_kind": artifact_kind,
                "deterministic": deterministic,
                "determinism": determinism_mode,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "wheel_sha256": wheel_sha,
                "extension_sha256": extension_sha,
                "object_closure_sha256": (
                    source_plan_object_closure.closure_sha256
                    if source_plan_object_closure is not None
                    else None
                ),
                "object_count": len(object_facts),
                "linked_object_count": len(object_paths),
                "provided_capsules": list(provided_capsules_tuple),
                "source_plan_digest": (
                    loaded_source_plan.digest
                    if loaded_source_plan is not None
                    else None
                ),
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    else:
        print(f"Built extension wheel: {wheel_path}")
        print(f"Wrote extension manifest: {manifest_path}")
        print(f"Wrote extension artifact: {extracted_extension_path}")
        print(f"Wrote artifact manifest: {artifact_manifest_path}")
        if verbose:
            print(f"Target triple: {target_triple}")
            print(f"Build target: {runtime_target_triple or 'native'}")
            print(f"Molt C API version: {abi_version}")
            print(f"Extension ABI tier: {normalized_abi_tier}")
            print(f"Capabilities: {json.dumps(capabilities_list)}")
            print(f"Compile steps: {len(compile_commands)}")
            if source_plan_object_closure is not None:
                print(
                    "Source extension object closure: "
                    f"{len(source_plan_object_closure.objects)}/{len(object_facts)} "
                    "objects"
                )
            if extracted_package_init_files:
                print(
                    "Copied package init files: "
                    + ", ".join(str(path) for path in extracted_package_init_files)
                )
    return 0
