"""Command-derived Python interpreter custody for proof execution receipts."""

from __future__ import annotations

import json
import platform
import re
import subprocess
import sys
from pathlib import Path
from typing import Mapping, Sequence

from tools import proof_plan

AUTHORITY_SCHEMA = "molt.proof-python-interpreter-authority.v1"
_PYTHON_COMMAND = re.compile(r"^python(?:\d+(?:\.\d+)*)?(?:\.exe)?$", re.IGNORECASE)
_PY_LAUNCHER = frozenset({"py", "py.exe"})
_PY_LAUNCHER_VERSION = re.compile(r"^-\d+(?:\.\d+)?$")
_PROBE_SCRIPT = (
    "import json,platform,sys;"
    "print(json.dumps({"
    "'executable':sys.executable,"
    "'implementation':platform.python_implementation(),"
    "'version':platform.python_version()"
    "},sort_keys=True))"
)


def _command_basename(value: str) -> str:
    return value.replace("\\", "/").rsplit("/", 1)[-1].casefold()


def _option_value(command: Sequence[str], name: str) -> str | None:
    prefix = f"{name}="
    for index, value in enumerate(command):
        if value == name and index + 1 < len(command):
            return str(command[index + 1])
        if value.startswith(prefix):
            return value[len(prefix) :]
    return None


def authority_for_command(command: Sequence[str]) -> dict[str, object]:
    """Serialize the proof-side interpreter choice without resolving it yet."""
    argv = [str(value) for value in command]
    if len(argv) >= 2 and _command_basename(argv[0]) in {"uv", "uv.exe"}:
        if argv[1] == "run":
            return {
                "schema": AUTHORITY_SCHEMA,
                "kind": "uv-project",
                "launcher": argv[0],
                "project": _option_value(argv, "--project"),
                "python_request": _option_value(argv, "--python"),
                "active": "--active" in argv,
                "no_sync": "--no-sync" in argv,
            }
    if argv and _PYTHON_COMMAND.fullmatch(_command_basename(argv[0])):
        return {
            "schema": AUTHORITY_SCHEMA,
            "kind": "direct",
            "executable": argv[0],
        }
    if argv and _command_basename(argv[0]) in _PY_LAUNCHER:
        version_request = next(
            (value for value in argv[1:] if _PY_LAUNCHER_VERSION.fullmatch(value)),
            None,
        )
        return {
            "schema": AUTHORITY_SCHEMA,
            "kind": "py-launcher",
            "launcher": argv[0],
            "python_request": version_request,
        }
    return {
        "schema": AUTHORITY_SCHEMA,
        "kind": "project-default",
        "launcher": "uv",
        "project": ".",
        "python_request": "3.12",
        "active": True,
        "no_sync": True,
    }


def _probe_command(authority: Mapping[str, object]) -> list[str]:
    if authority.get("schema") != AUTHORITY_SCHEMA:
        raise ValueError("proof Python interpreter authority schema mismatch")
    kind = authority.get("kind")
    if kind == "direct":
        executable = authority.get("executable")
        if not isinstance(executable, str) or not executable:
            raise ValueError("direct proof Python authority has no executable")
        return [executable, "-c", _PROBE_SCRIPT]
    if kind == "py-launcher":
        launcher = authority.get("launcher")
        version_request = authority.get("python_request")
        if not isinstance(launcher, str) or not launcher:
            raise ValueError("py launcher proof Python authority has no executable")
        command = [launcher]
        if isinstance(version_request, str) and version_request:
            command.append(version_request)
        command.extend(["-c", _PROBE_SCRIPT])
        return command
    if kind not in {"uv-project", "project-default"}:
        raise ValueError(f"unknown proof Python interpreter authority kind {kind!r}")
    launcher = authority.get("launcher")
    project = authority.get("project")
    python_request = authority.get("python_request")
    if not all(
        isinstance(value, str) and value
        for value in (launcher, project, python_request)
    ):
        raise ValueError("uv proof Python authority is incomplete")
    command = [str(launcher), "run"]
    if authority.get("active") is True:
        command.append("--active")
    command.extend(["--project", str(project), "--python", str(python_request)])
    if authority.get("no_sync") is True:
        command.append("--no-sync")
    command.extend(["python", "-c", _PROBE_SCRIPT])
    return command


def resolve_interpreter(
    authority: Mapping[str, object],
    *,
    cwd: Path,
    env: Mapping[str, str],
) -> dict[str, str]:
    """Resolve the commanded/project interpreter on the execution host."""
    command = _probe_command(authority)
    completed = subprocess.run(
        command,
        cwd=cwd,
        env=dict(env),
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=30,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise ValueError(
            "proof Python interpreter resolution failed "
            f"with exit code {completed.returncode}: {detail}"
        )
    try:
        payload = json.loads(completed.stdout.strip())
    except json.JSONDecodeError as exc:
        raise ValueError(
            "proof Python interpreter probe returned invalid JSON"
        ) from exc
    if not isinstance(payload, dict) or not all(
        isinstance(payload.get(name), str) and payload[name]
        for name in ("executable", "implementation", "version")
    ):
        raise ValueError("proof Python interpreter probe returned an invalid identity")
    return {
        name: str(payload[name]) for name in ("executable", "implementation", "version")
    }


def requested_toolchains(command: Sequence[str]) -> tuple[str, ...]:
    names = ["python"]
    if any(_command_basename(str(part)) in {"cargo", "cargo.exe"} for part in command):
        names.extend(("cargo", "rustc"))
    return tuple(names)


def receipt_context(
    authority: Mapping[str, object],
    *,
    command: Sequence[str],
    cwd: Path,
    env: Mapping[str, str],
) -> dict[str, object]:
    """Capture both queue-control and proof-command Python identities once."""
    proof_python = resolve_interpreter(authority, cwd=cwd, env=env)
    plan = proof_plan.ProofPlan.load()
    toolchains = proof_plan.toolchain_fingerprints(
        plan,
        requested_toolchains(command),
        executable_overrides={"python": proof_python["executable"]},
    )
    python_fingerprint = toolchains["python"]
    match = re.search(r"\b(\d+\.\d+)\.", python_fingerprint["version"])
    if match is None:
        raise ValueError(
            "proof Python toolchain fingerprint has no major.minor version"
        )
    return {
        "schema": plan.receipt_schema,
        "authority_sha256": proof_plan._authority_sha256(plan),
        "source_commit": proof_plan._source_commit(),
        "source_tree_state": proof_plan._source_tree_state(),
        "environment": {
            "os": proof_plan._normalized_os(),
            "arch": proof_plan._normalized_arch(),
            "python": match.group(1),
        },
        "toolchains": toolchains,
        "python_interpreters": {
            "queue_control_plane": {
                "executable": sys.executable,
                "implementation": platform.python_implementation(),
                "version": platform.python_version(),
                "role": "queue-runner-and-memory-guard",
            },
            "proof_command": {
                **proof_python,
                "authority": dict(authority),
                "role": "proof-command-envelope",
            },
        },
    }
