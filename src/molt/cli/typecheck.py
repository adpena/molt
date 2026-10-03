from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from molt.type_facts import collect_type_facts_from_paths, write_type_facts

from molt.cli.command_runtime import _run_completed_command
from molt.cli.lockfiles import _check_lockfiles
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import _find_project_root

_TY_CHECK_TIMEOUT_ENV = "MOLT_TY_TIMEOUT"
_DEFAULT_TY_CHECK_TIMEOUT = 30.0


def _collect_py_files(target: Path) -> list[Path]:
    if target.is_file():
        return [target]
    return sorted(path for path in target.rglob("*.py") if path.is_file())


def _ty_check_timeout() -> float:
    raw = os.environ.get(_TY_CHECK_TIMEOUT_ENV)
    if raw is None:
        return _DEFAULT_TY_CHECK_TIMEOUT
    try:
        timeout = float(raw)
    except ValueError:
        return _DEFAULT_TY_CHECK_TIMEOUT
    if timeout <= 0:
        return _DEFAULT_TY_CHECK_TIMEOUT
    return timeout


def _run_ty_check(path: Path) -> tuple[bool, str]:
    """Validate only when explicitly requested, using Molt's installed dependency.

    Isolated Python excludes a caller's cwd/PYTHONPATH from module resolution.
    ty still reads the target project's configuration and dependency environment;
    validation is consequently never cached as a compiler proof.
    """
    target = path.resolve()
    cmd = [
        sys.executable,
        "-I",
        "-m",
        "ty",
        "check",
        str(target),
        "--output-format",
        "concise",
    ]
    timeout = _ty_check_timeout()
    try:
        result = _run_completed_command(
            cmd,
            capture_output=True,
            env=None,
            cwd=_find_project_root(target),
            memory_guard_prefix="MOLT_CLI",
            timeout=timeout,
        )
    except OSError as exc:
        return False, f"Unable to run Molt's installed ty dependency: {exc}"
    except subprocess.TimeoutExpired:
        return (
            False,
            f"ty check timed out after {timeout:.1f}s; validation did not complete.",
        )
    output = (result.stdout + result.stderr).strip()
    if result.returncode == 0:
        return True, output
    return False, output or f"ty check failed with exit status {result.returncode}."


def check(
    path: str,
    output: str,
    strict: bool,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
) -> int:
    target = Path(path)
    if not target.exists():
        return _fail(f"Path not found: {target}", json_output, command="check")
    project_root = _find_project_root(target.resolve())
    warnings: list[str] = []
    lock_error = _check_lockfiles(
        project_root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "check",
    )
    if lock_error is not None:
        return lock_error
    files = _collect_py_files(target)
    if not files:
        return _fail(
            f"No Python files found under: {target}",
            json_output,
            command="check",
        )
    trust = "trusted" if strict else "guarded"
    ty_ok, ty_output = _run_ty_check(target)
    if not ty_ok:
        warnings.append(ty_output)
        if not json_output:
            print(ty_output, file=sys.stderr)
        if strict:
            return _fail(
                "ty check failed; refusing strict type facts.",
                json_output,
                command="check",
            )
    elif verbose and not json_output:
        print("ty check passed; source annotations validated.")
    # ty validates source; it does not expose flow-sensitive inference. A name's
    # first/last literal assignment cannot establish its type for the function.
    facts = collect_type_facts_from_paths(files, trust)
    output_path = Path(output)
    write_type_facts(output_path, facts)
    if json_output:
        payload = _json_payload(
            "check",
            "ok",
            data={
                "output": str(output_path),
                "strict": strict,
                "ty_ok": ty_ok,
                "deterministic": deterministic,
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output)
    else:
        print(f"Wrote type facts to {output_path}")
    return 0
