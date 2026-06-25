from __future__ import annotations

import argparse
import copy
import hashlib
import importlib
import io
import json
import os
from pathlib import Path
import shlex
import subprocess
from contextlib import redirect_stdout
from typing import Any, Mapping, cast

from molt.cli.capability_spec import _split_tokens
from molt.cli.models import BuildProfile

from molt.debug import (
    DebugFailureClass,
    DebugStatus,
    DebugSubcommand,
    allocate_debug_paths,
    normalize_debug_payload,
    render_debug_json_summary,
    render_debug_text_summary,
    write_debug_manifest,
)
from molt.debug.bisect import bisect_backend_profile_ic, bisect_first_bad_pass
from molt.debug.diff import (
    build_diff_summary_payload,
    load_diff_summary,
    load_failure_queue,
    render_diff_text,
)
from molt.debug.ir import capture_ir_snapshots, render_ir_text
from molt.debug.perf import build_perf_summary_payload, load_profile, render_perf_text
from molt.debug.reduce import normalize_failure_oracle
from molt.debug.reduce import (
    build_candidate_manifest,
    build_reduction_payload,
    load_reduction_input,
    reduce_source_text,
)
from molt.debug.trace import normalize_trace_families
from molt.debug.verify import build_verify_result_payload, run_default_verify_checks


def _cli_module() -> Any:
    return importlib.import_module("molt.cli")


