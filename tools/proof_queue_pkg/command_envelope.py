"""Typed proof-command custody from admission through guarded execution.

The queue persists exactly one envelope derived from the submitted argv.  The
same envelope is validated by the guarded child that fingerprints the selected
toolchains and launches the command.  No ambient interpreter is invented for a
non-Python command and no identity subprocess runs outside memory-guard custody.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Mapping, Sequence

_SOURCE_ROOT = Path(__file__).resolve().parents[2]
if str(_SOURCE_ROOT) not in sys.path:
    sys.path.insert(0, str(_SOURCE_ROOT))

from molt.cargo_execution_policy import (  # noqa: E402
    CARGO_WRAPPER_ENV_NAMES,
    normalize_cargo_environment,
)
from tools import proof_plan  # noqa: E402

ENVELOPE_SCHEMA = "molt.proof-command-envelope.v2"
EXECUTION_SCHEMA = "molt.proof-command-execution.v1"

_PYTHON_COMMAND = re.compile(r"^python(?:\d+(?:\.\d+)*)?(?:\.exe)?$", re.IGNORECASE)
_PY_LAUNCHERS = frozenset({"py", "py.exe"})
_PY_SELECTOR = re.compile(
    r"(?:-\d+(?:\.\d+)?(?:-(?:32|64))?|-V:[^\s/:]+(?:/[^\s/:]+)?)",
    re.IGNORECASE,
)
_SHELL_LAUNCHERS = frozenset(
    {
        "bash", "bash.exe", "cmd", "cmd.exe", "fish", "nu", "nu.exe",
        "powershell", "powershell.exe", "pwsh", "pwsh.exe", "sh", "sh.exe",
        "zsh", "zsh.exe",
    }
)
_PYTHON_CONSOLE_SCRIPTS = frozenset({"pytest", "pytest.exe", "py.test", "py.test.exe"})
_UV_FLAGS = frozenset(
    {
        "--active", "--all-extras", "--exact", "--frozen", "--inexact",
        "--isolated", "--locked", "--no-config", "--no-default-groups",
        "--no-dev", "--no-project", "--no-sync", "--offline",
    }
)
_UV_VALUE_OPTIONS = frozenset(
    {
        "--default-index", "--directory", "--env-file", "--extra", "--find-links",
        "--group", "--index", "--only-group", "--project", "--python", "--with",
        "--with-editable", "--with-requirements", "-p",
    }
)
_PROBE_SCRIPT = r'''
import hashlib
import importlib.metadata as metadata
import json
import platform
import re
import sys

executable = sys.executable
with open(executable, "rb") as handle:
    executable_sha256 = hashlib.file_digest(handle, "sha256").hexdigest()
distributions = []
for distribution in metadata.distributions():
    name = re.sub(r"[-_.]+", "-", distribution.metadata.get("Name", "")).lower()
    files = sorted(
        (
            str(item).replace("\\", "/"),
            str(item.hash) if item.hash is not None else None,
            item.size,
        )
        for item in (distribution.files or ())
    )
    file_manifest = json.dumps(files, separators=(",", ":"), sort_keys=True)
    record = distribution.read_text("RECORD") or ""
    direct_url = distribution.read_text("direct_url.json") or ""
    installer = distribution.read_text("INSTALLER") or ""
    distributions.append(
        {
            "name": name,
            "version": distribution.version,
            "record_sha256": hashlib.sha256(record.encode()).hexdigest(),
            "file_manifest_sha256": hashlib.sha256(file_manifest.encode()).hexdigest(),
            "direct_url_sha256": hashlib.sha256(direct_url.encode()).hexdigest(),
            "installer_sha256": hashlib.sha256(installer.encode()).hexdigest(),
        }
    )
distributions.sort(key=lambda item: (item["name"], item["version"], item["file_manifest_sha256"]))
inventory = json.dumps(distributions, separators=(",", ":"), sort_keys=True)
print(
    json.dumps(
        {
            "executable": executable,
            "implementation": platform.python_implementation(),
            "version": platform.python_version(),
            "executable_sha256": executable_sha256,
            "distributions": distributions,
            "distribution_inventory_sha256": hashlib.sha256(inventory.encode()).hexdigest(),
        },
        sort_keys=True,
    )
)
'''
_WHICH_SCRIPT = (
    "import json,pathlib,shutil,sys;"
    "v=sys.argv[1];c=pathlib.Path(v);"
    "p=str(c.resolve()) if (c.is_absolute() or c.parent != pathlib.Path('.')) and c.exists() else shutil.which(v);"
    "print(json.dumps({'path':p},sort_keys=True))"
)


def _basename(value: str) -> str:
    return value.replace("\\", "/").rsplit("/", 1)[-1].casefold()


def _uv_prefix_and_payload(argv: Sequence[str]) -> tuple[list[str], list[str]]:
    if len(argv) < 3 or argv[1] != "run":
        raise ValueError("proof queue only models `uv run` execution envelopes")
    index = 2
    while index < len(argv):
        value = argv[index]
        if value == "--":
            index += 1
            break
        option = value.split("=", 1)[0]
        if option in _UV_FLAGS:
            if "=" in value:
                raise ValueError(f"uv flag {option!r} does not accept a value")
            index += 1
            continue
        if option in _UV_VALUE_OPTIONS:
            if "=" in value:
                if not value.split("=", 1)[1]:
                    raise ValueError(f"uv option {option!r} has an empty value")
                index += 1
            else:
                if index + 1 >= len(argv) or not argv[index + 1]:
                    raise ValueError(f"uv option {option!r} needs a value")
                index += 2
            continue
        if value.startswith("-"):
            raise ValueError(
                f"unmodeled uv run option {value!r}; executable proof custody "
                "requires an exact, typed launch prefix"
            )
        break
    payload = [str(value) for value in argv[index:]]
    if not payload:
        raise ValueError("uv run proof envelope has no payload command")
    return [str(value) for value in argv[:index]], payload


def _uv_option_values(prefix: Sequence[str], name: str) -> list[str]:
    values: list[str] = []
    index = 2
    while index < len(prefix):
        value = str(prefix[index])
        option = value.split("=", 1)[0]
        if option == name:
            if "=" in value:
                values.append(value.split("=", 1)[1])
                index += 1
            else:
                values.append(str(prefix[index + 1]))
                index += 2
            continue
        index += 2 if option in _UV_VALUE_OPTIONS and "=" not in value else 1
    return values


def _path_inside(root: Path, raw: str, *, base: Path, label: str) -> Path:
    candidate = Path(raw)
    resolved = (candidate if candidate.is_absolute() else base / candidate).resolve(strict=True)
    try:
        resolved.relative_to(root.resolve())
    except ValueError as exc:
        raise ValueError(f"{label} {raw!r} escapes admitted source root {root}") from exc
    return resolved


def _execution_source_paths(
    envelope: Mapping[str, object], *, cwd: Path
) -> tuple[Path, list[Path]]:
    python = envelope.get("python")
    if not isinstance(python, Mapping) or python.get("kind") not in {"uv", "uv-console-script"}:
        return cwd.resolve(strict=True), []
    prefix = python.get("prefix")
    if not isinstance(prefix, list):
        raise ValueError("uv command envelope has no prefix")
    directories = _uv_option_values(prefix, "--directory")
    if len(directories) > 1:
        raise ValueError("uv command envelope has multiple --directory authorities")
    effective = (
        _path_inside(cwd, directories[0], base=cwd, label="uv --directory")
        if directories
        else cwd.resolve(strict=True)
    )
    projects = _uv_option_values(prefix, "--project")
    if len(projects) > 1:
        raise ValueError("uv command envelope has multiple --project authorities")
    if projects:
        project = _path_inside(cwd, projects[0], base=effective, label="uv --project")
        if project != effective:
            raise ValueError(
                "uv --project must equal the effective command cwd so one source "
                "snapshot owns every consumed project input"
            )
    overlay_inputs = [
        _path_inside(cwd, raw, base=effective, label="uv --with-requirements")
        for raw in _uv_option_values(prefix, "--with-requirements")
    ]
    return effective, overlay_inputs


def _nested_command(argv: Sequence[str]) -> list[str] | None:
    """Return a command explicitly delegated by Molt's guarded_exec authority."""
    if not argv:
        return None
    python_index = 0
    if _basename(argv[0]) in {"uv", "uv.exe"}:
        _prefix, payload = _uv_prefix_and_payload(argv)
    else:
        payload = [str(value) for value in argv]
    if not payload:
        return None
    first = _basename(payload[0])
    if _PYTHON_COMMAND.fullmatch(first) or first in _PY_LAUNCHERS:
        python_index = 1
        if first in _PY_LAUNCHERS and len(payload) > 1 and _PY_SELECTOR.fullmatch(payload[1]):
            python_index = 2
    else:
        return None
    if python_index >= len(payload):
        return None
    script = payload[python_index].replace("\\", "/")
    if script != "tools/guarded_exec.py":
        return None
    try:
        separator = payload.index("--", python_index + 1)
    except ValueError:
        return None
    nested = payload[separator + 1 :]
    return nested or None


