from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

from molt.type_facts import collect_type_facts_from_paths, write_type_facts

from molt.cli.command_runtime import _run_completed_command
from molt.cli.lockfiles import _check_lockfiles
from molt.cli.models import TypeHintPolicy
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import _find_project_root


def _collect_py_files(target: Path) -> list[Path]:
    if target.is_file():
        return [target]
    return sorted(path for path in target.rglob("*.py") if path.is_file())


def _run_ty_check(path: Path) -> tuple[bool, str]:
    commands = [
        ["uv", "run", "ty", "check", str(path), "--output-format", "concise"],
        ["ty", "check", str(path), "--output-format", "concise"],
    ]
    for cmd in commands:
        try:
            result = _run_completed_command(
                cmd,
                capture_output=True,
                env=None,
                cwd=None,
                memory_guard_prefix="MOLT_CLI",
            )
        except FileNotFoundError:
            continue
        if result.returncode == 0:
            return True, result.stdout.strip()
        combined = (result.stdout + result.stderr).strip()
        return False, combined
    return False, "ty is not available; install it with `uv add ty`."


def _collect_type_facts_for_build(
    paths: list[Path], type_hint_policy: TypeHintPolicy, ty_target: Path
) -> tuple[Any | None, bool]:
    trust = "trusted" if type_hint_policy == "trust" else "guarded"
    ty_ok, _ = _run_ty_check(ty_target)
    facts = collect_type_facts_from_paths(paths, trust, infer=ty_ok)
    if ty_ok:
        facts.tool = "molt-check+ty+infer"
    return facts, ty_ok


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
    if ty_ok:
        facts = collect_type_facts_from_paths(files, trust, infer=True)
        facts.tool = "molt-check+ty+infer"
        if verbose and not json_output:
            print("ty check passed; trusting inferred hints.")
    elif ty_output:
        warnings.append(ty_output)
        if not json_output:
            print(ty_output, file=sys.stderr)
        if strict:
            return _fail(
                "ty check failed; refusing strict type facts.",
                json_output,
                command="check",
            )
        facts = collect_type_facts_from_paths(files, trust, infer=False)
    else:
        facts = collect_type_facts_from_paths(files, trust, infer=False)
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