def _atomic_write_text(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._atomic_write_text(*args, **kwargs)


def _emit_debug_payload(
    *,
    payload: dict[str, Any],
    format_name: str,
    retained_output: Path | None,
    rendered_text: str | None = None,
) -> int:
    write_debug_manifest(Path(payload["manifest_path"]), payload)
    if format_name == "json":
        summary = render_debug_json_summary(payload)
    else:
        summary = (
            rendered_text
            if rendered_text is not None
            else render_debug_text_summary(payload)
        )
    if retained_output is not None:
        _atomic_write_text(retained_output, summary)
    print(summary, end="")
    return 0


def _capture_json_cli_result(
    runner: Any,
    /,
    *args: Any,
    **kwargs: Any,
) -> tuple[int, dict[str, Any] | None]:
    stdout_buffer = io.StringIO()
    with redirect_stdout(stdout_buffer):
        returncode = runner(*args, json_output=True, **kwargs)
    stdout_text = stdout_buffer.getvalue().strip()
    if not stdout_text:
        return returncode, None
    payload = json.loads(stdout_text)
    if not isinstance(payload, dict):
        return returncode, None
    return returncode, payload


def _load_debug_oracle(args: argparse.Namespace) -> dict[str, Any]:
    oracle_json = getattr(args, "oracle_json", None)
    oracle_file = getattr(args, "oracle_file", None)
    if oracle_json and oracle_file:
        raise ValueError("use --oracle-json or --oracle-file, not both")
    if oracle_file:
        oracle_payload = json.loads(Path(oracle_file).read_text(encoding="utf-8"))
    elif oracle_json:
        oracle_payload = json.loads(oracle_json)
    else:
        raise ValueError("missing oracle; use --oracle-json or --oracle-file")
    return normalize_failure_oracle(oracle_payload)


def _merge_debug_manifest(
    base_manifest: dict[str, Any],
    extra_manifest: Any,
) -> dict[str, Any]:
    merged = copy.deepcopy(base_manifest)
    if not isinstance(extra_manifest, Mapping):
        return merged
    for key, value in extra_manifest.items():
        if (
            key in merged
            and isinstance(merged[key], dict)
            and isinstance(value, Mapping)
        ):
            merged[key] = {**merged[key], **value}
        else:
            merged[key] = value
    return merged


def _debug_eval_base_env(cwd: Path) -> dict[str, str]:
    base_env: dict[str, str] = {}
    passthrough_names = {
        "ALL_PROXY",
        "COMSPEC",
        "HOME",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "NO_PROXY",
        "PATH",
        "PATHEXT",
        "PYTHONPATH",
        "SSL_CERT_DIR",
        "SSL_CERT_FILE",
        "SYSTEMROOT",
        "TEMP",
        "TERM",
        "TMP",
        "TMPDIR",
        "USERPROFILE",
        "VIRTUAL_ENV",
        "WINDIR",
    }
    for name in passthrough_names:
        value = os.environ.get(name)
        if value:
            base_env[name] = value

    ext_root = os.environ.get("MOLT_EXT_ROOT", str(cwd))
    cargo_target_dir = os.environ.get(
        "CARGO_TARGET_DIR", str(Path(ext_root) / "target")
    )
    base_env.update(
        {
            "MOLT_EXT_ROOT": ext_root,
            "CARGO_TARGET_DIR": cargo_target_dir,
            "MOLT_DIFF_CARGO_TARGET_DIR": os.environ.get(
                "MOLT_DIFF_CARGO_TARGET_DIR",
                cargo_target_dir,
            ),
            "MOLT_CACHE": os.environ.get(
                "MOLT_CACHE", str(Path(ext_root) / ".molt_cache")
            ),
            "MOLT_DIFF_ROOT": os.environ.get(
                "MOLT_DIFF_ROOT",
                str(Path(ext_root) / "tmp" / "diff"),
            ),
            "MOLT_DIFF_TMPDIR": os.environ.get(
                "MOLT_DIFF_TMPDIR",
                str(Path(ext_root) / "tmp"),
            ),
            "UV_CACHE_DIR": os.environ.get(
                "UV_CACHE_DIR",
                str(Path(ext_root) / ".uv-cache"),
            ),
            "TMPDIR": os.environ.get("TMPDIR", str(Path(ext_root) / "tmp")),
            "MOLT_SESSION_ID": os.environ.get("MOLT_SESSION_ID", "debug-eval"),
            "PYTHONHASHSEED": os.environ.get("PYTHONHASHSEED", "0"),
        }
    )
    return base_env


def _run_debug_eval_command(
    command: str | None,
    *,
    cwd: Path,
    env_updates: Mapping[str, str],
    default_manifest: dict[str, Any],
    timeout_sec: int,
) -> dict[str, Any]:
    cli_module = _cli_module()
    evaluation: dict[str, Any] = {
        "manifest": copy.deepcopy(default_manifest),
    }
    if not command:
        return evaluation

    env = _debug_eval_base_env(cwd)
    env.update(env_updates)
    try:
        proc = cli_module._run_completed_command(
            shlex.split(command, posix=True),
            cwd=cwd,
            env=env,
            capture_output=True,
            timeout=max(1, timeout_sec),
            memory_guard_prefix=cli_module._CLI_MEMORY_GUARD_PREFIX,
        )
    except subprocess.TimeoutExpired as exc:
        evaluation.update(
            {
                "classification": "nonzero_exit",
                "stdout": cli_module._coerce_process_text(exc.stdout),
                "stderr": (
                    cli_module._coerce_process_text(exc.stderr)
                    + f"\nevaluator timed out after {max(1, timeout_sec)}s"
                ).strip(),
                "returncode": 124,
                "timed_out": True,
            }
        )
        return evaluation
    stdout = proc.stdout or ""
    stderr = proc.stderr or ""
    timed_out = proc.returncode == 124 and "memory_guard: timeout" in stderr
    evaluation.update(
        {
            "classification": "nonzero_exit" if proc.returncode else "zero_exit",
            "stdout": stdout,
            "stderr": stderr,
            "returncode": proc.returncode,
            "timed_out": timed_out,
        }
    )
    parsed_stdout: dict[str, Any] | None = None
    if stdout.strip():
        try:
            candidate = json.loads(stdout)
        except json.JSONDecodeError:
            candidate = None
        if isinstance(candidate, dict):
            parsed_stdout = candidate
    if parsed_stdout is not None:
        if "manifest" in parsed_stdout:
            evaluation["manifest"] = _merge_debug_manifest(
                default_manifest,
                parsed_stdout.get("manifest"),
            )
        for key, value in parsed_stdout.items():
            if key == "manifest":
                continue
            evaluation[key] = value
    return evaluation


def _handle_debug_ir(
    args: argparse.Namespace,
    *,
    subcommand: DebugSubcommand,
    paths: Any,
    selectors: dict[str, Any],
) -> int:
    source_path = Path(args.source)
    if not source_path.is_file():
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message=f"{source_path} is not a file",
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )

    source = source_path.read_text(encoding="utf-8")
    result = capture_ir_snapshots(
        source,
        source_path=source_path,
        stage=args.stage,
        function_name=args.function,
        module_name=args.module,
        pass_name=args.pass_name,
    )
    payload = normalize_debug_payload(
        subcommand=subcommand,
        status=DebugStatus.OK,
        run_id=paths.run_id,
        artifact_root=paths.artifact_root,
        manifest_path=paths.manifest_path,
        selectors=selectors,
        retained_output=paths.retained_output,
        data=result,
    )
    rendered_text = None
    if args.format != "json":
        rendered_text = render_ir_text(result)
    return _emit_debug_payload(
        payload=payload,
        format_name=args.format,
        retained_output=paths.retained_output,
        rendered_text=rendered_text,
    )