def _guarded_exec_indices(argv: Sequence[str]) -> tuple[int, int] | None:
    if not argv:
        return None
    if _basename(argv[0]) in {"uv", "uv.exe"}:
        prefix, payload = _uv_prefix_and_payload(argv)
        offset = len(prefix)
    else:
        payload = [str(value) for value in argv]
        offset = 0
    first = _basename(payload[0])
    python_index = 1
    if first in _PY_LAUNCHERS and len(payload) > 1 and _PY_SELECTOR.fullmatch(payload[1]):
        python_index = 2
    if python_index >= len(payload) or payload[python_index].replace("\\", "/") != "tools/guarded_exec.py":
        return None
    separator = payload.index("--", python_index + 1)
    return offset + python_index, offset + separator + 1


def _requested_toolchains(
    argv: Sequence[str], *, has_python: bool, has_uv: bool
) -> list[str]:
    names: list[str] = ["python"] if has_python else []
    if has_uv:
        names.append("uv")
    if argv and _basename(argv[0]) in {"cargo", "cargo.exe"}:
        names.extend(("cargo", "rustc"))
    elif argv and _basename(argv[0]) in {"rustc", "rustc.exe"}:
        names.append("rustc")
    return names


def envelope_for_command(command: Sequence[str]) -> dict[str, object]:
    """Derive and validate the sole executable/toolchain authority for ``command``."""
    argv = [str(value) for value in command]
    if not argv or not argv[0]:
        raise ValueError("proof command must have a non-empty executable")
    first = _basename(argv[0])
    if first in _SHELL_LAUNCHERS:
        raise ValueError(
            "opaque shell wrappers are not executable proof evidence; submit a "
            "typed argv command or a declared queue command family"
        )

    python: dict[str, object] | None = None
    if first in {"uv", "uv.exe"}:
        prefix, payload = _uv_prefix_and_payload(argv)
        payload_first = _basename(payload[0])
        if _PYTHON_COMMAND.fullmatch(payload_first):
            python = {"kind": "uv", "prefix": prefix, "payload_executable": payload[0]}
        elif payload_first in _PYTHON_CONSOLE_SCRIPTS:
            python = {
                "kind": "uv-console-script",
                "prefix": prefix,
                "console_script": payload[0],
            }
        elif payload_first in _SHELL_LAUNCHERS:
            raise ValueError("opaque shell payloads under uv are not executable proof evidence")
        elif payload_first in {"cargo", "cargo.exe", "rustc", "rustc.exe"}:
            raise ValueError(
                "direct Rust payloads under uv bypass canonical queue custody; use "
                "the queue Cargo command family or a direct typed rustc argv"
            )
        else:
            raise ValueError(
                f"uv payload {payload[0]!r} may be an interpreter-bound console "
                "script; invoke it as `python -m ...` or declare a typed command family"
            )
    elif _PYTHON_COMMAND.fullmatch(first):
        python = {"kind": "direct", "executable": argv[0]}
    elif first in _PY_LAUNCHERS:
        selector: str | None = None
        if len(argv) > 1 and (argv[1].startswith("-") or argv[1].startswith("/")):
            if not _PY_SELECTOR.fullmatch(argv[1]):
                raise ValueError(f"unsupported Windows py selector {argv[1]!r}")
            selector = argv[1]
        python = {"kind": "py-launcher", "launcher": argv[0], "selector": selector}
    elif first in _PYTHON_CONSOLE_SCRIPTS:
        raise ValueError(
            "raw Python console scripts do not identify an interpreter; use "
            "`python -m pytest` or an exact `uv run ... pytest` envelope"
        )

    toolchains = _requested_toolchains(
        argv,
        has_python=python is not None,
        has_uv=first in {"uv", "uv.exe"},
    )
    nested_command = _nested_command(argv)
    delegated = (
        envelope_for_command(nested_command) if nested_command is not None else None
    )
    if delegated is not None:
        if delegated.get("kind") == "python":
            raise ValueError(
                "guarded_exec may not delegate another Python authority; invoke the "
                "final Python command directly"
            )
        if delegated.get("delegated") is not None:
            raise ValueError("nested guarded_exec delegation is limited to one typed layer")
        for name in delegated["toolchains"]:  # type: ignore[union-attr]
            if name not in toolchains:
                toolchains.append(str(name))
    return {
        "schema": ENVELOPE_SCHEMA,
        "kind": "python" if python is not None else "none",
        "argv": argv,
        "python": python,
        "toolchains": toolchains,
        "delegated": delegated,
    }


