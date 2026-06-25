from __future__ import annotations

import argparse
import copy
import importlib
import json
import os
from pathlib import Path
import shlex
import subprocess
from typing import Any, Mapping

from molt.debug import (
    render_debug_json_summary,
    render_debug_text_summary,
    write_debug_manifest,
)
from molt.debug.reduce import normalize_failure_oracle


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