def _handle_debug_verify(
    args: argparse.Namespace,
    *,
    subcommand: DebugSubcommand,
    paths: Any,
    selectors: dict[str, Any],
) -> int:
    checks, errors = run_default_verify_checks(
        require_probe_execution=getattr(args, "require_probe_execution", False),
        probe_rss_metrics=(
            Path(args.probe_rss_metrics).expanduser()
            if getattr(args, "probe_rss_metrics", None)
            else None
        ),
        probe_run_id=getattr(args, "probe_run_id", None),
        failure_queue=(
            Path(args.failure_queue).expanduser()
            if getattr(args, "failure_queue", None)
            else None
        ),
    )
    result_payload = build_verify_result_payload(checks)
    payload = normalize_debug_payload(
        subcommand=subcommand,
        status=DebugStatus.OK if not errors else DebugStatus.ERROR,
        run_id=paths.run_id,
        artifact_root=paths.artifact_root,
        manifest_path=paths.manifest_path,
        selectors=selectors,
        retained_output=paths.retained_output,
        failure_class=DebugFailureClass.INTERNAL_ERROR if errors else None,
        message=None if not errors else "verification checks reported errors",
        data=result_payload,
    )
    return _emit_debug_payload(
        payload=payload,
        format_name=args.format,
        retained_output=paths.retained_output,
    )


def _handle_debug_diff(
    args: argparse.Namespace,
    *,
    subcommand: DebugSubcommand,
    paths: Any,
    selectors: dict[str, Any],
) -> int:
    summary_path = Path(args.summary_path)
    if not summary_path.is_file():
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message=f"{summary_path} is not a file",
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )
    failure_queue = (
        load_failure_queue(Path(args.failure_queue)) if args.failure_queue else []
    )
    summary = build_diff_summary_payload(
        load_diff_summary(summary_path),
        failures=failure_queue,
    )
    payload = normalize_debug_payload(
        subcommand=subcommand,
        status=DebugStatus.OK,
        run_id=paths.run_id,
        artifact_root=paths.artifact_root,
        manifest_path=paths.manifest_path,
        selectors=selectors,
        retained_output=paths.retained_output,
        data=summary,
    )
    rendered_text = None if args.format == "json" else render_diff_text(summary)
    return _emit_debug_payload(
        payload=payload,
        format_name=args.format,
        retained_output=paths.retained_output,
        rendered_text=rendered_text,
    )


def _handle_debug_perf(
    args: argparse.Namespace,
    *,
    subcommand: DebugSubcommand,
    paths: Any,
    selectors: dict[str, Any],
) -> int:
    profiles: dict[str, dict[str, Any]] = {}
    missing: list[str] = []
    empty: list[str] = []
    for file_arg in args.files:
        path = Path(file_arg)
        if not path.exists():
            missing.append(str(path))
            continue
        profile = load_profile(path)
        if profile is None:
            empty.append(str(path))
            continue
        profiles[path.stem] = profile
    if missing or empty or not profiles:
        issues = []
        issues.extend(f"missing file: {path}" for path in missing)
        issues.extend(f"no profile data found in: {path}" for path in empty)
        if not profiles and not issues:
            issues.append("no profile inputs provided")
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message="; ".join(issues),
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )
    summary = build_perf_summary_payload(profiles)
    payload = normalize_debug_payload(
        subcommand=subcommand,
        status=DebugStatus.OK,
        run_id=paths.run_id,
        artifact_root=paths.artifact_root,
        manifest_path=paths.manifest_path,
        selectors=selectors,
        retained_output=paths.retained_output,
        data=summary,
    )
    rendered_text = None if args.format == "json" else render_perf_text(summary)
    return _emit_debug_payload(
        payload=payload,
        format_name=args.format,
        retained_output=paths.retained_output,
        rendered_text=rendered_text,
    )