def admission_envelope(command: Sequence[str]) -> dict[str, object]:
    """Persist rejected argv without fabricating any executable authority."""
    try:
        return envelope_for_command(command)
    except ValueError as exc:
        return {
            "schema": ENVELOPE_SCHEMA,
            "kind": "rejected",
            "argv": [str(value) for value in command],
            "python": None,
            "toolchains": [],
            "delegated": None,
            "error": str(exc),
        }


def validate_envelope(envelope: Mapping[str, object], command: Sequence[str]) -> None:
    expected = envelope_for_command(command)
    if dict(envelope) != expected:
        raise ValueError("persisted proof command envelope does not match submitted argv")


def _hash_file(path: Path) -> str:
    try:
        with path.open("rb") as handle:
            return hashlib.file_digest(handle, "sha256").hexdigest()
    except OSError as exc:
        return f"unavailable:{type(exc).__name__}"


def _executable_identity(path: Path) -> dict[str, object]:
    try:
        size = path.stat().st_size
    except OSError as exc:
        size = -1
        digest = f"unavailable:{type(exc).__name__}"
    else:
        digest = _hash_file(path)
    identity: dict[str, object] = {
        "path": str(path),
        "size_bytes": size,
        "sha256": digest,
    }
    identity["identity_sha256"] = hashlib.sha256(
        json.dumps(identity, sort_keys=True).encode()
    ).hexdigest()
    return identity


