from __future__ import annotations

import contextlib
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

from molt.type_facts import collect_type_facts_from_paths, write_type_facts
from molt.type_facts import TypeFacts

from molt.cli.atomic_io import _atomic_write_json
from molt.cli.command_runtime import _run_completed_command
from molt.cli.default_paths import _default_molt_cache
from molt.cli.file_hashing import _sha256_file
from molt.cli.lockfiles import _check_lockfiles
from molt.cli.models import TypeHintPolicy
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.project_roots import _find_project_root

_TY_CHECK_TIMEOUT_ENV = "MOLT_TY_TIMEOUT"
_DEFAULT_TY_CHECK_TIMEOUT = 30.0
_TYPE_FACTS_CACHE_SCHEMA_VERSION = 1


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
    commands = [
        ["uv", "run", "ty", "check", str(path), "--output-format", "concise"],
        ["ty", "check", str(path), "--output-format", "concise"],
    ]
    timeout = _ty_check_timeout()
    for cmd in commands:
        try:
            result = _run_completed_command(
                cmd,
                capture_output=True,
                env=None,
                cwd=None,
                memory_guard_prefix="MOLT_CLI",
                timeout=timeout,
            )
        except FileNotFoundError:
            continue
        except subprocess.TimeoutExpired:
            return (
                False,
                f"ty check timed out after {timeout:.1f}s; "
                "continuing with guarded hints only.",
            )
        if result.returncode == 0:
            return True, result.stdout.strip()
        combined = (result.stdout + result.stderr).strip()
        return False, combined
    return False, "ty is not available; install it with `uv add ty`."


def _type_facts_cache_root() -> Path:
    return _default_molt_cache() / "type_facts"


def _type_facts_tooling_identity() -> dict[str, str]:
    typecheck_path = Path(__file__).resolve()
    molt_root = typecheck_path.parent.parent
    project_root = molt_root.parent.parent
    candidates = [
        typecheck_path,
        molt_root / "type_facts.py",
        project_root / "pyproject.toml",
        project_root / "uv.lock",
    ]
    identity: dict[str, str] = {}
    for path in candidates:
        with contextlib.suppress(OSError):
            identity[str(path)] = _sha256_file(path)
    return identity


def _type_facts_source_identity(paths: list[Path]) -> list[dict[str, Any]] | None:
    identity: list[dict[str, Any]] = []
    for path in sorted(paths, key=lambda item: os.fspath(item.resolve())):
        try:
            stat = path.stat()
            source_sha256 = _sha256_file(path)
        except OSError:
            return None
        identity.append(
            {
                "module": path.stem,
                "path": os.fspath(path.resolve()),
                "size": stat.st_size,
                "sha256": source_sha256,
            }
        )
    return identity


def _type_facts_cache_key(
    paths: list[Path],
    type_hint_policy: TypeHintPolicy,
    ty_target: Path,
) -> str | None:
    source_identity = _type_facts_source_identity(paths)
    if source_identity is None:
        return None
    payload = {
        "schema_version": _TYPE_FACTS_CACHE_SCHEMA_VERSION,
        "type_hint_policy": type_hint_policy,
        "ty_target": os.fspath(ty_target.resolve()),
        "sources": source_identity,
        "tooling": _type_facts_tooling_identity(),
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )
    return hashlib.sha256(encoded).hexdigest()


def _type_facts_cache_path(cache_key: str) -> Path:
    return _type_facts_cache_root() / cache_key[:2] / f"{cache_key}.json"


def _read_cached_type_facts(cache_key: str | None) -> tuple[TypeFacts, bool] | None:
    if cache_key is None:
        return None
    try:
        payload = json.loads(
            _type_facts_cache_path(cache_key).read_text(encoding="utf-8")
        )
    except (OSError, json.JSONDecodeError):
        return None
    if (
        not isinstance(payload, dict)
        or payload.get("schema_version") != _TYPE_FACTS_CACHE_SCHEMA_VERSION
        or payload.get("cache_key") != cache_key
        or payload.get("ty_ok") is not True
    ):
        return None
    facts_payload = payload.get("facts")
    if not isinstance(facts_payload, dict):
        return None
    return TypeFacts.from_dict(facts_payload), True


def _write_cached_type_facts(cache_key: str | None, facts: TypeFacts) -> None:
    if cache_key is None:
        return
    payload = {
        "schema_version": _TYPE_FACTS_CACHE_SCHEMA_VERSION,
        "cache_key": cache_key,
        "ty_ok": True,
        "facts": facts.to_dict(),
    }
    with contextlib.suppress(OSError):
        _atomic_write_json(
            _type_facts_cache_path(cache_key),
            payload,
            indent=2,
            sort_keys=True,
        )


def _collect_type_facts_for_build(
    paths: list[Path], type_hint_policy: TypeHintPolicy, ty_target: Path
) -> tuple[Any | None, bool]:
    cache_key = _type_facts_cache_key(paths, type_hint_policy, ty_target)
    cached = _read_cached_type_facts(cache_key)
    if cached is not None:
        return cached
    trust = "trusted" if type_hint_policy == "trust" else "guarded"
    ty_ok, _ = _run_ty_check(ty_target)
    facts = collect_type_facts_from_paths(paths, trust, infer=ty_ok)
    if ty_ok:
        facts.tool = "molt-check+ty+infer"
        _write_cached_type_facts(cache_key, facts)
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