def _handle_debug_repro(
    args: argparse.Namespace,
    *,
    subcommand: DebugSubcommand,
    paths: Any,
    selectors: dict[str, Any],
) -> int:
    source_path = Path(args.source)
    if not source_path.is_file():
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message=f"{source_path} is not a file",
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )

    source_text = source_path.read_text(encoding="utf-8")
    build_args: list[str] = []
    if args.backend:
        build_args.extend(["--backend", args.backend])
    if getattr(args, "rebuild", False):
        build_args.append("--no-cache")
    profile = args.profile or "dev"
    cli_module = _cli_module()
    if args.compare:
        inner_rc, inner_payload = cli_module._capture_json_cli_result(
            cli_module.compare,
            str(source_path),
            None,
            args.python,
            [],
            verbose=False,
            trusted=False,
            capabilities=None,
            build_args=build_args,
            rebuild=getattr(args, "rebuild", False),
            build_profile=cast(BuildProfile | None, profile),
        )
        mode = "compare"
    else:
        inner_rc, inner_payload = cli_module._capture_json_cli_result(
            cli_module.run_script,
            str(source_path),
            None,
            [],
            verbose=False,
            timing=True,
            trusted=False,
            capabilities=None,
            build_args=build_args,
            build_profile=cast(BuildProfile | None, profile),
        )
        mode = "run"
    if inner_payload is None:
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INTERNAL_ERROR,
            message="debug repro did not produce a JSON payload",
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )

    inner_status = inner_payload.get("status")
    payload = normalize_debug_payload(
        subcommand=subcommand,
        status=DebugStatus.OK if inner_status == "ok" else DebugStatus.ERROR,
        run_id=paths.run_id,
        artifact_root=paths.artifact_root,
        manifest_path=paths.manifest_path,
        selectors=selectors,
        retained_output=paths.retained_output,
        failure_class=None
        if inner_status == "ok"
        else DebugFailureClass.INTERNAL_ERROR,
        message=None if inner_status == "ok" else f"debug repro {mode} failed",
        data={
            "mode": mode,
            "source_path": str(source_path),
            "source_sha256": hashlib.sha256(source_text.encode("utf-8")).hexdigest(),
            "profile": profile,
            "backend": args.backend,
            "build_args": build_args,
            "execution": inner_payload,
            "returncode": inner_rc,
        },
    )
    return _emit_debug_payload(
        payload=payload,
        format_name=args.format,
        retained_output=paths.retained_output,
    )


def _handle_debug_trace(
    args: argparse.Namespace,
    *,
    subcommand: DebugSubcommand,
    paths: Any,
    selectors: dict[str, Any],
) -> int:
    source_path = Path(args.source)
    if not source_path.is_file():
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message=f"{source_path} is not a file",
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )
    try:
        trace_config = normalize_trace_families(args.family)
    except ValueError as exc:
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message=str(exc),
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )

    source_text = source_path.read_text(encoding="utf-8")
    build_args: list[str] = []
    if args.backend:
        build_args.extend(["--backend", args.backend])
    if getattr(args, "rebuild", False):
        build_args.append("--no-cache")
    profile = args.profile or "dev"
    trace_env = dict(trace_config.env)
    trace_env["MOLT_ASSERT_NO_PENDING_ON_SUCCESS"] = (
        "1" if getattr(args, "assert_no_pending_on_success", False) else "0"
    )
    cli_module = _cli_module()
    with cli_module._temporary_env_overrides(trace_env):
        inner_rc, inner_payload = cli_module._capture_json_cli_result(
            cli_module.run_script,
            str(source_path),
            None,
            [],
            verbose=False,
            timing=True,
            trusted=False,
            capabilities=None,
            build_args=build_args,
            build_profile=cast(BuildProfile | None, profile),
        )
    if inner_payload is None:
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INTERNAL_ERROR,
            message="debug trace did not produce a JSON payload",
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )

    inner_status = inner_payload.get("status")
    payload = normalize_debug_payload(
        subcommand=subcommand,
        status=DebugStatus.OK if inner_status == "ok" else DebugStatus.ERROR,
        run_id=paths.run_id,
        artifact_root=paths.artifact_root,
        manifest_path=paths.manifest_path,
        selectors=selectors,
        retained_output=paths.retained_output,
        failure_class=None
        if inner_status == "ok"
        else DebugFailureClass.INTERNAL_ERROR,
        message=None if inner_status == "ok" else "debug trace execution failed",
        data={
            "mode": "run",
            "source_path": str(source_path),
            "source_sha256": hashlib.sha256(source_text.encode("utf-8")).hexdigest(),
            "profile": profile,
            "backend": args.backend,
            "families": list(trace_config.families),
            "assertions": (
                ["no_pending_on_success"]
                if getattr(args, "assert_no_pending_on_success", False)
                else []
            ),
            "trace_env": trace_env,
            "execution": inner_payload,
            "returncode": inner_rc,
        },
    )
    return _emit_debug_payload(
        payload=payload,
        format_name=args.format,
        retained_output=paths.retained_output,
    )