def _content_identity_available(identity: Mapping[str, object]) -> bool:
    digest = identity.get("sha256")
    return (
        isinstance(identity.get("size_bytes"), int)
        and int(identity["size_bytes"]) >= 0
        and isinstance(digest, str)
        and re.fullmatch(r"[0-9a-f]{64}", digest) is not None
    )


def _run_captured(command: Sequence[str], *, cwd: Path, env: Mapping[str, str], timeout: float = 30.0) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(command), cwd=cwd, env=dict(env), check=False,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=timeout,
    )


def _resolve_outer_executable(token: str, *, cwd: Path, env: Mapping[str, str]) -> Path:
    candidate = Path(token)
    if candidate.is_absolute() or candidate.parent != Path("."):
        path = candidate if candidate.is_absolute() else cwd / candidate
        try:
            return path.resolve(strict=True)
        except OSError as exc:
            raise ValueError(f"proof executable {token!r} is unavailable") from exc
    found = shutil.which(token, path=env.get("PATH"))
    if found is None:
        raise ValueError(f"proof executable {token!r} is not on the execution PATH")
    return Path(found).resolve(strict=True)


def _exact_command(envelope: Mapping[str, object], *, cwd: Path, env: Mapping[str, str]) -> list[str]:
    argv = [str(value) for value in envelope["argv"]]  # type: ignore[index]
    argv[0] = str(_resolve_outer_executable(argv[0], cwd=cwd, env=env))
    return argv


