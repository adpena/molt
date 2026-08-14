"""Batch build, lint, test, benchmark, and profile CLI commands."""

from __future__ import annotations

import io
import json
import shlex
import sys
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from typing import Any, Mapping, cast
from molt.dx import DEFAULT_UV_PROJECT_PYTHON, DxConfigError, DxProject
from molt.cli.command_runtime import (
    _CLI_MEMORY_GUARD_PREFIX,
    _run_completed_command,
)
from molt.cli.config_resolution import (
    DEFAULT_STDLIB_PROFILE,
    STDLIB_PROFILE_CHOICES,
)
from molt.cli.env_overrides import temporary_env_overrides as _temporary_env_overrides
from molt.cli.env_paths import _base_env
from molt.cli.models import (
    BuildProfile,
    EmitMode,
    FallbackPolicy,
    ParseCodec,
    Target,
    TypeHintPolicy,
)
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import (
    _find_molt_root,
    _require_molt_root,
)

from molt.cli.process_execution import _run_command


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
        cmd = [
            "uv",
            "run",
            "--python",
            DEFAULT_UV_PROJECT_PYTHON,
            "pytest",
            "-q",
        ]
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