def _handle_debug_reduce(
    args: argparse.Namespace,
    *,
    subcommand: DebugSubcommand,
    paths: Any,
    selectors: dict[str, Any],
) -> int:
    input_path = Path(args.input_path)
    if not input_path.is_file():
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message=f"{input_path} is not a file",
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )
    try:
        oracle = _load_debug_oracle(args)
    except (ValueError, json.JSONDecodeError) as exc:
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message=str(exc),
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )
    if args.eval_command is None and oracle["kind"] != "manifest_predicate":
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message="--eval-command is required for non-manifest reduction oracles",
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )

    reduction_input = load_reduction_input(input_path)
    scratch_dir = paths.artifact_root / "scratch"
    scratch_dir.mkdir(parents=True, exist_ok=True)
    scratch_input_path = scratch_dir / reduction_input.source_path.name

    def evaluator(candidate_text: str) -> dict[str, Any]:
        _atomic_write_text(scratch_input_path, candidate_text)
        default_manifest = build_candidate_manifest(candidate_text, scratch_input_path)
        env_updates = {
            "MOLT_DEBUG_EVAL_MODE": "reduce",
            "MOLT_DEBUG_EVAL_INPUT": str(scratch_input_path),
            "MOLT_DEBUG_EVAL_SOURCE_PATH": str(reduction_input.source_path),
            "MOLT_DEBUG_EVAL_ORACLE_JSON": json.dumps(oracle, sort_keys=True),
        }
        return _run_debug_eval_command(
            args.eval_command,
            cwd=Path.cwd(),
            env_updates=env_updates,
            default_manifest=default_manifest,
            timeout_sec=args.eval_timeout,
        )

    try:
        result = reduce_source_text(
            reduction_input,
            oracle=oracle,
            evaluator=evaluator,
        )
    except ValueError as exc:
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message=str(exc),
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )

    reduction_payload = build_reduction_payload(
        result, artifact_root=paths.artifact_root
    )
    reduced_source_path = Path(reduction_payload["artifacts"]["reduced_source"])
    _atomic_write_text(reduced_source_path, result.reduced_source + "\n")
    payload = normalize_debug_payload(
        subcommand=subcommand,
        status=DebugStatus.OK,
        run_id=paths.run_id,
        artifact_root=paths.artifact_root,
        manifest_path=paths.manifest_path,
        selectors=selectors,
        retained_output=paths.retained_output,
        data=reduction_payload,
    )
    return _emit_debug_payload(
        payload=payload,
        format_name=args.format,
        retained_output=paths.retained_output,
    )