def _bind_delegated_command(
    envelope: Mapping[str, object],
    exact: list[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
) -> tuple[dict[str, object] | None, dict[str, object] | None]:
    indices = _guarded_exec_indices(exact)
    delegated = envelope.get("delegated")
    if indices is None:
        if delegated is not None:
            raise ValueError("delegated envelope has no canonical guarded_exec launch")
        return None, None
    if not isinstance(delegated, Mapping):
        raise ValueError("canonical guarded_exec launch has no delegated envelope")
    script_index, delegated_index = indices
    guarded_exec_path = _path_inside(
        cwd,
        "tools/guarded_exec.py",
        base=cwd,
        label="canonical guarded_exec",
    )
    if not guarded_exec_path.is_file():
        raise ValueError("canonical guarded_exec authority is not a file")
    exact[script_index] = str(guarded_exec_path)
    delegated_path = _which_in_command_environment(
        exact[delegated_index], envelope, exact, cwd=cwd, env=env
    )
    exact[delegated_index] = str(delegated_path)
    return _file_identity(guarded_exec_path), _executable_identity(delegated_path)


def _python_probe_command(envelope: Mapping[str, object], exact: Sequence[str]) -> list[str] | None:
    python = envelope.get("python")
    if not isinstance(python, Mapping):
        return None
    kind = python.get("kind")
    if kind == "direct":
        return [exact[0], "-c", _PROBE_SCRIPT]
    if kind == "py-launcher":
        command = [exact[0]]
        selector = python.get("selector")
        if isinstance(selector, str) and selector:
            command.append(selector)
        return [*command, "-c", _PROBE_SCRIPT]
    if kind in {"uv", "uv-console-script"}:
        prefix = python.get("prefix")
        if not isinstance(prefix, list) or len(prefix) < 2:
            raise ValueError("uv proof envelope has no exact prefix")
        return [exact[0], *[str(value) for value in prefix[1:]], "python", "-c", _PROBE_SCRIPT]
    raise ValueError(f"unknown proof Python envelope kind {kind!r}")


def _parse_json_output(completed: subprocess.CompletedProcess[str], *, purpose: str) -> dict[str, object]:
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise ValueError(f"{purpose} failed with exit code {completed.returncode}: {detail}")
    try:
        payload = json.loads(completed.stdout.strip())
    except json.JSONDecodeError as exc:
        raise ValueError(f"{purpose} returned invalid JSON") from exc
    if not isinstance(payload, dict):
        raise ValueError(f"{purpose} returned a non-object identity")
    return payload


def _python_identity(envelope: Mapping[str, object], exact: Sequence[str], *, cwd: Path, env: Mapping[str, str]) -> dict[str, object] | None:
    command = _python_probe_command(envelope, exact)
    if command is None:
        return None
    payload = _parse_json_output(
        _run_captured(command, cwd=cwd, env=env), purpose="proof Python identity probe"
    )
    required = (
        "executable",
        "implementation",
        "version",
        "executable_sha256",
        "distribution_inventory_sha256",
    )
    if not all(isinstance(payload.get(name), str) and payload[name] for name in required):
        raise ValueError("proof Python identity probe returned incomplete identity")
    distributions = payload.get("distributions")
    if not isinstance(distributions, list):
        raise ValueError("proof Python identity has no distribution inventory")
    identity: dict[str, object] = {name: str(payload[name]) for name in required}
    identity["distributions"] = distributions
    identity["identity_sha256"] = hashlib.sha256(
        json.dumps(identity, sort_keys=True).encode()
    ).hexdigest()
    return identity


def _file_identity(path: Path) -> dict[str, object]:
    return {
        "path": str(path),
        "size_bytes": path.stat().st_size,
        "sha256": _hash_file(path),
    }


def _in_python_environment(envelope: Mapping[str, object], exact: Sequence[str], payload: Sequence[str]) -> list[str]:
    python = envelope.get("python")
    if isinstance(python, Mapping) and python.get("kind") in {"uv", "uv-console-script"}:
        prefix = python.get("prefix")
        assert isinstance(prefix, list)
        return [exact[0], *[str(value) for value in prefix[1:]], *payload]
    return list(payload)


def _which_in_command_environment(name: str, envelope: Mapping[str, object], exact: Sequence[str], *, cwd: Path, env: Mapping[str, str]) -> Path:
    python = envelope.get("python")
    if isinstance(python, Mapping) and python.get("kind") in {"uv", "uv-console-script"}:
        command = _in_python_environment(
            envelope, exact, ("python", "-c", _WHICH_SCRIPT, name)
        )
        payload = _parse_json_output(
            _run_captured(command, cwd=cwd, env=env), purpose=f"{name} path resolution"
        )
        found = payload.get("path")
        if not isinstance(found, str) or not found:
            raise ValueError(f"{name} is not on the proof command PATH")
        return Path(found).resolve(strict=True)
    return _resolve_outer_executable(name, cwd=cwd, env=env)


def _tool_identity(name: str, requested: str, envelope: Mapping[str, object], exact: Sequence[str], *, cwd: Path, env: Mapping[str, str]) -> dict[str, str]:
    path = _which_in_command_environment(requested, envelope, exact, cwd=cwd, env=env)
    version_args = ("-vV",) if name == "rustc" else ("--version",)
    completed = _run_captured(
        _in_python_environment(envelope, exact, (str(path), *version_args)),
        cwd=cwd, env=env,
    )
    if completed.returncode != 0:
        raise ValueError(f"{name} version probe failed: {completed.stderr.strip()}")
    content_path = path
    try:
        rustup = _which_in_command_environment(
            "rustup", envelope, exact, cwd=cwd, env=env
        )
    except ValueError:
        rustup = None
    if rustup is not None:
        resolved = _run_captured(
            _in_python_environment(envelope, exact, (rustup, "which", name)),
            cwd=cwd, env=env,
        )
        if resolved.returncode == 0 and resolved.stdout.strip():
            candidate = Path(resolved.stdout.strip())
            if candidate.is_file():
                content_path = candidate.resolve()
    material = {
        "path": str(path),
        "launcher_sha256": _hash_file(path),
        "content_path": str(content_path),
        "executable_sha256": _hash_file(content_path),
        "version": (completed.stdout or completed.stderr).strip(),
    }
    material["identity_sha256"] = hashlib.sha256(
        json.dumps(material, sort_keys=True).encode()
    ).hexdigest()
    return material


def _validate_toolchain_identity(
    plan: proof_plan.ProofPlan,
    name: str,
    identity: Mapping[str, object],
) -> None:
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    try:
        policy = policies[name]
    except KeyError as exc:
        raise ValueError(f"proof plan has no {name!r} toolchain policy") from exc
    version = identity.get("version")
    if name == "python" and isinstance(version, str):
        version = f"Python {version}"
    pattern = str(policy.data["version_pattern"])
    if not isinstance(version, str) or re.search(pattern, version) is None:
        raise ValueError(
            f"{name} identity version {version!r} violates canonical policy {pattern!r}"
        )
    hash_values = [
        value
        for key, value in identity.items()
        if key in {"sha256", "launcher_sha256", "executable_sha256"}
    ]
    if not hash_values or any(
        not isinstance(value, str) or value.startswith("unavailable:")
        for value in hash_values
    ):
        raise ValueError(f"{name} identity has no available executable content hash")


def _execution_environment_authority(
    env: Mapping[str, str], *, applied_cargo_policies: Sequence[str]
) -> dict[str, object]:
    fixed = {
        "PATH",
        "RUSTUP_TOOLCHAIN",
        "RUSTUP_HOME",
        "RUSTC",
        "CARGO_HOME",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_INCREMENTAL",
        "PYTHONHOME",
        "PYTHONPATH",
        "VIRTUAL_ENV",
        "UV_PROJECT_ENVIRONMENT",
        "UV_PYTHON",
        "RUSTFLAGS",
        *CARGO_WRAPPER_ENV_NAMES,
    }
    names = sorted(
        name
        for name in env
        if name in fixed or name.startswith(("CARGO_BUILD_", "UV_"))
    )
    values: dict[str, object] = {}
    for name in names:
        normalized = str(env[name]).replace("\\", "/")
        if name == "PATH":
            values[name] = {
                "entry_count": len([part for part in env[name].split(os.pathsep) if part]),
                "value_sha256": hashlib.sha256(normalized.encode()).hexdigest(),
                "redacted": True,
            }
        elif name.startswith("UV_"):
            values[name] = {
                "value_sha256": hashlib.sha256(normalized.encode()).hexdigest(),
                "redacted": True,
            }
        else:
            values[name] = {
                "value": normalized,
                "value_sha256": hashlib.sha256(normalized.encode()).hexdigest(),
                "redacted": False,
            }
    payload: dict[str, object] = {
        "variables": values,
        "cargo_policies": list(applied_cargo_policies),
    }
    payload["identity_sha256"] = hashlib.sha256(
        json.dumps(payload, sort_keys=True).encode()
    ).hexdigest()
    return payload


def _git_snapshot(cwd: Path, env: Mapping[str, str]) -> dict[str, object]:
    head = _run_captured(("git", "rev-parse", "HEAD"), cwd=cwd, env=env)
    if head.returncode != 0 or not re.fullmatch(r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", head.stdout.strip()):
        return {"available": False, "clean": False, "commit": None, "status_sha256": None}
    root = _run_captured(("git", "rev-parse", "--show-toplevel"), cwd=cwd, env=env)
    if root.returncode != 0:
        return {"available": False, "clean": False, "commit": head.stdout.strip().lower(), "status_sha256": None}
    source_root = Path(root.stdout.strip()).resolve(strict=True)
    status = subprocess.run(
        ["git", "status", "--porcelain=v1", "-z", "--untracked-files=all", "--ignore-submodules=none"],
        cwd=cwd, env=dict(env), check=False, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    if status.returncode != 0:
        return {"available": False, "clean": False, "commit": head.stdout.strip().lower(), "status_sha256": None}
    return {
        "available": True,
        "root": str(source_root),
        "clean": not status.stdout,
        "commit": head.stdout.strip().lower(),
        "status_sha256": hashlib.sha256(status.stdout).hexdigest(),
    }


def _atomic_json(path: Path, payload: Mapping[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + f".{os.getpid()}.tmp")
    temporary.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def execute_guarded_request(request_path: Path) -> int:
    """Run identity, preflight, proof, and completion custody under one guard."""
    request = json.loads(request_path.read_text(encoding="utf-8"))
    if not isinstance(request, dict):
        raise ValueError("proof execution request must be an object")
    command = request.get("command")
    envelope = request.get("envelope")
    result_path = Path(str(request["result_path"]))
    cwd = Path(str(request["cwd"]))
    if not isinstance(command, list) or not isinstance(envelope, dict):
        raise ValueError("proof execution request has no typed command envelope")
    command = [str(value) for value in command]
    validate_envelope(envelope, command)
    result: dict[str, object] = {
        "schema": EXECUTION_SCHEMA,
        "envelope": envelope,
        "phase": "identity",
        "command_started": False,
    }
    try:
        execution_env = dict(os.environ)
        applied_cargo_policies: tuple[str, ...] = ()
        if "cargo" in envelope.get("toolchains", []):
            execution_env, applied_cargo_policies = normalize_cargo_environment(
                execution_env
            )
        exact = _exact_command(envelope, cwd=cwd, env=execution_env)
        effective_cwd, overlay_paths = _execution_source_paths(envelope, cwd=cwd)
        guarded_exec_pre, delegated_pre = _bind_delegated_command(
            envelope,
            exact,
            cwd=cwd,
            env=execution_env,
        )
        executable_pre = _executable_identity(Path(exact[0]))
        overlay_pre = [_file_identity(path) for path in overlay_paths]
        pre_identities = [executable_pre, *overlay_pre]
        if guarded_exec_pre is not None:
            pre_identities.append(guarded_exec_pre)
        if delegated_pre is not None:
            pre_identities.append(delegated_pre)
        if not all(_content_identity_available(identity) for identity in pre_identities):
            raise ValueError("proof command or overlay input has unavailable content identity")
        pre_source = _git_snapshot(effective_cwd, execution_env)
        proof_python = _python_identity(envelope, exact, cwd=cwd, env=execution_env)
        toolchains: dict[str, object] = {}
        if proof_python is not None:
            toolchains["python"] = proof_python
        if "uv" in envelope.get("toolchains", []):
            toolchains["uv"] = _tool_identity(
                "uv",
                exact[0],
                {"python": None},
                exact,
                cwd=cwd,
                env=execution_env,
            )
        nested = _nested_command(command)
        nested_first = _basename(nested[0]) if nested else ""
        if "cargo" in envelope.get("toolchains", []):
            cargo_request = nested[0] if nested_first in {"cargo", "cargo.exe"} else "cargo"
            toolchains["cargo"] = _tool_identity(
                "cargo", cargo_request, envelope, exact, cwd=cwd, env=execution_env
            )
            toolchains["rustc"] = _tool_identity(
                "rustc", execution_env.get("RUSTC", "rustc"), envelope, exact,
                cwd=cwd, env=execution_env,
            )
        elif "rustc" in envelope.get("toolchains", []):
            rustc_request = command[0] if _basename(command[0]) in {"rustc", "rustc.exe"} else execution_env.get("RUSTC", "rustc")
            toolchains["rustc"] = _tool_identity(
                "rustc", rustc_request, envelope, exact, cwd=cwd, env=execution_env
            )

        plan = proof_plan.ProofPlan.load()
        for name, identity in toolchains.items():
            assert isinstance(identity, Mapping)
            _validate_toolchain_identity(plan, name, identity)
        python_version = "none"
        if proof_python is not None:
            match = re.match(r"(\d+\.\d+)", str(proof_python["version"]))
            if match is None:
                raise ValueError("proof Python identity has no major.minor version")
            python_version = match.group(1)
        context: dict[str, object] = {
            "schema": plan.receipt_schema,
            "authority_sha256": proof_plan._authority_sha256(plan),
            "source_commit": pre_source.get("commit"),
            "source_tree_state": "clean" if pre_source.get("clean") else "dirty",
            "environment": {
                "os": proof_plan._normalized_os(),
                "arch": proof_plan._normalized_arch(),
                "python": python_version,
            },
            "toolchains": toolchains,
            "command_envelope": envelope,
            "command_executable": {"prelaunch": executable_pre},
            "guarded_exec": (
                {"prelaunch": guarded_exec_pre}
                if guarded_exec_pre is not None
                else None
            ),
            "delegated_command_executable": (
                {"prelaunch": delegated_pre}
                if delegated_pre is not None
                else None
            ),
            "execution_environment": _execution_environment_authority(
                execution_env,
                applied_cargo_policies=applied_cargo_policies,
            ),
            "python_interpreters": {
                "queue_control_plane": {
                    "executable": sys.executable,
                    "implementation": platform.python_implementation(),
                    "version": platform.python_version(),
                    "role": "queue-runner-and-memory-guard",
                },
                "proof_command": (
                    {**proof_python, "role": "proof-command-envelope"}
                    if proof_python is not None
                    else {"kind": "none", "role": "proof-command-envelope"}
                ),
            },
            "source_custody": {
                "row_cwd": str(cwd.resolve(strict=True)),
                "effective_cwd": str(effective_cwd),
                "prelaunch": pre_source,
                "overlay_inputs": {"prelaunch": overlay_pre},
            },
        }
        result.update({"phase": "command", "receipt_context": context, "exact_command": exact})
        _atomic_json(result_path, result)
        # Toolchain provisioning/contract checks are descendants of the same
        # queue guard and therefore appear in its resource and timeout summary.
        from tools.proof_queue_pkg import policy

        preflight = policy._ensure_run_toolchain_preflight(
            repo_root=cwd, resource_family=str(request["resource_family"])
        )
        if preflight:
            raise ValueError("toolchain preflight failed: " + "; ".join(preflight))
        result["command_started"] = True
        completed = subprocess.run(exact, cwd=cwd, env=execution_env, check=False)
        result["command_returncode"] = int(completed.returncode)
        post_source = _git_snapshot(effective_cwd, execution_env)
        overlay_post = [_file_identity(path) for path in overlay_paths]
        executable_post = _executable_identity(Path(exact[0]))
        guarded_exec_post = (
            _file_identity(Path(str(guarded_exec_pre["path"])))
            if guarded_exec_pre is not None
            else None
        )
        delegated_post = (
            _executable_identity(Path(str(delegated_pre["path"])))
            if delegated_pre is not None
            else None
        )
        source_identical = pre_source == post_source
        executable_identical = executable_pre == executable_post
        guarded_exec_identical = guarded_exec_pre == guarded_exec_post
        delegated_identical = delegated_pre == delegated_post
        ineligible_reasons: list[str] = []
        if not pre_source.get("available") or not post_source.get("available"):
            ineligible_reasons.append("source-unavailable")
        if not pre_source.get("clean"):
            ineligible_reasons.append("source-dirty-prelaunch")
        if not post_source.get("clean"):
            ineligible_reasons.append("source-dirty-postcompletion")
        if not source_identical:
            ineligible_reasons.append("source-snapshot-changed")
        if not executable_identical:
            ineligible_reasons.append("command-executable-changed")
        if not _content_identity_available(executable_post):
            ineligible_reasons.append("command-executable-unavailable-postcompletion")
        if not guarded_exec_identical:
            ineligible_reasons.append("guarded-exec-changed")
        if not delegated_identical:
            ineligible_reasons.append("delegated-command-executable-changed")
        if overlay_pre != overlay_post:
            ineligible_reasons.append("overlay-input-changed")
        if not all(_content_identity_available(identity) for identity in overlay_post):
            ineligible_reasons.append("overlay-input-unavailable-postcompletion")
        eligible = not ineligible_reasons
        source_custody = context["source_custody"]
        assert isinstance(source_custody, dict)
        source_custody.update(
            {
                "postcompletion": post_source,
                "identical": source_identical,
                "evidence_eligible": eligible,
                "ineligible_reasons": ineligible_reasons,
            }
        )
        overlay_inputs = source_custody["overlay_inputs"]
        assert isinstance(overlay_inputs, dict)
        overlay_inputs.update(
            {
                "postcompletion": overlay_post,
                "identical": overlay_pre == overlay_post,
            }
        )
        command_executable = context["command_executable"]
        assert isinstance(command_executable, dict)
        command_executable.update(
            {
                "postcompletion": executable_post,
                "identical": executable_identical,
            }
        )
        if guarded_exec_pre is not None:
            guarded_exec = context["guarded_exec"]
            assert isinstance(guarded_exec, dict)
            guarded_exec.update(
                {"postcompletion": guarded_exec_post, "identical": guarded_exec_identical}
            )
        if delegated_pre is not None:
            delegated_executable = context["delegated_command_executable"]
            assert isinstance(delegated_executable, dict)
            delegated_executable.update(
                {"postcompletion": delegated_post, "identical": delegated_identical}
            )
        result["phase"] = "complete"
        _atomic_json(result_path, result)
        return int(completed.returncode)
    except BaseException as exc:
        result.update(
            {
                "phase": "failed",
                "error": f"{type(exc).__name__}: {exc}",
            }
        )
        _atomic_json(result_path, result)
        print(f"proof command envelope failed: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 2


def _main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", required=True)
    args = parser.parse_args(argv)
    return execute_guarded_request(Path(args.request))


if __name__ == "__main__":
    raise SystemExit(_main())