def _handle_debug_bisect(
    args: argparse.Namespace,
    *,
    subcommand: DebugSubcommand,
    paths: Any,
    selectors: dict[str, Any],
) -> int:
    input_path = Path(args.input_path)
    if not input_path.is_file():
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message=f"{input_path} is not a file",
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )
    try:
        oracle = _load_debug_oracle(args)
    except (ValueError, json.JSONDecodeError) as exc:
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message=str(exc),
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )
    if args.eval_command is None:
        payload = normalize_debug_payload(
            subcommand=subcommand,
            status=DebugStatus.ERROR,
            run_id=paths.run_id,
            artifact_root=paths.artifact_root,
            manifest_path=paths.manifest_path,
            selectors=selectors,
            failure_class=DebugFailureClass.INVALID_REQUEST,
            message="--eval-command is required for bisect runs",
            retained_output=paths.retained_output,
        )
        return _emit_debug_payload(
            payload=payload,
            format_name=args.format,
            retained_output=paths.retained_output,
        )

    reduction_input = load_reduction_input(input_path)
    default_input_payload = {
        "kind": reduction_input.input_kind,
        "source_path": str(reduction_input.source_path),
        "manifest_path": (
            str(reduction_input.manifest_path)
            if reduction_input.manifest_path is not None
            else None
        ),
    }

    if args.passes:
        passes = tuple(_split_tokens(args.passes))
        if not passes:
            message = "--passes must include at least one pass name"
            payload = normalize_debug_payload(
                subcommand=subcommand,
                status=DebugStatus.ERROR,
                run_id=paths.run_id,
                artifact_root=paths.artifact_root,
                manifest_path=paths.manifest_path,
                selectors=selectors,
                failure_class=DebugFailureClass.INVALID_REQUEST,
                message=message,
                retained_output=paths.retained_output,
            )
            return _emit_debug_payload(
                payload=payload,
                format_name=args.format,
                retained_output=paths.retained_output,
            )

        def evaluator(prefix: tuple[str, ...]) -> dict[str, Any]:
            env_updates = {
                "MOLT_DEBUG_EVAL_MODE": "bisect-pass",
                "MOLT_DEBUG_EVAL_INPUT": str(reduction_input.source_path),
                "MOLT_DEBUG_EVAL_SOURCE_PATH": str(reduction_input.source_path),
                "MOLT_DEBUG_EVAL_PASSES_CSV": ",".join(prefix),
                "MOLT_DEBUG_EVAL_PASSES_JSON": json.dumps(list(prefix), sort_keys=True),
                "MOLT_DEBUG_EVAL_ORACLE_JSON": json.dumps(oracle, sort_keys=True),
            }
            default_manifest = {
                "candidate": {
                    "source_path": str(reduction_input.source_path),
                    "passes": list(prefix),
                }
            }
            return _run_debug_eval_command(
                args.eval_command,
                cwd=Path.cwd(),
                env_updates=env_updates,
                default_manifest=default_manifest,
                timeout_sec=args.eval_timeout,
            )

        bisect_payload = bisect_first_bad_pass(
            passes,
            oracle=oracle,
            evaluator=evaluator,
        )
    else:
        if not args.baseline_json or not args.failing_json:
            message = "use --passes or both --baseline-json and --failing-json"
            payload = normalize_debug_payload(
                subcommand=subcommand,
                status=DebugStatus.ERROR,
                run_id=paths.run_id,
                artifact_root=paths.artifact_root,
                manifest_path=paths.manifest_path,
                selectors=selectors,
                failure_class=DebugFailureClass.INVALID_REQUEST,
                message=message,
                retained_output=paths.retained_output,
            )
            return _emit_debug_payload(
                payload=payload,
                format_name=args.format,
                retained_output=paths.retained_output,
            )
        try:
            baseline = json.loads(args.baseline_json)
            failing = json.loads(args.failing_json)
        except json.JSONDecodeError as exc:
            payload = normalize_debug_payload(
                subcommand=subcommand,
                status=DebugStatus.ERROR,
                run_id=paths.run_id,
                artifact_root=paths.artifact_root,
                manifest_path=paths.manifest_path,
                selectors=selectors,
                failure_class=DebugFailureClass.INVALID_REQUEST,
                message=f"invalid bisect config JSON: {exc}",
                retained_output=paths.retained_output,
            )
            return _emit_debug_payload(
                payload=payload,
                format_name=args.format,
                retained_output=paths.retained_output,
            )

        def evaluator(candidate: dict[str, Any]) -> dict[str, Any]:
            env_updates = {
                "MOLT_DEBUG_EVAL_MODE": "bisect-config",
                "MOLT_DEBUG_EVAL_INPUT": str(reduction_input.source_path),
                "MOLT_DEBUG_EVAL_SOURCE_PATH": str(reduction_input.source_path),
                "MOLT_DEBUG_EVAL_CANDIDATE_JSON": json.dumps(candidate, sort_keys=True),
                "MOLT_DEBUG_EVAL_ORACLE_JSON": json.dumps(oracle, sort_keys=True),
            }
            default_manifest = {
                "candidate": candidate,
                "source_path": str(reduction_input.source_path),
            }
            return _run_debug_eval_command(
                args.eval_command,
                cwd=Path.cwd(),
                env_updates=env_updates,
                default_manifest=default_manifest,
                timeout_sec=args.eval_timeout,
            )

        bisect_payload = bisect_backend_profile_ic(
            baseline=baseline,
            failing=failing,
            oracle=oracle,
            evaluator=evaluator,
        )

    payload = normalize_debug_payload(
        subcommand=subcommand,
        status=DebugStatus.OK,
        run_id=paths.run_id,
        artifact_root=paths.artifact_root,
        manifest_path=paths.manifest_path,
        selectors=selectors,
        retained_output=paths.retained_output,
        data={
            "source_path": str(reduction_input.source_path),
            "source_text": reduction_input.source_text,
            "input": default_input_payload,
            "oracle": oracle,
            "bisect": bisect_payload,
        },
    )
    return _emit_debug_payload(
        payload=payload,
        format_name=args.format,
        retained_output=paths.retained_output,
    )


def _handle_debug_command(args: argparse.Namespace) -> int:
    subcommand = DebugSubcommand(args.debug_subcommand)
    paths = allocate_debug_paths(
        subcommand,
        out=args.out,
        output_extension=args.format,
    )
    selectors = {
        key: value
        for key, value in {
            "function": args.function,
            "module": args.module,
            "pass": args.pass_name,
            "backend": args.backend,
            "profile": args.profile,
            "stage": getattr(args, "stage", None),
        }.items()
        if value is not None
    }
    if subcommand == DebugSubcommand.IR:
        return _handle_debug_ir(
            args, subcommand=subcommand, paths=paths, selectors=selectors
        )
    if subcommand == DebugSubcommand.REPRO:
        return _handle_debug_repro(
            args, subcommand=subcommand, paths=paths, selectors=selectors
        )
    if subcommand == DebugSubcommand.VERIFY:
        return _handle_debug_verify(
            args, subcommand=subcommand, paths=paths, selectors=selectors
        )
    if subcommand == DebugSubcommand.TRACE:
        return _handle_debug_trace(
            args, subcommand=subcommand, paths=paths, selectors=selectors
        )
    if subcommand == DebugSubcommand.DIFF:
        return _handle_debug_diff(
            args, subcommand=subcommand, paths=paths, selectors=selectors
        )
    if subcommand == DebugSubcommand.PERF:
        return _handle_debug_perf(
            args, subcommand=subcommand, paths=paths, selectors=selectors
        )
    if subcommand == DebugSubcommand.REDUCE:
        return _handle_debug_reduce(
            args, subcommand=subcommand, paths=paths, selectors=selectors
        )
    if subcommand == DebugSubcommand.BISECT:
        return _handle_debug_bisect(
            args, subcommand=subcommand, paths=paths, selectors=selectors
        )
    pending_data: dict[str, Any] | None = None
    if hasattr(args, "source"):
        pending_data = {"source": str(Path(args.source))}
    elif hasattr(args, "input_path"):
        pending_data = {"input_path": str(Path(args.input_path))}
    payload = normalize_debug_payload(
        subcommand=subcommand,
        status=DebugStatus.UNSUPPORTED,
        run_id=paths.run_id,
        artifact_root=paths.artifact_root,
        manifest_path=paths.manifest_path,
        selectors=selectors,
        failure_class=DebugFailureClass.NOT_YET_WIRED,
        message=f"molt debug {subcommand.value} is not yet wired",
        retained_output=paths.retained_output,
        data=pending_data,
    )
    return _emit_debug_payload(
        payload=payload,
        format_name=args.format,
        retained_output=paths.retained_output,
    )
