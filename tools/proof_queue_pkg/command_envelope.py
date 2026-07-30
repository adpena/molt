"""Typed proof-command custody from admission through guarded execution.

The queue persists exactly one envelope derived from the submitted argv.  The
same envelope is validated by the guarded child that fingerprints the selected
toolchains and launches the command.  No ambient interpreter is invented for a
non-Python command and no identity subprocess runs outside memory-guard custody.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import functools
import hashlib
import hmac
import json
import math
import os
import platform
import re
import secrets
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Mapping, Sequence

_REPO_ROOT = Path(__file__).resolve().parents[2]
_PYTHON_SOURCE_ROOT = _REPO_ROOT / "src"
# The guarded child is launched by absolute script path from an arbitrary proof
# cwd. Bootstrap both import authorities explicitly: the repository root owns
# `tools`, while the src-layout root owns `molt`. Ambient editable-install or
# caller PYTHONPATH state must not decide whether custody can start.
for _import_root in (_REPO_ROOT, _PYTHON_SOURCE_ROOT):
    if str(_import_root) not in sys.path:
        sys.path.insert(0, str(_import_root))
# Editable installs may import `molt` from another canonical worktree during
# interpreter startup. Once a regular package is loaded, changing sys.path does
# not change its submodule search path, so bind the package itself to this
# envelope's source tree before importing custody authorities.
_loaded_molt = sys.modules.get("molt")
if _loaded_molt is not None and hasattr(_loaded_molt, "__path__"):
    _local_molt_root = str(_PYTHON_SOURCE_ROOT / "molt")
    if _local_molt_root not in _loaded_molt.__path__:
        _loaded_molt.__path__.insert(0, _local_molt_root)
_PYTHON_IDENTITY_PROBE = (
    _REPO_ROOT / "tools" / "proof_queue_pkg" / "python_identity_probe.py"
)

from molt.cargo_execution_policy import normalize_cargo_environment  # noqa: E402
from tools.command_execution import CommandExecutor  # noqa: E402
from tools import proof_plan  # noqa: E402
from tools.proof_queue_pkg import (  # noqa: E402
    custody_cas,
    execution_custody,
    process_image_capture,
    toolchain_capture,
)

ENVELOPE_SCHEMA = "molt.proof-command-envelope.v3"
EXECUTION_SCHEMA = "molt.proof-command-execution.v4"
_COMMANDS = CommandExecutor.for_file(__file__)

_PYTHON_COMMAND = re.compile(r"^python(?:\d+(?:\.\d+)*)?(?:\.exe)?$", re.IGNORECASE)
_PY_LAUNCHERS = frozenset({"py", "py.exe"})
_PY_SELECTOR = re.compile(
    r"(?:-\d+(?:\.\d+)?(?:-(?:32|64))?|-V:[^\s/:]+(?:/[^\s/:]+)?)",
    re.IGNORECASE,
)
_PYTHON_CUSTODY_BOOTSTRAP = Path(__file__).with_name("python_custody_bootstrap.py")
_PYTHON_TOOLCHAIN_LOCATOR = Path(__file__).with_name("python_toolchain_locator.py")


@dataclass(frozen=True)
class PythonInvocation:
    """Canonical CPython command-line split at the payload boundary.

    Interpreter options remain interpreter options when the payload is routed
    through the custody bootstrap.  Payload arguments are never reparsed, so
    values beginning with ``-`` retain their ordinary ``sys.argv`` meaning.
    """

    interpreter_options: tuple[str, ...]
    mode: str
    target: str | None
    arguments: tuple[str, ...]


_PYTHON_FLAG_CHARACTERS = frozenset("bBdEhiIOPqRsSuvVx?")
_PYTHON_TERMINAL_OPTIONS = frozenset(
    {
        "-h",
        "-?",
        "-V",
        "--help",
        "--help-all",
        "--help-env",
        "--help-xoptions",
        "--version",
    }
)


def parse_python_invocation(argv: Sequence[str]) -> PythonInvocation:
    """Parse CPython's interpreter options once for admission and execution."""
    if not argv:
        raise ValueError("Python invocation has no interpreter")
    values = [str(value) for value in argv]
    options: list[str] = []
    index = 1
    while index < len(values):
        value = values[index]
        if value == "--":
            index += 1
            break
        if value == "-":
            return PythonInvocation(tuple(options), "stdin", None, tuple(values[index + 1 :]))
        if value == "-c" or value.startswith("-c"):
            if value == "-c":
                if index + 1 >= len(values):
                    raise ValueError("Python -c requires a command")
                target = values[index + 1]
                arguments = values[index + 2 :]
            else:
                target = value[2:]
                arguments = values[index + 1 :]
            return PythonInvocation(tuple(options), "command", target, tuple(arguments))
        if value == "-m" or value.startswith("-m"):
            if value == "-m":
                if index + 1 >= len(values):
                    raise ValueError("Python -m requires a module")
                target = values[index + 1]
                arguments = values[index + 2 :]
            else:
                target = value[2:]
                arguments = values[index + 1 :]
            if not target:
                raise ValueError("Python -m requires a non-empty module")
            return PythonInvocation(tuple(options), "module", target, tuple(arguments))
        if not value.startswith("-") or value == "-":
            break
        if value in _PYTHON_TERMINAL_OPTIONS or (
            value.startswith("-")
            and not value.startswith("--")
            and value[1:]
            and set(value[1:]) <= _PYTHON_FLAG_CHARACTERS
            and any(character in "hV?" for character in value[1:])
        ):
            return PythonInvocation(tuple((*options, value)), "terminal", None, ())
        if value == "--check-hash-based-pycs":
            if index + 1 >= len(values):
                raise ValueError("Python --check-hash-based-pycs requires a value")
            option_value = values[index + 1]
            if option_value not in {"always", "default", "never"}:
                raise ValueError(
                    "Python --check-hash-based-pycs requires always, default, or never"
                )
            options.extend((value, option_value))
            index += 2
            continue
        if value in {"-W", "-X"}:
            if index + 1 >= len(values):
                raise ValueError(f"Python {value} requires a value")
            options.extend((value, values[index + 1]))
            index += 2
            continue
        if value.startswith(("-W", "-X")) and len(value) > 2:
            options.append(value)
            index += 1
            continue
        if (
            value.startswith("-")
            and not value.startswith("--")
            and value[1:]
            and set(value[1:]) <= _PYTHON_FLAG_CHARACTERS
        ):
            options.append(value)
            index += 1
            continue
        raise ValueError(f"unsupported Python interpreter option {value!r}")
    if index < len(values):
        return PythonInvocation(
            tuple(options), "script", values[index], tuple(values[index + 1 :])
        )
    return PythonInvocation(tuple(options), "stdin", None, ())
_SHELL_LAUNCHERS = frozenset(
    {
        "bash",
        "bash.exe",
        "cmd",
        "cmd.exe",
        "fish",
        "nu",
        "nu.exe",
        "powershell",
        "powershell.exe",
        "pwsh",
        "pwsh.exe",
        "sh",
        "sh.exe",
        "zsh",
        "zsh.exe",
    }
)
_PYTHON_CONSOLE_MODULES = {
    "pytest": "pytest",
    "pytest.exe": "pytest",
    "py.test": "pytest",
    "py.test.exe": "pytest",
    "pip-audit": "pip_audit",
    "pip-audit.exe": "pip_audit",
}
_PYTHON_CONSOLE_SCRIPTS = frozenset(_PYTHON_CONSOLE_MODULES)
# One closed authority for every admitted ``uv run`` option.  Input-bearing
# options are either assigned an immutable custody role or rejected here; no
# second parser is allowed to infer their semantics later.
_UV_OPTION_SEMANTICS: dict[str, tuple[str, str]] = {
    "--active": ("flag", "environment-selection"),
    "--all-extras": ("flag", "project-selection"),
    "--exact": ("flag", "environment-selection"),
    "--frozen": ("flag", "project-lock"),
    "--inexact": ("flag", "environment-selection"),
    "--isolated": ("flag", "environment-selection"),
    "--locked": ("flag", "project-lock"),
    "--no-config": ("flag", "environment-selection"),
    "--no-default-groups": ("flag", "project-selection"),
    "--no-dev": ("flag", "project-selection"),
    "--no-project": ("flag", "project-selection"),
    "--no-sync": ("flag", "environment-selection"),
    "--offline": ("flag", "network-denial"),
    "--directory": ("value", "source-directory"),
    "--extra": ("value", "project-selection"),
    "--group": ("value", "project-selection"),
    "--only-group": ("value", "project-selection"),
    "--project": ("value", "project-directory"),
    "--python": ("value", "python-selection"),
    "-p": ("value", "python-selection"),
    "--with-requirements": ("value", "requirements-file"),
    # These can inject source, configuration, or network state that is not
    # represented by the admitted project snapshot.  Reject them structurally
    # rather than growing exception-shaped partial custody.
    "--default-index": ("reject", "network-source"),
    "--env-file": ("reject", "environment-file"),
    "--find-links": ("reject", "package-source"),
    "--index": ("reject", "network-source"),
    "--with": ("reject", "package-overlay"),
    "--with-editable": ("reject", "editable-source"),
}
_UV_VALUE_OPTIONS = frozenset(
    option
    for option, (shape, _role) in _UV_OPTION_SEMANTICS.items()
    if shape in {"value", "reject"}
)
_WHICH_SCRIPT = (
    "import json,pathlib,shutil,sys;"
    "v=sys.argv[1];c=pathlib.Path(v);"
    "p=str(c.resolve()) if (c.is_absolute() or c.parent != pathlib.Path('.')) and c.exists() else shutil.which(v);"
    "print(json.dumps({'path':p},sort_keys=True))"
)


def _basename(value: str) -> str:
    return value.replace("\\", "/").rsplit("/", 1)[-1].casefold()


def _executable_registry_names(value: str) -> frozenset[str]:
    basename = _basename(value)
    suffixes = (".exe", ".cmd", ".bat", ".ps1")
    stem = next(
        (basename[: -len(suffix)] for suffix in suffixes if basename.endswith(suffix)),
        basename,
    )
    return frozenset({stem, *(stem + suffix for suffix in suffixes)})


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
        semantics = _UV_OPTION_SEMANTICS.get(option)
        if semantics is None:
            if value.startswith("-"):
                raise ValueError(
                    f"unmodeled uv run option {value!r}; executable proof custody "
                    "requires an exact, typed launch prefix"
                )
            break
        shape, role = semantics
        if shape == "reject":
            raise ValueError(
                f"uv option {option!r} is non-hermetic ({role}) and is not "
                "admitted by proof custody"
            )
        if shape == "flag":
            if "=" in value:
                raise ValueError(f"uv flag {option!r} does not accept a value")
            index += 1
            continue
        if shape == "value":
            if "=" in value:
                if not value.split("=", 1)[1]:
                    raise ValueError(f"uv option {option!r} has an empty value")
                index += 1
            else:
                if index + 1 >= len(argv) or not argv[index + 1]:
                    raise ValueError(f"uv option {option!r} needs a value")
                index += 2
            continue
        raise AssertionError(f"unknown uv option shape {shape!r}")
    payload = [str(value) for value in argv[index:]]
    if not payload:
        raise ValueError("uv run proof envelope has no payload command")
    return [str(value) for value in argv[:index]], payload


def _normalized_entrypoint_target(value: str) -> str:
    normalized = value.replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    candidate = Path(value)
    if candidate.is_absolute():
        try:
            normalized = (
                candidate.resolve(strict=False).relative_to(_REPO_ROOT).as_posix()
            )
        except ValueError:
            pass
    return normalized.casefold()


def _command_entrypoint(argv: Sequence[str]) -> tuple[str, str] | None:
    """Return the stable program entrypoint whose plan authority cannot drift.

    Arguments are deliberately excluded.  If an argv is close enough to execute
    the same Python/Node program as a proof-plan command, it must be an exact plan
    command; otherwise changing one selector could silently discard that
    command's declared toolchain closure.
    """
    if not argv:
        return None
    payload = [str(value) for value in argv]
    if _basename(payload[0]) in {"uv", "uv.exe"}:
        _prefix, payload = _uv_prefix_and_payload(payload)
    first = _basename(payload[0])
    python_index: int | None = None
    if _PYTHON_COMMAND.fullmatch(first):
        python_index = 1
    elif first in _PY_LAUNCHERS:
        python_index = (
            2 if len(payload) > 1 and _PY_SELECTOR.fullmatch(payload[1]) else 1
        )
    if python_index is not None:
        if python_index >= len(payload):
            return None
        target = payload[python_index]
        if target == "-m" and python_index + 1 < len(payload):
            module = payload[python_index + 1]
            if module == "tools.guarded_exec":
                return None
            return ("python-module", module.casefold())
        if target.startswith("-"):
            return None
        if _basename(target) == "guarded_exec.py":
            return None
        return ("python-script", _normalized_entrypoint_target(target))
    if first in {"node", "node.exe"} and len(payload) > 1:
        target = payload[1]
        if not target.startswith("-"):
            return ("node-script", _normalized_entrypoint_target(target))
    return None


@functools.lru_cache(maxsize=1)
def _proof_command_registry() -> dict[str, object]:
    """Project the proof plan into the one admitted executable/toolchain registry."""
    plan = proof_plan.ProofPlan.load()
    exact: dict[tuple[str, ...], dict[str, object]] = {}
    console_tools: dict[str, set[str]] = {}
    policy_executables: dict[str, str] = {}
    entrypoints: dict[tuple[str, str], list[str]] = {}
    entrypoint_variants: dict[tuple[str, str], set[tuple[str, ...]]] = {}
    for policy in plan.toolchain_policies:
        executable = str(policy.data.get("executable") or policy.name)
        if executable == "{python}":
            continue
        for basename in _executable_registry_names(executable):
            prior = policy_executables.get(basename)
            if prior is not None and prior != policy.name:
                raise ValueError(
                    f"proof plan executable {basename!r} has ambiguous toolchain policies"
                )
            policy_executables[basename] = policy.name
    for command in plan.commands:
        argv = tuple(str(value) for value in command.argv)
        declared = tuple(command.toolchains)
        existing = exact.get(argv)
        if existing is None:
            exact[argv] = {"ids": [command.id], "toolchains": declared}
        else:
            if existing["toolchains"] != declared:
                raise ValueError(
                    "identical proof-plan argv has conflicting toolchain authorities: "
                    f"{existing['ids']!r}, {command.id!r}"
                )
            ids = existing["ids"]
            assert isinstance(ids, list)
            ids.append(command.id)
        entrypoint = _command_entrypoint(argv)
        if entrypoint is not None:
            entrypoints.setdefault(entrypoint, []).append(command.id)
            entrypoint_variants.setdefault(entrypoint, set()).add(argv)
        if argv and _basename(argv[0]) in {"uv", "uv.exe"}:
            _prefix, payload = _uv_prefix_and_payload(argv)
            payload_name = _basename(payload[0])
            if not _PYTHON_COMMAND.fullmatch(payload_name):
                console_tools.setdefault(payload_name, set()).update(command.toolchains)
    return {
        "exact": exact,
        "console_tools": {
            name: tuple(sorted(toolchains))
            for name, toolchains in sorted(console_tools.items())
        },
        "policy_executables": policy_executables,
        "entrypoints": entrypoints,
        "entrypoint_variants": entrypoint_variants,
    }


def _registered_console_toolchains(name: str) -> tuple[str, ...] | None:
    registry = _proof_command_registry()
    console_tools = registry["console_tools"]
    assert isinstance(console_tools, dict)
    value = console_tools.get(name)
    return tuple(value) if isinstance(value, tuple) else None


def _command_registration(
    argv: Sequence[str], *, has_python: bool, has_uv: bool
) -> tuple[str, list[str], list[str]]:
    registry = _proof_command_registry()
    exact = registry["exact"]
    assert isinstance(exact, dict)
    exact_match = exact.get(tuple(str(value) for value in argv))
    if isinstance(exact_match, dict):
        command_ids = exact_match["ids"]
        declared = exact_match["toolchains"]
        assert isinstance(command_ids, list) and isinstance(declared, tuple)
        toolchains = [str(name) for name in declared]
        if not toolchains:
            raise ValueError(
                f"proof-plan commands {command_ids!r} have no toolchain authority"
            )
        return "proof-plan", toolchains, [str(command_id) for command_id in command_ids]

    entrypoint = _command_entrypoint(argv)
    registered_entrypoints = registry["entrypoints"]
    assert isinstance(registered_entrypoints, dict)
    near_matches = registered_entrypoints.get(entrypoint)
    registered_variants = registry["entrypoint_variants"]
    assert isinstance(registered_variants, dict)
    variants = registered_variants.get(entrypoint)
    if (
        isinstance(near_matches, list)
        and isinstance(variants, set)
        and len(variants) == 1
    ):
        raise ValueError(
            "proof-plan entrypoint argv must match its registered command exactly; "
            f"near-match would discard toolchain authority for {near_matches!r}"
        )

    toolchains: list[str] = []

    def add(name: str) -> None:
        if name not in toolchains:
            toolchains.append(name)

    if has_python:
        add("python")
        if has_uv:
            add("uv")
        if argv and _basename(argv[0]) in {"uv", "uv.exe"}:
            _prefix, payload = _uv_prefix_and_payload(argv)
            console = _registered_console_toolchains(_basename(payload[0]))
            if console is not None:
                for name in console:
                    add(name)
        return "python", toolchains, []

    if not argv:
        raise ValueError("proof command has no executable registration")
    first = _basename(argv[0])
    policy_executables = registry["policy_executables"]
    assert isinstance(policy_executables, dict)
    policy_name = policy_executables.get(first)
    if not isinstance(policy_name, str):
        raise ValueError(
            f"unknown proof executable kind {argv[0]!r}; add it to the proof-plan "
            "toolchain registry or invoke a registered typed command family"
        )
    add(policy_name)
    if policy_name == "cargo":
        add("rustc")
        if len(argv) > 1 and argv[1] == "deny":
            add("cargo-deny")
        elif len(argv) > 1 and argv[1] == "audit":
            add("cargo-audit")
    return "toolchain", toolchains, []


_CARGO_LEAF_SUBCOMMANDS = frozenset(
    {
        "help",
        "locate-project",
        "metadata",
        "pkgid",
        "search",
        "tree",
        "version",
        "--help",
        "-h",
        "--version",
        "-V",
    }
)
_TOOLCHAIN_LEAF_PROBES = frozenset({"--help", "-h", "--version", "-V", "-vV"})


def _registered_toolchain_descendants(argv: Sequence[str]) -> str:
    """Classify exact capability probes separately from process-spawning tools."""
    arguments = [str(value) for value in argv[1:]]
    if not arguments:
        return "forbidden"
    executable = _basename(str(argv[0]))
    if executable in {"cargo", "cargo.exe"}:
        command_index = 0
        if arguments[0].startswith("+"):
            command_index = 1
        if command_index >= len(arguments):
            return "forbidden"
        return (
            "forbidden"
            if arguments[command_index] in _CARGO_LEAF_SUBCOMMANDS
            else "declared-toolchains"
        )
    if arguments[0] in _TOOLCHAIN_LEAF_PROBES:
        return "forbidden"
    if executable in {"rustc", "rustc.exe"} and arguments[0] in {
        "--print",
        "--explain",
    }:
        return "forbidden"
    return "declared-toolchains"


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


def _uv_option_value_indices(prefix: Sequence[str], name: str) -> list[int]:
    """Return indices of values for ``name`` in the persisted uv prefix."""
    indices: list[int] = []
    index = 2
    while index < len(prefix):
        value = str(prefix[index])
        option = value.split("=", 1)[0]
        semantics = _UV_OPTION_SEMANTICS.get(option)
        if semantics is None:
            break
        shape, _role = semantics
        if option == name:
            if "=" in value:
                indices.append(index)
                index += 1
            else:
                indices.append(index + 1)
                index += 2
            continue
        index += 2 if shape == "value" and "=" not in value else 1
    return indices


def _path_inside(root: Path, raw: str, *, base: Path, label: str) -> Path:
    candidate = Path(raw)
    resolved = (candidate if candidate.is_absolute() else base / candidate).resolve(
        strict=True
    )
    try:
        resolved.relative_to(root.resolve())
    except ValueError as exc:
        raise ValueError(
            f"{label} {raw!r} escapes admitted source root {root}"
        ) from exc
    return resolved


_HASHED_REQUIREMENT = re.compile(
    r"^[A-Za-z0-9_.-]+(?:\[[A-Za-z0-9_.,-]+\])?=="
    r"(?:[0-9]+!)?[0-9]+(?:\.[0-9]+)*(?:(?:a|b|rc)[0-9]+)?"
    r"(?:\.post[0-9]+)?(?:\.dev[0-9]+)?"
    r"(?:\+[A-Za-z0-9]+(?:[._-][A-Za-z0-9]+)*)?"
    r"(?:\s+--hash=sha256:[0-9a-fA-F]{64})+$"
)


def _validate_requirements_file(path: Path) -> None:
    """Admit only offline, hash-locked package requirements."""
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise ValueError(f"requirements custody cannot read {path}") from exc
    logical: list[str] = []
    pending = ""
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        pending = f"{pending} {line}".strip()
        if pending.endswith("\\"):
            pending = pending[:-1].rstrip()
            continue
        logical.append(pending)
        pending = ""
    if pending:
        raise ValueError(f"requirements file {path} ends in a continuation")
    if not logical:
        raise ValueError(f"requirements file {path} has no locked requirements")
    for line in logical:
        if not _HASHED_REQUIREMENT.fullmatch(line):
            raise ValueError(
                "proof requirements must be exact name==version entries with one "
                f"or more sha256 hashes; rejected {line!r} in {path}"
            )


def _execution_source_paths(
    envelope: Mapping[str, object], *, cwd: Path
) -> tuple[Path, list[Path]]:
    python = envelope.get("python")
    if not isinstance(python, Mapping) or python.get("kind") not in {
        "uv",
        "uv-console-script",
    }:
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
    if overlay_inputs and "--offline" not in prefix:
        raise ValueError("uv --with-requirements proofs require --offline custody")
    for overlay in overlay_inputs:
        if not overlay.is_file():
            raise ValueError(f"requirements authority is not a file: {overlay}")
        _validate_requirements_file(overlay)
    return effective, overlay_inputs


def _require_external_execution_outputs(
    *, result_path: Path, effective_source: Path
) -> None:
    """Reject proof output authorities that overlap the consumed source tree."""
    if not result_path.is_absolute():
        raise ValueError("proof execution result path must be absolute")
    source = effective_source.resolve(strict=True)
    output_parent = result_path.parent.resolve(strict=True)
    if output_parent == source or output_parent.is_relative_to(source):
        raise ValueError("proof execution outputs must be outside effective source")
    cas_root = output_parent / "custody-cas"
    if cas_root.exists():
        resolved_cas = cas_root.resolve(strict=True)
        if resolved_cas == source or resolved_cas.is_relative_to(source):
            raise ValueError("proof custody CAS must be outside effective source")


def _canonical_uv_prefix(
    envelope: Mapping[str, object], *, cwd: Path
) -> tuple[list[str], Path, list[Path]]:
    python = envelope.get("python")
    if not isinstance(python, Mapping) or python.get("kind") not in {
        "uv",
        "uv-console-script",
    }:
        return [], cwd.resolve(strict=True), []
    prefix = python.get("prefix")
    if not isinstance(prefix, list):
        raise ValueError("uv command envelope has no prefix")
    exact_prefix = [str(value) for value in prefix]
    effective, overlays = _execution_source_paths(envelope, cwd=cwd)
    replacements = {
        "--directory": [effective] if _uv_option_values(prefix, "--directory") else [],
        "--project": [effective] if _uv_option_values(prefix, "--project") else [],
        "--with-requirements": overlays,
    }
    for option, paths in replacements.items():
        indices = _uv_option_value_indices(prefix, option)
        if len(indices) != len(paths):
            raise ValueError(f"uv {option} custody index mismatch")
        for index, path in zip(indices, paths, strict=True):
            original = exact_prefix[index]
            exact_prefix[index] = (
                f"{option}={path}" if original.startswith(f"{option}=") else str(path)
            )
    return exact_prefix, effective, overlays


def _guarded_exec_invocation(argv: Sequence[str]) -> dict[str, object] | None:
    """Parse every canonical spelling of the queue's guarded delegation seam."""
    if not argv:
        return None
    if _basename(argv[0]) in {"uv", "uv.exe"}:
        prefix, payload = _uv_prefix_and_payload(argv)
        offset = len(prefix)
    else:
        payload = [str(value) for value in argv]
        offset = 0
    if not payload:
        return None
    first = _basename(payload[0])
    python_index = 1
    if _PYTHON_COMMAND.fullmatch(first) or first in _PY_LAUNCHERS:
        if (
            first in _PY_LAUNCHERS
            and len(payload) > 1
            and _PY_SELECTOR.fullmatch(payload[1])
        ):
            python_index = 2
    else:
        if any("guarded_exec" in str(value).casefold() for value in payload):
            raise ValueError("guarded_exec delegation must be the direct Python target")
        return None
    if python_index >= len(payload):
        return None
    target = payload[python_index]
    mode: str | None = None
    target_indices: list[int] = []
    after_target = python_index + 1
    if target == "-m":
        if after_target >= len(payload):
            return None
        module = payload[after_target]
        if module == "tools.guarded_exec":
            mode = "module"
            target_indices = [offset + python_index, offset + after_target]
            after_target += 1
        elif "guarded_exec" in module.casefold():
            raise ValueError(f"ambiguous guarded_exec module authority {module!r}")
    elif _basename(target) == "guarded_exec.py":
        mode = "script"
        target_indices = [offset + python_index]
    if mode is None:
        if any(
            "guarded_exec" in str(value).casefold()
            for value in payload[python_index + 1 :]
        ):
            raise ValueError("guarded_exec delegation must be the direct Python target")
        return None
    try:
        separator = payload.index("--", after_target)
    except ValueError:
        raise ValueError("guarded_exec delegation requires an explicit `--` boundary")
    nested = payload[separator + 1 :]
    if not nested:
        raise ValueError("guarded_exec delegation has no delegated command")
    return {
        "mode": mode,
        "target_indices": target_indices,
        "delegated_index": offset + separator + 1,
        "nested": [str(value) for value in nested],
    }


def _nested_command(argv: Sequence[str]) -> list[str] | None:
    invocation = _guarded_exec_invocation(argv)
    if invocation is None:
        return None
    nested = invocation["nested"]
    assert isinstance(nested, list)
    return [str(value) for value in nested]


def _python_invocation_argv(
    argv: Sequence[str], python: Mapping[str, object]
) -> list[str]:
    """Return one interpreter-shaped argv for every admitted Python spelling."""
    kind = python.get("kind")
    if kind in {"uv", "uv-console-script"}:
        _prefix, payload = _uv_prefix_and_payload(argv)
        if kind == "uv-console-script":
            module = _PYTHON_CONSOLE_MODULES.get(_basename(str(python["console_script"])))
            if module is not None:
                return ["python", "-m", module, *payload[1:]]
        return [str(value) for value in payload]
    if kind == "py-launcher":
        selector_offset = 2 if python.get("selector") else 1
        return ["python", *[str(value) for value in argv[selector_offset:]]]
    if kind == "direct":
        return [str(value) for value in argv]
    raise ValueError(f"unknown proof Python envelope kind {kind!r}")


def _python_bootstrap_command(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    supervised_executable: str | None = None,
) -> list[str]:
    """Route Python payload execution through the absolute custody authority."""
    python = envelope.get("python")
    if not isinstance(python, Mapping):
        return [str(value) for value in exact]
    kind = python.get("kind")
    exact_values = [str(value) for value in exact]
    if kind in {"uv", "uv-console-script"}:
        prefix = python.get("prefix")
        if not isinstance(prefix, list):
            raise ValueError("uv proof envelope has no exact prefix")
        payload_index = len(prefix)
        invocation_argv = exact_values[payload_index:]
        outer = exact_values[: payload_index + 1]
    elif kind == "py-launcher":
        selector_offset = 2 if python.get("selector") else 1
        invocation_argv = [exact_values[0], *exact_values[selector_offset:]]
        outer = exact_values[:selector_offset]
    elif kind == "direct":
        invocation_argv = exact_values
        outer = exact_values[:1]
    else:
        raise ValueError(f"unknown proof Python envelope kind {kind!r}")
    invocation = parse_python_invocation(invocation_argv)
    if invocation.mode == "terminal":
        # Interpreter-owned terminal actions execute no user payload and cannot
        # launch descendants, so retain CPython's own early-exit semantics.
        if supervised_executable is not None:
            return [supervised_executable, *invocation.interpreter_options]
        return exact_values
    bootstrap = _PYTHON_CUSTODY_BOOTSTRAP.resolve(strict=True)
    skip_first_line = any(
        option.startswith("-")
        and not option.startswith("-X")
        and "x" in option[1:]
        for option in invocation.interpreter_options
    )
    arguments = list(invocation.arguments)
    if invocation.mode == "module" and invocation.target == "pytest":
        cache_disabled = any(
            argument == "-pno:cacheprovider"
            or (
                argument == "-p"
                and index + 1 < len(arguments)
                and arguments[index + 1] == "no:cacheprovider"
            )
            for index, argument in enumerate(arguments)
        )
        if not cache_disabled:
            arguments[:0] = ["-p", "no:cacheprovider"]
    payload = [str(bootstrap), invocation.mode, "1" if skip_first_line else "0"]
    if invocation.target is not None:
        payload.append(invocation.target)
    payload.extend(arguments)
    if supervised_executable is not None:
        return [supervised_executable, *invocation.interpreter_options, *payload]
    return [*outer, *invocation.interpreter_options, *payload]


def _supervised_execution_command(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    located_toolchains: Mapping[str, object],
) -> tuple[list[str], dict[str, str]]:
    """Collapse a Windows Python launcher into its captured runtime image.

    Windows virtual-environment and ``py``/``uv`` launchers create a second
    process before Python code can install custody.  A leaf proof therefore
    executes the selected interpreter's captured base image directly and gives
    CPython its standard launcher identity variable.  This preserves
    ``sys.executable`` and ordinary venv discovery while making the kernel root
    the process that actually executes user code.
    """
    if os.name != "nt" or not isinstance(envelope.get("python"), Mapping):
        return _python_bootstrap_command(envelope, exact), {}
    python = located_toolchains.get("python")
    if not isinstance(python, Mapping):
        raise ValueError("proof Python execution has no located runtime")
    selected_raw = python.get("executable")
    base_raw = python.get("base_executable")
    if not isinstance(selected_raw, str) or not isinstance(base_raw, str):
        raise ValueError("proof Python execution has no captured launcher chain")
    selected = Path(selected_raw).resolve(strict=True)
    base = Path(base_raw).resolve(strict=True)
    if os.path.normcase(str(selected)) == os.path.normcase(str(base)):
        return _python_bootstrap_command(
            envelope, exact, supervised_executable=str(base)
        ), {}
    return _python_bootstrap_command(
        envelope, exact, supervised_executable=str(base)
    ), {"__PYVENV_LAUNCHER__": str(selected)}


def _envelope_for_command(
    command: Sequence[str], *, typed_delegation: bool
) -> dict[str, object]:
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
            raise ValueError(
                "opaque shell payloads under uv are not executable proof evidence"
            )
        elif payload_first in {"cargo", "cargo.exe", "rustc", "rustc.exe"}:
            raise ValueError(
                "direct Rust payloads under uv bypass canonical queue custody; use "
                "the queue Cargo command family or a direct typed rustc argv"
            )
        elif _registered_console_toolchains(payload_first) is None:
            raise ValueError(
                f"uv payload {payload[0]!r} may be an interpreter-bound console "
                "script; invoke it as `python -m ...` or declare a typed command family"
            )
    elif _PYTHON_COMMAND.fullmatch(first):
        python = {"kind": "direct", "executable": argv[0]}
    elif first in _PY_LAUNCHERS:
        selector: str | None = None
        if len(argv) > 1 and _PY_SELECTOR.fullmatch(argv[1]):
            selector = argv[1]
        python = {"kind": "py-launcher", "launcher": argv[0], "selector": selector}
    elif first in _PYTHON_CONSOLE_SCRIPTS:
        raise ValueError(
            "raw Python console scripts do not identify an interpreter; use "
            "`python -m pytest` or an exact `uv run ... pytest` envelope"
        )

    if python is not None:
        parse_python_invocation(_python_invocation_argv(argv, python))

    registration_kind, toolchains, proof_plan_command_ids = _command_registration(
        argv,
        has_python=python is not None,
        has_uv=first in {"uv", "uv.exe"},
    )
    guarded_exec = _guarded_exec_invocation(argv)
    nested_command = (
        [str(value) for value in guarded_exec["nested"]]
        if guarded_exec is not None
        else None
    )
    delegated = (
        _envelope_for_command(nested_command, typed_delegation=True)
        if nested_command is not None
        else None
    )
    if delegated is not None:
        if delegated.get("python") is not None:
            raise ValueError(
                "guarded_exec may not delegate another Python authority; invoke the "
                "final Python command directly"
            )
        if (
            delegated.get("guarded_exec") is not None
            or delegated.get("delegated") is not None
        ):
            raise ValueError(
                "nested guarded_exec delegation is limited to one typed layer"
            )
        for name in delegated["toolchains"]:  # type: ignore[union-attr]
            if name not in toolchains:
                toolchains.append(str(name))
    if registration_kind == "proof-plan":
        process_closure = {
            "kind": "proof-plan",
            "descendants": "declared-toolchains",
            "toolchains": list(toolchains),
        }
    elif delegated is not None:
        process_closure = {
            "kind": "typed-delegation",
            "descendants": "declared-toolchains",
            "toolchains": list(toolchains),
        }
    elif registration_kind == "toolchain" and (
        descendants := _registered_toolchain_descendants(argv)
    ) == "declared-toolchains":
        process_closure = {
            "kind": "registered-toolchain",
            "descendants": descendants,
            "toolchains": list(toolchains),
        }
    else:
        process_closure = {
            "kind": "leaf",
            "descendants": "forbidden",
            "toolchains": list(toolchains),
        }
    return {
        "schema": ENVELOPE_SCHEMA,
        "kind": registration_kind,
        "argv": argv,
        "python": python,
        "toolchains": toolchains,
        "proof_plan_command_ids": proof_plan_command_ids,
        "guarded_exec": (
            {key: value for key, value in guarded_exec.items() if key != "nested"}
            if guarded_exec is not None
            else None
        ),
        "delegated": delegated,
        "process_closure": process_closure,
    }


def envelope_for_command(command: Sequence[str]) -> dict[str, object]:
    """Derive the sole executable, toolchain, and child-process authority."""
    return _envelope_for_command(command, typed_delegation=False)


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
            "proof_plan_command_ids": [],
            "guarded_exec": None,
            "delegated": None,
            "process_closure": None,
            "error": str(exc),
        }


def validate_envelope(envelope: Mapping[str, object], command: Sequence[str]) -> None:
    expected = envelope_for_command(command)
    if dict(envelope) != expected:
        raise ValueError(
            "persisted proof command envelope does not match submitted argv"
        )


def _hash_file(path: Path) -> str:
    try:
        with path.open("rb") as handle:
            return hashlib.file_digest(handle, "sha256").hexdigest()
    except OSError as exc:
        return f"unavailable:{type(exc).__name__}"


def _directory_manifest_identity(path: Path, *, label: str) -> dict[str, object]:
    root = path.resolve(strict=True)
    if not root.is_dir():
        raise ValueError(f"{label} is not a directory: {root}")
    files: list[dict[str, object]] = []
    for candidate in sorted(root.rglob("*"), key=lambda value: value.as_posix()):
        if candidate.is_symlink() and candidate.is_dir():
            raise ValueError(
                f"{label} contains an unowned directory symlink: {candidate}"
            )
        if not candidate.is_file():
            continue
        resolved = candidate.resolve(strict=True)
        try:
            resolved.relative_to(root)
        except ValueError as exc:
            raise ValueError(
                f"{label} file escapes its package root: {candidate} -> {resolved}"
            ) from exc
        size = resolved.stat().st_size
        digest = _hash_file(resolved)
        if re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise ValueError(f"{label} file has no content identity: {resolved}")
        files.append(
            {
                "relative_path": candidate.relative_to(root).as_posix(),
                "lexical_path": str(candidate.absolute()),
                "resolved_path": str(resolved),
                "symlinked": os.path.normcase(str(candidate.absolute()))
                != os.path.normcase(str(resolved)),
                "size": size,
                "sha256": digest,
            }
        )
    manifest = json.dumps(files, sort_keys=True, separators=(",", ":"))
    return {
        "root": str(root),
        "file_count": len(files),
        "files": files,
        "manifest_sha256": hashlib.sha256(manifest.encode()).hexdigest(),
    }


def _executable_identity(path: Path) -> dict[str, object]:
    lexical = Path(os.path.abspath(path))
    try:
        resolved = lexical.resolve(strict=True)
        size = lexical.stat().st_size
    except OSError as exc:
        resolved = lexical
        size = -1
        digest = f"unavailable:{type(exc).__name__}"
    else:
        digest = _hash_file(lexical)
    identity: dict[str, object] = {
        "path": str(lexical),
        "resolved_path": str(resolved),
        "symlinked": os.path.normcase(str(lexical)) != os.path.normcase(str(resolved)),
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


def _run_captured(
    command: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    timeout: float = 30.0,
    text: bool = True,
) -> subprocess.CompletedProcess[Any]:
    return _COMMANDS.run(
        list(command),
        cwd=cwd,
        env=dict(env),
        check=False,
        capture_output=True,
        text=text,
        timeout=timeout,
    )


def _resolve_outer_executable(token: str, *, cwd: Path, env: Mapping[str, str]) -> Path:
    candidate = Path(token)
    if candidate.is_absolute() or candidate.parent != Path("."):
        path = candidate if candidate.is_absolute() else cwd / candidate
        try:
            lexical = Path(os.path.abspath(path))
            if not lexical.is_file():
                raise FileNotFoundError(lexical)
            return lexical
        except OSError as exc:
            raise ValueError(f"proof executable {token!r} is unavailable") from exc
    found = shutil.which(token, path=env.get("PATH"))
    if found is None:
        raise ValueError(f"proof executable {token!r} is not on the execution PATH")
    lexical = Path(os.path.abspath(found))
    if not lexical.is_file():
        raise ValueError(f"proof executable {token!r} is unavailable")
    return lexical


def _exact_command(
    envelope: Mapping[str, object], *, cwd: Path, env: Mapping[str, str]
) -> list[str]:
    argv = [str(value) for value in envelope["argv"]]  # type: ignore[index]
    python = envelope.get("python")
    if isinstance(python, Mapping) and python.get("kind") in {
        "uv",
        "uv-console-script",
    }:
        prefix, _effective, _overlays = _canonical_uv_prefix(envelope, cwd=cwd)
        raw_prefix = python.get("prefix")
        assert isinstance(raw_prefix, list)
        argv = [*prefix, *argv[len(raw_prefix) :]]
    argv[0] = str(_resolve_outer_executable(argv[0], cwd=cwd, env=env))
    if isinstance(python, Mapping) and python.get("kind") == "uv-console-script":
        prefix = python.get("prefix")
        assert isinstance(prefix, list)
        console = _basename(str(python["console_script"]))
        payload_index = len(prefix)
        module = _PYTHON_CONSOLE_MODULES.get(console)
        if module is not None:
            argv = [
                *argv[:payload_index],
                "python",
                "-m",
                module,
                *argv[payload_index + 1 :],
            ]
        else:
            payload_path = _which_in_command_environment(
                argv[payload_index], envelope, argv, cwd=cwd, env=env
            )
            argv[payload_index] = str(payload_path)
    return argv


def _payload_executable_identity(
    envelope: Mapping[str, object], exact: Sequence[str]
) -> dict[str, object] | None:
    python = envelope.get("python")
    if not isinstance(python, Mapping) or python.get("kind") != "uv-console-script":
        return None
    console = _basename(str(python.get("console_script") or ""))
    if console in _PYTHON_CONSOLE_MODULES:
        return None
    prefix = python.get("prefix")
    if not isinstance(prefix, list):
        raise ValueError("uv console command has no exact prefix")
    return _executable_identity(Path(str(exact[len(prefix)])))


def _bind_delegated_command(
    envelope: Mapping[str, object],
    exact: list[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
) -> tuple[dict[str, object] | None, dict[str, object] | None]:
    invocation = envelope.get("guarded_exec")
    delegated = envelope.get("delegated")
    if invocation is None:
        if delegated is not None:
            raise ValueError("delegated envelope has no canonical guarded_exec launch")
        return None, None
    if not isinstance(invocation, Mapping):
        raise ValueError("guarded_exec invocation authority is malformed")
    if not isinstance(delegated, Mapping):
        raise ValueError("canonical guarded_exec launch has no delegated envelope")
    guarded_exec_path = _path_inside(
        cwd,
        "tools/guarded_exec.py",
        base=cwd,
        label="canonical guarded_exec",
    )
    if not guarded_exec_path.is_file():
        raise ValueError("canonical guarded_exec authority is not a file")
    target_indices = invocation.get("target_indices")
    delegated_index_raw = invocation.get("delegated_index")
    mode = invocation.get("mode")
    if (
        not isinstance(target_indices, list)
        or not all(isinstance(index, int) for index in target_indices)
        or not isinstance(delegated_index_raw, int)
    ):
        raise ValueError("guarded_exec invocation indices are malformed")
    delegated_index = delegated_index_raw
    if mode == "script" and len(target_indices) == 1:
        script_index = int(target_indices[0])
        submitted = Path(str(envelope["argv"][script_index]))  # type: ignore[index]
        if submitted.is_absolute():
            if submitted.resolve(strict=True) != guarded_exec_path:
                raise ValueError(
                    "absolute guarded_exec path is not the canonical source authority"
                )
        else:
            normalized = str(submitted).replace("\\", "/")
            while normalized.startswith("./"):
                normalized = normalized[2:]
            if normalized != "tools/guarded_exec.py":
                raise ValueError("relative guarded_exec path is not canonical")
        exact[script_index] = str(guarded_exec_path)
    elif mode == "module" and len(target_indices) == 2:
        module_flag, module_name = (int(index) for index in target_indices)
        if module_name != module_flag + 1:
            raise ValueError("guarded_exec module authority is not contiguous")
        exact[module_flag : module_name + 1] = [str(guarded_exec_path)]
        delegated_index -= 1
    else:
        raise ValueError("unknown guarded_exec invocation mode")
    delegated_path = _which_in_command_environment(
        exact[delegated_index], envelope, exact, cwd=cwd, env=env
    )
    exact[delegated_index] = str(delegated_path)
    return _file_identity(guarded_exec_path), _executable_identity(delegated_path)


def _python_auxiliary_command(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    authority: Path,
    arguments: Sequence[str],
) -> list[str] | None:
    python = envelope.get("python")
    if not isinstance(python, Mapping):
        return None
    kind = python.get("kind")
    if kind == "direct":
        return [exact[0], str(authority), *arguments]
    if kind == "py-launcher":
        command = [exact[0]]
        selector = python.get("selector")
        if isinstance(selector, str) and selector:
            command.append(selector)
        return [*command, str(authority), *arguments]
    if kind in {"uv", "uv-console-script"}:
        prefix = python.get("prefix")
        if not isinstance(prefix, list) or len(prefix) < 2:
            raise ValueError("uv proof envelope has no exact prefix")
        return [
            *exact[: len(prefix)],
            "python",
            str(authority),
            *arguments,
        ]
    raise ValueError(f"unknown proof Python envelope kind {kind!r}")


def _python_probe_command(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    source_root: Path,
    hash_workers: int = 1,
) -> list[str] | None:
    return _python_auxiliary_command(
        envelope,
        exact,
        authority=_PYTHON_IDENTITY_PROBE,
        arguments=(str(source_root), str(hash_workers)),
    )


def _parse_json_output(
    completed: subprocess.CompletedProcess[str], *, purpose: str
) -> dict[str, object]:
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise ValueError(
            f"{purpose} failed with exit code {completed.returncode}: {detail}"
        )
    try:
        payload = json.loads(completed.stdout.strip())
    except json.JSONDecodeError as exc:
        raise ValueError(f"{purpose} returned invalid JSON") from exc
    if not isinstance(payload, dict):
        raise ValueError(f"{purpose} returned a non-object identity")
    return payload


def _python_identity(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    source_root: Path,
    hash_workers: int,
) -> dict[str, object] | None:
    command = _python_probe_command(
        envelope,
        exact,
        source_root=source_root,
        hash_workers=hash_workers,
    )
    if command is None:
        return None
    payload = _parse_json_output(
        _run_captured(command, cwd=cwd, env=env, timeout=120.0),
        purpose="proof Python identity probe",
    )
    required = (
        "executable",
        "implementation",
        "version",
        "executable_sha256",
        "runtime_closure_sha256",
        "distribution_inventory_sha256",
    )
    if not all(
        isinstance(payload.get(name), str) and payload[name] for name in required
    ):
        raise ValueError("proof Python identity probe returned incomplete identity")
    distributions = payload.get("distributions")
    if not isinstance(distributions, list):
        raise ValueError("proof Python identity has no distribution inventory")
    runtime = payload.get("runtime")
    if not isinstance(runtime, dict):
        raise ValueError("proof Python identity has no CPython runtime closure")
    identity: dict[str, object] = {name: str(payload[name]) for name in required}
    identity["runtime"] = runtime
    identity["distributions"] = distributions
    inventory_profile = payload.get("inventory_profile")
    if not isinstance(inventory_profile, dict):
        raise ValueError("proof Python identity has no inventory profile")
    identity["identity_sha256"] = hashlib.sha256(
        json.dumps(identity, sort_keys=True).encode()
    ).hexdigest()
    identity["inventory_profile"] = inventory_profile
    return identity


def _file_identity(path: Path) -> dict[str, object]:
    return {
        "path": str(path),
        "size_bytes": path.stat().st_size,
        "sha256": _hash_file(path),
    }


_TEST_COUNT_PATTERN = re.compile(
    r"(?P<count>\d+)\s+(?P<kind>passed|failed|ignored|skipped|deselected|xfailed|xpassed)",
    re.IGNORECASE,
)


def _transcript_identity(path: Path) -> dict[str, object]:
    identity = _file_identity(path)
    counts: dict[str, int] = {}
    with path.open("r", encoding="utf-8", errors="replace") as handle:
        for line in handle:
            for match in _TEST_COUNT_PATTERN.finditer(line):
                kind = match.group("kind").casefold()
                counts[kind] = counts.get(kind, 0) + int(match.group("count"))
    identity["test_counts"] = {name: counts[name] for name in sorted(counts)}
    identity["structured_test_output"] = bool(counts)
    return identity


def _replay_transcript(path: Path, stream: object) -> None:
    binary = getattr(stream, "buffer", None)
    if binary is not None:
        with path.open("rb") as source:
            shutil.copyfileobj(source, binary, length=1024 * 1024)
        binary.flush()
        return
    with path.open("r", encoding="utf-8", errors="replace") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), ""):
            if not chunk:
                break
            stream.write(chunk)  # type: ignore[attr-defined]
    stream.flush()  # type: ignore[attr-defined]


def _requires_structured_test_counts(envelope: Mapping[str, object]) -> bool:
    argv = [str(value) for value in envelope["argv"]]  # type: ignore[index]
    nested = _nested_command(argv)
    payload = nested if nested is not None else argv
    lowered = [_basename(value) for value in payload]
    if any(value in _PYTHON_CONSOLE_SCRIPTS for value in lowered):
        return True
    for index, value in enumerate(payload[:-1]):
        if value == "-m" and payload[index + 1] in {"pytest", "py.test"}:
            return True
    return bool(
        payload
        and _basename(payload[0]) in {"cargo", "cargo.exe"}
        and "test" in payload[1:]
    )


def _in_python_environment(
    envelope: Mapping[str, object], exact: Sequence[str], payload: Sequence[str]
) -> list[str]:
    python = envelope.get("python")
    if isinstance(python, Mapping) and python.get("kind") in {
        "uv",
        "uv-console-script",
    }:
        prefix = python.get("prefix")
        assert isinstance(prefix, list)
        return [*exact[: len(prefix)], *payload]
    return list(payload)


def _which_in_command_environment(
    name: str,
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
) -> Path:
    python = envelope.get("python")
    if isinstance(python, Mapping) and python.get("kind") in {
        "uv",
        "uv-console-script",
    }:
        command = _in_python_environment(
            envelope, exact, ("python", "-c", _WHICH_SCRIPT, name)
        )
        payload = _parse_json_output(
            _run_captured(command, cwd=cwd, env=env), purpose=f"{name} path resolution"
        )
        found = payload.get("path")
        if not isinstance(found, str) or not found:
            raise ValueError(f"{name} is not on the proof command PATH")
        return _resolve_outer_executable(found, cwd=cwd, env=env)
    return _resolve_outer_executable(name, cwd=cwd, env=env)


def _tool_configuration_identities(
    name: str, *, cwd: Path, env: Mapping[str, str]
) -> list[dict[str, object]]:
    candidates: list[Path] = []
    if name in {"cargo", "rustc", "rustfmt", "cargo-deny", "cargo-audit"}:
        for parent in (cwd, *cwd.parents):
            candidates.extend(
                (parent / ".cargo" / "config.toml", parent / ".cargo" / "config")
            )
        cargo_home = env.get("CARGO_HOME")
        if cargo_home:
            candidates.extend(
                (Path(cargo_home) / "config.toml", Path(cargo_home) / "config")
            )
    if name == "lean":
        candidates.append(cwd / "formal" / "lean" / "lean-toolchain")
    identities: list[dict[str, object]] = []
    seen: set[str] = set()
    for candidate in candidates:
        if not candidate.is_file():
            continue
        resolved = candidate.resolve(strict=True)
        key = os.path.normcase(str(resolved))
        if key in seen:
            continue
        seen.add(key)
        identities.append(_file_identity(resolved))
    return sorted(identities, key=lambda item: os.path.normcase(str(item["path"])))


def _rust_target(exact: Sequence[str], env: Mapping[str, str]) -> str | None:
    selected_command = _nested_command(exact) or [str(value) for value in exact]
    selected: list[str] = []
    before_separator = True
    index = 1
    while index < len(selected_command) and before_separator:
        value = str(selected_command[index])
        if value == "--":
            before_separator = False
            break
        if value == "--target":
            if index + 1 >= len(selected_command):
                raise ValueError("Rust --target requires a value")
            selected.append(str(selected_command[index + 1]))
            index += 2
            continue
        if value.startswith("--target="):
            selected.append(value.split("=", 1)[1])
        index += 1
    environment_target = env.get("CARGO_BUILD_TARGET", "").strip()
    if environment_target:
        selected.append(environment_target)
    unique = list(dict.fromkeys(selected))
    if len(unique) > 1:
        raise ValueError(f"Rust target selection is ambiguous: {unique!r}")
    return unique[0] if unique else None


def _tool_identity(
    plan: proof_plan.ProofPlan,
    name: str,
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
) -> dict[str, object]:
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    try:
        policy = policies[name]
    except KeyError as exc:
        raise ValueError(f"proof plan has no {name!r} toolchain policy") from exc
    requested = str(policy.data.get("executable") or name)
    if requested == "{python}":
        raise ValueError("Python toolchain identity must use the runtime-closure probe")
    probe_cwd = cwd
    configured_probe_cwd = policy.data.get("probe_cwd")
    if configured_probe_cwd is not None:
        if not isinstance(configured_probe_cwd, str) or not configured_probe_cwd:
            raise ValueError(f"{name} toolchain probe cwd is malformed")
        relative_probe_cwd = Path(configured_probe_cwd)
        if relative_probe_cwd.is_absolute():
            raise ValueError(f"{name} toolchain probe cwd must be repository-relative")
        probe_cwd = (proof_plan.ROOT / relative_probe_cwd).resolve(strict=True)
    python_authority = envelope.get("python")
    if (
        not isinstance(python_authority, Mapping)
        and exact
        and _basename(exact[0]) in _executable_registry_names(requested)
    ):
        path = _resolve_outer_executable(exact[0], cwd=probe_cwd, env=env)
    else:
        path = _which_in_command_environment(
            requested, envelope, exact, cwd=probe_cwd, env=env
        )
    raw_version_args = policy.data.get("version_args")
    if not isinstance(raw_version_args, list) or not all(
        isinstance(value, str) and value for value in raw_version_args
    ):
        raise ValueError(f"{name} toolchain policy has no typed version command")
    version_args = tuple(raw_version_args)
    completed = _run_captured(
        _in_python_environment(envelope, exact, (str(path), *version_args)),
        cwd=probe_cwd,
        env=env,
    )
    if completed.returncode != 0:
        raise ValueError(f"{name} version probe failed: {completed.stderr.strip()}")
    content_path = path
    content_command = policy.data.get("content_path_command")
    content_resolver_identity: dict[str, object] | None = None
    if content_command is not None:
        if not isinstance(content_command, list) or not all(
            isinstance(value, str) and value for value in content_command
        ):
            raise ValueError(f"{name} content-path command is malformed")
        resolver = _which_in_command_environment(
            content_command[0], envelope, exact, cwd=probe_cwd, env=env
        )
        resolved = _run_captured(
            _in_python_environment(
                envelope, exact, (str(resolver), *content_command[1:])
            ),
            cwd=probe_cwd,
            env=env,
        )
        if resolved.returncode != 0 or not resolved.stdout.strip():
            raise ValueError(
                f"{name} content-path probe failed: "
                + (resolved.stderr.strip() or resolved.stdout.strip())
            )
        candidate = Path(resolved.stdout.strip())
        if not candidate.is_file():
            raise ValueError(
                f"{name} content-path probe returned no executable: {candidate}"
            )
        content_path = candidate.resolve(strict=True)
        content_resolver_identity = _executable_identity(resolver)
    process_images: list[dict[str, object]] = []
    launcher_image = process_image_capture.capture_image(f"{name}-launcher", path)
    process_images.append(launcher_image)
    if os.path.normcase(str(content_path)) == os.path.normcase(str(path)):
        content_image = launcher_image
    else:
        content_image = process_image_capture.capture_image(name, content_path)
        process_images.append(content_image)
    material: dict[str, object] = {
        "path": str(path),
        "launcher_sha256": launcher_image["sha256"],
        "content_path": str(content_path),
        "executable_sha256": content_image["sha256"],
        "version": (completed.stdout or completed.stderr).strip(),
        "probe_cwd": str(probe_cwd),
        "policy_sha256": hashlib.sha256(
            json.dumps(policy.data, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
        "configuration_files": _tool_configuration_identities(name, cwd=cwd, env=env),
    }
    if content_resolver_identity is not None:
        material["content_resolver"] = content_resolver_identity
    if name == "rustc":
        requested_toolchains = envelope.get("toolchains")
        cargo_path = None
        if isinstance(requested_toolchains, list) and "cargo" in requested_toolchains:
            cargo_path = _which_in_command_environment(
                "cargo", envelope, exact, cwd=probe_cwd, env=env
            )
        linker_images, linker_telemetry = (
            toolchain_capture.capture_rust_link_process_images(
                rustc=content_path,
                cargo=cargo_path,
                cwd=probe_cwd,
                env=env,
                target=_rust_target(exact, env),
                command_argv=_nested_command(exact) or exact,
                linker_process_helpers=(
                    policy.data.get("linker_process_helpers")
                    if isinstance(policy.data.get("linker_process_helpers"), Mapping)
                    else {}
                ),
            )
        )
        process_images.extend(linker_images)
        material["link_selection"] = linker_telemetry
    material["process_images"] = process_images
    if name == "node":
        node_probe = (
            "const m=require('module');"
            "console.log(JSON.stringify({execPath:process.execPath,"
            "versions:process.versions,config:process.config,globalPaths:m.globalPaths}))"
        )
        runtime = _run_captured(
            (str(content_path), "-e", node_probe), cwd=probe_cwd, env=env
        )
        runtime_payload = _parse_json_output(runtime, purpose="node runtime closure")
        exec_path = runtime_payload.get("execPath")
        if (
            not isinstance(exec_path, str)
            or Path(exec_path).resolve(strict=True) != content_path
        ):
            raise ValueError("node runtime closure resolved a substituted executable")
        material["runtime"] = runtime_payload
        material["runtime_sha256"] = hashlib.sha256(
            json.dumps(runtime_payload, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
    node_package = policy.data.get("node_package")
    if node_package is not None:
        if not isinstance(node_package, str) or not node_package:
            raise ValueError(f"{name} node package authority is malformed")
        node_path = _which_in_command_environment(
            "node", envelope, exact, cwd=probe_cwd, env=env
        )
        package_probe = (
            "const fs=require('fs'),p=require('path'),name=process.argv[1];"
            "const entry=require.resolve(name);let root=p.dirname(entry);"
            "for(;;){const manifest=p.join(root,'package.json');"
            "if(fs.existsSync(manifest)){const data=JSON.parse(fs.readFileSync(manifest));"
            "if(data.name===name){console.log(JSON.stringify({entry,manifest,root}));break;}}"
            "const parent=p.dirname(root);if(parent===root)throw new Error('package root not found');"
            "root=parent;}"
        )
        resolved_package = _parse_json_output(
            _run_captured(
                _in_python_environment(
                    envelope, exact, (str(node_path), "-e", package_probe, node_package)
                ),
                cwd=probe_cwd,
                env=env,
            ),
            purpose=f"{name} node package closure",
        )
        package_root_raw = resolved_package.get("root")
        entry_raw = resolved_package.get("entry")
        manifest_raw = resolved_package.get("manifest")
        if not all(
            isinstance(value, str) and value
            for value in (package_root_raw, entry_raw, manifest_raw)
        ):
            raise ValueError(f"{name} node package closure is malformed")
        package_root = Path(str(package_root_raw)).resolve(strict=True)
        entry = Path(str(entry_raw)).resolve(strict=True)
        manifest_path = Path(str(manifest_raw)).resolve(strict=True)
        for candidate, label in ((entry, "entry"), (manifest_path, "manifest")):
            try:
                candidate.relative_to(package_root)
            except ValueError as exc:
                raise ValueError(
                    f"{name} node package {label} escapes its resolved package root"
                ) from exc
        material["node_package"] = {
            "name": node_package,
            "entry": str(entry),
            "manifest": str(manifest_path),
            "resolver": _executable_identity(node_path),
            "package": _directory_manifest_identity(
                package_root, label=f"{name} node package"
            ),
        }
    material["identity_sha256"] = hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
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
        not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None
        for value in hash_values
    ):
        raise ValueError(f"{name} identity has no available executable content hash")
    if name == "python":
        runtime_digest = identity.get("runtime_closure_sha256")
        runtime = identity.get("runtime")
        if (
            not isinstance(runtime_digest, str)
            or re.fullmatch(r"[0-9a-f]{64}", runtime_digest) is None
            or not isinstance(runtime, Mapping)
            or not isinstance(runtime.get("runtime_file_count"), int)
            or int(runtime["runtime_file_count"]) <= 0
        ):
            raise ValueError("python identity has no complete CPython runtime closure")
    if name == "node":
        runtime_digest = identity.get("runtime_sha256")
        if (
            not isinstance(runtime_digest, str)
            or re.fullmatch(r"[0-9a-f]{64}", runtime_digest) is None
        ):
            raise ValueError("node identity has no runtime/configuration closure")


_ENVIRONMENT_EXACT_NAMES = frozenset(
    {
        "APPDATA",
        "COMSPEC",
        "HOME",
        "HOMEDRIVE",
        "HOMEPATH",
        "LANG",
        "LOCALAPPDATA",
        "LOGNAME",
        "NUMBER_OF_PROCESSORS",
        "NODE_OPTIONS",
        "OS",
        "PATH",
        "PATHEXT",
        "PROCESSOR_ARCHITECTURE",
        "PROCESSOR_IDENTIFIER",
        "PROGRAMDATA",
        "SHELL",
        "SYSTEMDRIVE",
        "SYSTEMROOT",
        "TEMP",
        "TERM",
        "TMP",
        "TMPDIR",
        "USER",
        "USERNAME",
        "USERPROFILE",
        "VIRTUAL_ENV",
        "WINDIR",
    }
)
_ENVIRONMENT_PREFIXES = (
    "AR_",
    "CARGO_",
    "CC_",
    "CI_",
    "CMAKE_",
    "CXX_",
    "GITHUB_",
    "LC_",
    "LLVM_",
    "MOLT_",
    "PYO3_",
    "PYTHON",
    "RUST",
    "SCCACHE_",
    "UV_",
    "WASM_",
    "XDG_",
)
_ENVIRONMENT_BUILD_NAMES = frozenset(
    {
        "AR",
        "CC",
        "CFLAGS",
        "CL",
        "CLANG",
        "CMAKE",
        "CXX",
        "CXXFLAGS",
        "DLLTOOL",
        "INCLUDE",
        "LDFLAGS",
        "LIB",
        "LINK",
        "LLVM_CONFIG",
        "MAKE",
        "MAKEFLAGS",
        "MESON",
        "NASM",
        "NINJAFLAGS",
        "NINJA",
        "NM",
        "OBJCOPY",
        "PERL",
        "PKG_CONFIG",
        "RANLIB",
        "RC",
        "RUSTC",
        "RUSTFLAGS",
        "STRIP",
        "YASM",
    }
)
_NONDETERMINISTIC_ENV_NAMES = frozenset(
    {
        "PYTHONBREAKPOINT",
        "PYTHONHOME",
        "PYTHONINSPECT",
        "PYTHONPATH",
        "PYTHONSTARTUP",
        "PYTHONUSERBASE",
        "PYTEST_ADDOPTS",
        "PYTEST_PLUGINS",
        "PYTEST_DISABLE_PLUGIN_AUTOLOAD",
        "UV_CONFIG_FILE",
        "UV_DEFAULT_INDEX",
        "UV_EXTRA_INDEX_URL",
        "UV_FIND_LINKS",
        "UV_INDEX",
        "UV_INDEX_URL",
    }
)
_CANONICAL_EXECUTION_ENV = {
    "PYTHONDONTWRITEBYTECODE": "1",
    "PYTHONNOUSERSITE": "1",
}
_QUEUE_CUSTODY_ENV_NAMES = frozenset(
    {
        "MOLT_PROOF_CHILD_CUSTODY_JSON",
        "MOLT_PROOF_CHILD_CUSTODY_ENDPOINT",
        "MOLT_PROOF_CHILD_CUSTODY_TOKEN",
    }
)
_EXECUTABLE_ENV_NAMES = frozenset(
    {
        "AR",
        "CC",
        "CMAKE",
        "CXX",
        "DLLTOOL",
        "LINK",
        "MAKE",
        "MESON",
        "NASM",
        "NINJA",
        "NM",
        "OBJCOPY",
        "PERL",
        "PKG_CONFIG",
        "RANLIB",
        "RC",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUNNER",
        "CLANG",
        "LLVM_CONFIG",
        "STRIP",
        "WASM_BINDGEN",
        "WASM_OPT",
        "YASM",
    }
)
_EXECUTABLE_ENV_PATTERNS = (
    re.compile(r"(?:AR|CC|CXX|RANLIB|RC|STRIP)_[A-Z0-9_]+"),
    re.compile(r"CARGO_TARGET_[A-Z0-9_]+_(?:LINKER|RUNNER)"),
    re.compile(r"CMAKE_(?:C|CXX)_COMPILER"),
)
_SECRET_ENV_NAME = re.compile(
    r"(?:TOKEN|SECRET|PASSWORD|PASSWD|API_?KEY|PRIVATE_?KEY|CREDENTIAL|COOKIE|AUTH)",
    re.IGNORECASE,
)
_SECRET_ARGUMENT_FLAG = re.compile(
    r"^--?(?:api[-_]?key|auth|credential|password|passwd|private[-_]?key|secret|token)(?:=|$)",
    re.IGNORECASE,
)


def command_secret_policy_error(command: Sequence[str]) -> str | None:
    for index, value in enumerate(command):
        if re.search(r"://[^/@\s]+@", value):
            return f"command argument {index} embeds URL credentials"
        if _SECRET_ARGUMENT_FLAG.match(value):
            return (
                f"secret-bearing command option {value.split('=', 1)[0]!r} is forbidden"
            )
    return None


def _environment_name_class(name: str) -> str | None:
    upper = name.upper()
    if upper in _QUEUE_CUSTODY_ENV_NAMES - {"PYTHONPATH"}:
        return "queue-owned-custody"
    if upper in _NONDETERMINISTIC_ENV_NAMES:
        return "denied-nondeterministic"
    if upper in _ENVIRONMENT_EXACT_NAMES:
        return "host-runtime"
    if upper in _ENVIRONMENT_BUILD_NAMES:
        return "build-toolchain"
    if any(upper.startswith(prefix) for prefix in _ENVIRONMENT_PREFIXES):
        return "semantic-prefix"
    return None


def environment_override_policy_error(env_overrides: Mapping[str, str]) -> str | None:
    seen: dict[str, str] = {}
    for name, value in sorted(env_overrides.items()):
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name) is None:
            return f"non-canonical environment override name {name!r}"
        folded = name.casefold()
        if folded in seen:
            return (
                "case-ambiguous environment overrides are forbidden: "
                f"{seen[folded]!r}, {name!r}"
            )
        seen[folded] = name
        if (
            name.upper() in _CANONICAL_EXECUTION_ENV
            or name.upper() == "NODE_OPTIONS"
            or name.upper() in _QUEUE_CUSTODY_ENV_NAMES
        ):
            return f"queue-owned canonical environment override {name!r} is forbidden"
        classification = _environment_name_class(name)
        if classification is None:
            return f"unclassified environment override {name!r}"
        if classification == "denied-nondeterministic":
            return f"nondeterministic environment override {name!r} is forbidden"
        if _SECRET_ENV_NAME.search(name):
            return f"secret-bearing environment override {name!r} is forbidden"
        if "\x00" in value or "\n" in value or "\r" in value:
            return f"environment override {name!r} has non-canonical control characters"
        if re.search(r"://[^/@\s]+@", value):
            return f"environment override {name!r} embeds URL credentials"
    return None


def _deterministic_execution_environment(
    inherited: Mapping[str, str], *, override_names: Sequence[str]
) -> tuple[dict[str, str], dict[str, object]]:
    override_keys = [name.casefold() for name in override_names]
    if len(override_keys) != len(set(override_keys)):
        raise ValueError("environment overrides contain case-ambiguous names")
    overrides = set(override_keys)
    selected: dict[str, str] = {}
    omitted: list[str] = []
    seen_names: set[str] = set()
    for name, value in inherited.items():
        folded = name.casefold()
        if folded in seen_names:
            raise ValueError(f"execution environment has case-ambiguous name {name!r}")
        seen_names.add(folded)
        classification = _environment_name_class(name)
        if classification in {
            None,
            "denied-nondeterministic",
        } or _SECRET_ENV_NAME.search(name):
            omitted.append(name)
            continue
        selected[name] = str(value)
    missing = sorted(
        name
        for name in override_names
        if name.casefold() not in {key.casefold() for key in selected}
    )
    if missing:
        raise ValueError(
            "classified environment overrides disappeared: " + ", ".join(missing)
        )
    contract: dict[str, object] = {
        "schema": "molt.proof-execution-environment.v1",
        "passed_names": sorted(selected, key=str.casefold),
        "override_names": sorted(
            (name for name in selected if name.casefold() in overrides),
            key=str.casefold,
        ),
        "omitted_names": sorted(omitted, key=str.casefold),
    }
    return selected, contract


def _execution_environment_authority(
    env: Mapping[str, str],
    *,
    applied_cargo_policies: Sequence[str],
    fingerprint_key: bytes,
    contract: Mapping[str, object],
) -> dict[str, object]:
    names = sorted(env, key=str.casefold)
    values: dict[str, object] = {}
    for name in names:
        normalized = str(env[name]).replace("\\", "/")
        values[name] = {
            "class": (
                "queue-owned-custody"
                if name.upper() == "PYTHONPATH"
                else _environment_name_class(name)
            ),
            "fingerprint": hmac.new(
                fingerprint_key,
                f"{name.casefold()}\0{normalized}".encode(),
                hashlib.sha256,
            ).hexdigest(),
            "redacted": True,
        }
    payload: dict[str, object] = {
        **dict(contract),
        "variables": values,
        "cargo_policies": list(applied_cargo_policies),
        "fingerprint_key_id": hashlib.sha256(fingerprint_key).hexdigest(),
        "canonical_values_sha256": _canonical_environment_sha256(env),
    }
    payload["identity_sha256"] = hashlib.sha256(
        json.dumps(payload, sort_keys=True).encode()
    ).hexdigest()
    return payload


def _canonical_environment_sha256(env: Mapping[str, str]) -> str:
    """Bind every admitted environment value without publishing those values."""
    canonical = {name: str(env[name]) for name in sorted(env, key=str.casefold)}
    return hashlib.sha256(
        json.dumps(canonical, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def _execution_environment_executable_identities(
    env: Mapping[str, str], *, cwd: Path
) -> dict[str, object]:
    identities: dict[str, object] = {}
    for name, value in sorted(env.items()):
        upper = name.upper()
        if upper not in _EXECUTABLE_ENV_NAMES and not any(
            pattern.fullmatch(upper) for pattern in _EXECUTABLE_ENV_PATTERNS
        ):
            continue
        if not value:
            continue
        try:
            parts = shlex.split(value, posix=os.name != "nt")
        except ValueError as exc:
            raise ValueError(f"executable environment {name} is malformed") from exc
        if not parts:
            raise ValueError(f"executable environment {name} is empty")
        token = parts[0].strip('"')
        path = _resolve_outer_executable(token, cwd=cwd, env=env)
        identity = _executable_identity(path)
        if not _content_identity_available(identity):
            raise ValueError(f"executable environment {name} has no content identity")
        identities[name] = {
            "executable": identity,
            "argument_count": len(parts) - 1,
        }
    return identities


def _capture_toolchains(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    source_root: Path,
    hash_workers: int,
    located_toolchains: Mapping[str, object],
) -> tuple[dict[str, object] | None, dict[str, object]]:
    plan = proof_plan.ProofPlan.load()
    requested_raw = envelope.get("toolchains")
    if (
        not isinstance(requested_raw, list)
        or not requested_raw
        or not all(isinstance(name, str) and name for name in requested_raw)
    ):
        raise ValueError("proof command envelope has no non-empty toolchain authority")
    requested = [str(name) for name in requested_raw]
    if len(requested) != len(set(requested)):
        raise ValueError("proof command envelope has duplicate toolchain authorities")
    known = {policy.name for policy in plan.toolchain_policies}
    unknown = sorted(set(requested) - known)
    if unknown:
        raise ValueError(f"proof command envelope has unknown toolchains: {unknown!r}")
    proof_python = _python_identity(
        envelope,
        exact,
        cwd=cwd,
        env=env,
        source_root=source_root,
        hash_workers=hash_workers,
    )
    if proof_python is None and "python" in requested:
        synthetic_envelope = envelope_for_command(
            [sys.executable, "-c", "raise SystemExit('identity-only')"]
        )
        proof_python = _python_identity(
            synthetic_envelope,
            [sys.executable, "-c", "raise SystemExit('identity-only')"],
            cwd=cwd,
            env=env,
            source_root=source_root,
            hash_workers=hash_workers,
        )
    toolchains: dict[str, object] = {}
    if proof_python is not None:
        toolchains["python"] = proof_python
    for name in requested:
        if name == "python":
            continue
        located = located_toolchains.get(name)
        if not isinstance(located, Mapping):
            raise ValueError(f"located {name} toolchain identity is unavailable")
        if name == "rustc":
            toolchain_capture.revalidate_rust_link_process_images(
                located,
                target=_rust_target(exact, env),
                command_argv=_nested_command(exact) or exact,
            )
        for frozen in toolchain_capture.frozen_files({name: located}):
            path = Path(frozen.path)
            if (
                _hash_file(path) != frozen.sha256
                or (
                    frozen.size is not None
                    and path.stat().st_size != frozen.size
                )
            ):
                raise ValueError(
                    f"{name} toolchain file changed while live custody armed: {path}"
                )
        toolchains[name] = dict(located)
    if set(toolchains) != set(requested):
        raise ValueError(
            "proof command toolchain capture is incomplete: "
            f"requested={sorted(requested)!r} captured={sorted(toolchains)!r}"
        )
    return proof_python, toolchains


def _locate_toolchain_watch_roots(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
) -> tuple[list[Path], dict[str, object], dict[str, object]]:
    """Locate broad roots and child executables without a full inventory."""
    started = time.perf_counter()
    plan = proof_plan.ProofPlan.load()
    requested_raw = envelope.get("toolchains")
    if not isinstance(requested_raw, list) or not all(
        isinstance(name, str) and name for name in requested_raw
    ):
        raise ValueError("proof command envelope has no toolchain authority")
    requested = [str(name) for name in requested_raw]
    roots: list[Path] = []
    policy_identities: dict[str, object] = {}
    if "python" in requested:
        python_envelope = envelope
        python_exact = list(exact)
        if envelope.get("python") is None:
            python_envelope = envelope_for_command(
                [sys.executable, "-c", "raise SystemExit('location-only')"]
            )
            python_exact = [sys.executable, "-c", "raise SystemExit('location-only')"]
        command = _python_auxiliary_command(
            python_envelope,
            python_exact,
            authority=_PYTHON_TOOLCHAIN_LOCATOR,
            arguments=(),
        )
        if command is None:
            raise ValueError("proof Python locator has no selected interpreter")
        located = _parse_json_output(
            _run_captured(command, cwd=cwd, env=env),
            purpose="proof Python toolchain locator",
        )
        if located.get("schema") != "molt.proof-python-toolchain-location.v1":
            raise ValueError("proof Python toolchain locator schema mismatch")
        executable_raw = located.get("executable")
        base_executable_raw = located.get("base_executable")
        if not isinstance(executable_raw, str) or not isinstance(
            base_executable_raw, str
        ):
            raise ValueError("proof Python toolchain locator has no executable chain")
        executable = Path(executable_raw).resolve(strict=True)
        base_executable = Path(base_executable_raw).resolve(strict=True)
        policy_identities["python"] = {
            "executable": str(executable),
            "executable_sha256": _hash_file(executable),
            "base_executable": str(base_executable),
            "base_executable_sha256": _hash_file(base_executable),
        }
        for field in ("roots", "editable_roots"):
            values = located.get(field)
            if not isinstance(values, list) or not all(
                isinstance(value, str) for value in values
            ):
                raise ValueError(f"proof Python locator has malformed {field}")
            for value in values:
                path = Path(value)
                if path.is_dir():
                    roots.append(path.resolve(strict=True))
    for name in requested:
        if name == "python":
            continue
        identity = _tool_identity(plan, name, envelope, exact, cwd=cwd, env=env)
        policy_identities[name] = identity
        roots.extend(_broad_toolchain_roots({name: identity}))
    roots = list(dict.fromkeys(roots))
    telemetry = {
        "schema": "molt.proof-toolchain-location-telemetry.v1",
        "non_python_selection_capture_count": sum(
            name != "python" for name in requested
        ),
        "root_count": len(roots),
        "locate_s": time.perf_counter() - started,
    }
    return roots, policy_identities, telemetry


def _python_editable_ineligible_reasons(
    identity: Mapping[str, object] | None,
    *,
    source_snapshot: Mapping[str, object],
) -> list[str]:
    if identity is None:
        return []
    reasons: list[str] = []
    distributions = identity.get("distributions")
    if not isinstance(distributions, list):
        return ["python-distribution-inventory-malformed"]
    for distribution in distributions:
        if not isinstance(distribution, Mapping):
            reasons.append("python-distribution-inventory-malformed")
            continue
        editable = distribution.get("editable_source")
        if not isinstance(editable, Mapping):
            continue
        name = str(distribution.get("name") or "unknown")
        if editable.get("inside_admitted_source") is not True:
            reasons.append(f"python-editable-source-outside:{name}")
        if (
            editable.get("source_metadata_root") is not None
            and editable.get("source_metadata_inside_admitted_source") is not True
        ):
            reasons.append(f"python-source-metadata-outside:{name}")
        if editable.get("git_available") is not True:
            reasons.append(f"python-editable-source-git-unavailable:{name}")
        elif editable.get("git_clean") is not True:
            reasons.append(f"python-editable-source-dirty:{name}")
        if editable.get("git_commit") != source_snapshot.get("commit"):
            reasons.append(f"python-editable-source-commit-mismatch:{name}")
        if editable.get("git_tree") != source_snapshot.get("tree"):
            reasons.append(f"python-editable-source-tree-mismatch:{name}")
    return reasons


def _git_snapshot(cwd: Path, env: Mapping[str, str]) -> dict[str, object]:
    head = _run_captured(("git", "rev-parse", "HEAD"), cwd=cwd, env=env)
    if head.returncode != 0 or not re.fullmatch(
        r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", head.stdout.strip()
    ):
        return {
            "available": False,
            "clean": False,
            "commit": None,
            "status_sha256": None,
        }
    root = _run_captured(("git", "rev-parse", "--show-toplevel"), cwd=cwd, env=env)
    if root.returncode != 0:
        return {
            "available": False,
            "clean": False,
            "commit": head.stdout.strip().lower(),
            "status_sha256": None,
        }
    source_root = Path(root.stdout.strip()).resolve(strict=True)
    tree = _run_captured(("git", "rev-parse", "HEAD^{tree}"), cwd=cwd, env=env)
    if tree.returncode != 0 or not re.fullmatch(
        r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", tree.stdout.strip()
    ):
        return {
            "available": False,
            "clean": False,
            "commit": head.stdout.strip().lower(),
            "tree": None,
            "status_sha256": None,
        }
    status = _run_captured(
        (
            "git",
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ),
        cwd=cwd,
        env=env,
        text=False,
    )
    if status.returncode != 0:
        return {
            "available": False,
            "clean": False,
            "commit": head.stdout.strip().lower(),
            "status_sha256": None,
        }
    return {
        "available": True,
        "root": str(source_root),
        "clean": not status.stdout,
        "commit": head.stdout.strip().lower(),
        "tree": tree.stdout.strip().lower(),
        "status_sha256": hashlib.sha256(status.stdout).hexdigest(),
    }


def _git_tracked_paths(cwd: Path, env: Mapping[str, str]) -> list[Path]:
    completed = _run_captured(
        ("git", "ls-files", "--cached", "--full-name", "-z"),
        cwd=cwd,
        env=env,
        text=False,
    )
    if completed.returncode != 0:
        raise ValueError("proof source custody cannot enumerate tracked inputs")
    root_result = _run_captured(
        ("git", "rev-parse", "--show-toplevel"), cwd=cwd, env=env
    )
    if root_result.returncode != 0:
        raise ValueError("proof source custody cannot resolve its Git root")
    root = Path(root_result.stdout.strip()).resolve(strict=True)
    paths: list[Path] = []
    for raw in completed.stdout.split(b"\0"):
        if not raw:
            continue
        relative = Path(os.fsdecode(raw))
        candidate = Path(os.path.abspath(root / relative))
        if candidate.is_file():
            paths.append(candidate)
    return paths


def _broad_toolchain_roots(toolchains: Mapping[str, object]) -> list[Path]:
    roots: list[Path] = []
    python = toolchains.get("python")
    if isinstance(python, Mapping):
        runtime = python.get("runtime")
        runtime_roots = (
            runtime.get("runtime_roots") if isinstance(runtime, Mapping) else None
        )
        if isinstance(runtime_roots, Mapping):
            for raw in runtime_roots.values():
                if isinstance(raw, str) and Path(raw).is_dir():
                    roots.append(Path(raw).resolve(strict=True))
        if isinstance(runtime, Mapping):
            for key in ("base_prefix", "prefix"):
                raw = runtime.get(key)
                if isinstance(raw, str) and Path(raw).is_dir():
                    roots.append(Path(raw).resolve(strict=True))
        distributions = python.get("distributions")
        if isinstance(distributions, list):
            for distribution in distributions:
                if not isinstance(distribution, Mapping):
                    continue
                for key in ("install_prefix", "editable_source"):
                    raw = distribution.get(key)
                    if isinstance(raw, str) and Path(raw).is_dir():
                        roots.append(Path(raw).resolve(strict=True))
    for identity in toolchains.values():
        if not isinstance(identity, Mapping):
            continue
        node_package = identity.get("node_package")
        package = (
            node_package.get("package") if isinstance(node_package, Mapping) else None
        )
        raw_root = package.get("root") if isinstance(package, Mapping) else None
        if isinstance(raw_root, str) and Path(raw_root).is_dir():
            roots.append(Path(raw_root).resolve(strict=True))
    return list(dict.fromkeys(roots))


def _atomic_json(path: Path, payload: Mapping[str, object]) -> None:
    custody_cas.atomic_write_bytes(
        path,
        (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode(),
    )


def _provision_proof_supervisor(
    *, cwd: Path, env: Mapping[str, str]
) -> tuple[Path, dict[str, object]]:
    started = time.perf_counter()
    build = _REPO_ROOT / "tools" / "proof_supervisor" / "build.py"
    completed = _run_captured(
        (sys.executable, str(build), "--release"),
        cwd=cwd,
        env=env,
        timeout=600.0,
    )
    if completed.returncode != 0:
        raise ValueError(
            "native proof supervisor provisioning failed: "
            + (completed.stderr.strip() or completed.stdout.strip())
        )
    lines = [line.strip() for line in completed.stdout.splitlines() if line.strip()]
    if not lines:
        raise ValueError("native proof supervisor build returned no binary")
    binary = Path(lines[-1]).resolve(strict=True)
    if not binary.is_file():
        raise ValueError("native proof supervisor binary is unavailable")
    binary_identity = _file_identity(binary)
    return binary, {
        "schema": "molt.proof-supervisor-provision-telemetry.v1",
        "build_s": time.perf_counter() - started,
        "build_target_dir": str(Path(env["CARGO_TARGET_DIR"]).resolve(strict=True)),
        "build_output_sha256": binary_identity["sha256"],
        "build_output_size_bytes": binary_identity["size_bytes"],
    }


def _supervisor_fixed_images(
    toolchains: Mapping[str, object],
    environment_executables: Mapping[str, object],
    execution_command: Sequence[str],
    platform_process_images: Sequence[Mapping[str, object]] = (),
) -> tuple[str, list[dict[str, str]]]:
    root = os.path.normcase(os.path.abspath(execution_command[0]))
    images: dict[str, dict[str, str]] = {}

    def add(
        role: str,
        raw_path: object,
        raw_digest: object,
        raw_root_exit_disposition: object = None,
    ) -> None:
        if not isinstance(raw_path, str) or not isinstance(raw_digest, str):
            return
        if re.fullmatch(r"[0-9a-f]{64}", raw_digest) is None:
            return
        path = Path(raw_path)
        if not path.is_absolute() or not path.is_file():
            return
        key = os.path.normcase(os.path.abspath(path))
        disposition = (
            str(raw_root_exit_disposition)
            if raw_root_exit_disposition is not None
            else "require-exit"
        )
        if disposition not in {"require-exit", "terminate"}:
            raise ValueError(f"supervisor image has invalid root-exit disposition: {path}")
        row = {"role": role, "path": str(path), "sha256": raw_digest}
        if disposition != "require-exit":
            row["root_exit_disposition"] = disposition
        prior = images.get(key)
        if prior is not None and prior["sha256"] != raw_digest:
            raise ValueError(f"supervisor image has conflicting identities: {path}")
        if prior is not None and prior.get("root_exit_disposition", "require-exit") != disposition:
            raise ValueError(
                f"supervisor image has conflicting root-exit dispositions: {path}"
            )
        if prior is None:
            images[key] = row

    root_path = Path(os.path.abspath(execution_command[0]))
    if not root_path.is_file():
        raise ValueError("supervisor root executable is unavailable")
    add("root-command", str(root_path), _hash_file(root_path))
    for name, raw in toolchains.items():
        if not isinstance(raw, Mapping):
            continue
        add(str(name), raw.get("executable"), raw.get("executable_sha256"))
        add(str(name), raw.get("path"), raw.get("launcher_sha256"))
        add(str(name), raw.get("content_path"), raw.get("executable_sha256"))
        process_images = raw.get("process_images")
        if isinstance(process_images, list):
            for image in process_images:
                if isinstance(image, Mapping):
                    add(
                        str(image.get("role") or name),
                        image.get("path"),
                        image.get("sha256"),
                        image.get("root_exit_disposition"),
                    )
    for name, raw in environment_executables.items():
        if not isinstance(raw, Mapping):
            continue
        executable = raw.get("executable")
        if isinstance(executable, Mapping):
            add(f"env:{name}", executable.get("path"), executable.get("sha256"))
    for image in platform_process_images:
        add(
            str(image.get("role") or "platform-process"),
            image.get("path"),
            image.get("sha256"),
            image.get("root_exit_disposition"),
        )
    root_image = images.get(root)
    if root_image is None:
        raise ValueError("supervisor policy has no captured root executable image")
    return root_image["role"], [images[key] for key in sorted(images)]


def _supervisor_derived_roots(
    *, descendants: object, env: Mapping[str, str]
) -> list[dict[str, str]]:
    if descendants == "forbidden":
        return []
    roots: list[dict[str, str]] = []
    for role, name in (("build-output", "CARGO_TARGET_DIR"),):
        raw = env.get(name)
        if not raw:
            continue
        path = Path(raw)
        if not path.is_absolute() or not path.is_dir():
            raise ValueError(f"declared-tree supervisor requires existing absolute {name}")
        roots.append({"role": role, "path": str(path.resolve(strict=True))})
    return roots


def _derived_root_provenance(
    *,
    descendants: object,
    env: Mapping[str, str],
    source_root: Path,
    result_path: Path,
) -> list[dict[str, object]]:
    roots = _supervisor_derived_roots(descendants=descendants, env=env)
    source = source_root.resolve(strict=True)
    cas_root = Path(os.path.abspath(result_path.parent / "custody-cas"))
    admitted: list[dict[str, object]] = []
    for row in roots:
        path = Path(row["path"])
        lexical = Path(os.path.abspath(path))
        resolved = lexical.resolve(strict=True)
        if os.path.normcase(str(lexical)) != os.path.normcase(str(resolved)):
            raise ValueError("derived executable root may not traverse a symlink")
        if (
            resolved == source
            or resolved.is_relative_to(source)
            or source.is_relative_to(resolved)
        ):
            raise ValueError("derived executable root overlaps admitted source")
        if (
            resolved == cas_root
            or resolved.is_relative_to(cas_root)
            or cas_root.is_relative_to(resolved)
        ):
            raise ValueError("derived executable root overlaps proof custody CAS")
        if resolved == Path(os.path.abspath(result_path)):
            raise ValueError("derived executable root overlaps terminal result")
        entries = list(resolved.iterdir())
        if entries:
            raise ValueError(
                f"derived executable root is not fresh and empty: {resolved}"
            )
        admitted.append(
            {
                **row,
                "initial_entry_count": 0,
                "initial_manifest_sha256": _canonical_payload_sha256([]),
                "run_owned": True,
            }
        )
    return admitted


def _supervisor_policy(
    *,
    envelope: Mapping[str, object],
    execution_command: Sequence[str],
    execution_env: Mapping[str, str],
    cwd: Path,
    nonce: str,
    toolchains: Mapping[str, object],
    environment_executables: Mapping[str, object],
    platform_process_images: Sequence[Mapping[str, object]],
) -> dict[str, object]:
    closure = envelope.get("process_closure")
    if not isinstance(closure, Mapping):
        raise ValueError("proof envelope has no supervisor closure authority")
    descendants = closure.get("descendants")
    mode = "leaf" if descendants == "forbidden" else "declared-tree"
    root_role, fixed_images = _supervisor_fixed_images(
        toolchains,
        environment_executables,
        execution_command,
        platform_process_images,
    )
    return {
        "schema": "molt.proof-process-closure.v2",
        "nonce": nonce,
        "mode": mode,
        "cwd": str(cwd.resolve(strict=True)),
        "command": [str(value) for value in execution_command],
        "environment": dict(sorted(execution_env.items(), key=lambda item: item[0].casefold())),
        "root_role": root_role,
        "fixed_images": fixed_images,
        "derived_roots": _supervisor_derived_roots(
            descendants=descendants, env=execution_env
        ),
    }


def _validated_supervisor_receipt(
    *,
    binary: Path,
    policy_path: Path,
    receipt_path: Path,
    cwd: Path,
    env: Mapping[str, str],
) -> dict[str, object]:
    verified = _run_captured(
        (
            str(binary),
            "verify",
            "--policy",
            str(policy_path),
            "--receipt",
            str(receipt_path),
        ),
        cwd=cwd,
        env=env,
    )
    if verified.returncode != 0:
        raise ValueError(
            "native proof supervisor receipt verification failed: "
            + (verified.stderr.strip() or verified.stdout.strip())
        )
    try:
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError("native proof supervisor returned no readable receipt") from exc
    if not isinstance(receipt, dict):
        raise ValueError("native proof supervisor receipt is not an object")
    return receipt


def _publish_supervisor_event_artifact(
    *, receipt_path: Path, receipt: Mapping[str, object], cas_root: Path
) -> dict[str, object]:
    event_log = receipt.get("event_log")
    if not isinstance(event_log, Mapping):
        raise ValueError("native proof supervisor receipt has no event artifact")
    file_name = event_log.get("file")
    expected_sha256 = event_log.get("sha256")
    expected_bytes = event_log.get("bytes")
    expected_count = event_log.get("count")
    if (
        not isinstance(file_name, str)
        or Path(file_name).name != file_name
        or not isinstance(expected_sha256, str)
        or re.fullmatch(r"[0-9a-f]{64}", expected_sha256) is None
        or not isinstance(expected_bytes, int)
        or isinstance(expected_bytes, bool)
        or expected_bytes < 0
        or not isinstance(expected_count, int)
        or isinstance(expected_count, bool)
        or expected_count < 0
    ):
        raise ValueError("native proof supervisor event artifact descriptor is malformed")
    event_path = receipt_path.with_name(file_name).resolve(strict=True)
    if event_path.parent != receipt_path.parent.resolve(strict=True):
        raise ValueError("native proof supervisor event artifact escaped its receipt directory")
    digest = hashlib.sha256()
    size = 0
    count = 0
    final_byte = None
    with event_path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
            size += len(chunk)
            count += chunk.count(b"\n")
            final_byte = chunk[-1]
    if (
        digest.hexdigest() != expected_sha256
        or size != expected_bytes
        or count != expected_count
        or (size > 0 and final_byte != ord("\n"))
    ):
        raise ValueError("native proof supervisor event artifact identity changed")
    artifact = custody_cas.put_file(
        cas_root, event_path, logical_name=file_name, executable=False
    ).as_dict()
    if (
        artifact.get("sha256") != expected_sha256
        or artifact.get("size_bytes") != expected_bytes
    ):
        raise ValueError("durable supervisor event artifact identity mismatch")
    return {
        "schema": "molt.proof-process-event-artifact.v1",
        "artifact": artifact,
        "count": expected_count,
        "bytes": expected_bytes,
        "sha256": expected_sha256,
    }


def _canonical_payload_sha256(payload: object) -> str:
    return hashlib.sha256(
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def _publish_live_custody_receipt(
    receipt: Mapping[str, object], *, cas_root: Path
) -> dict[str, object]:
    events = receipt.get("events")
    errors = receipt.get("errors")
    lifecycle = receipt.get("lifecycle")
    state = receipt.get("state")
    if not isinstance(events, list) or not isinstance(errors, list):
        raise ValueError("live custody receipt event authority is malformed")
    artifact_payload = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "kind": "live-input-custody-events",
        "events": events,
        "errors": errors,
    }
    artifact = custody_cas.put_json(cas_root, artifact_payload).as_dict()
    material = {
        "events": events,
        "errors": errors,
        "state": state,
        "lifecycle": lifecycle,
    }
    if receipt.get("identity_sha256") != _canonical_payload_sha256(material):
        raise ValueError("live custody receipt identity is inconsistent")
    return {
        **{key: value for key, value in receipt.items() if key not in {"events", "errors"}},
        "event_artifact": artifact,
        "event_count": len(events),
        "error_count": len(errors),
    }


def execution_custody_sha256(
    context: Mapping[str, object], *, run_id: str, returncode: int
) -> str:
    custody_context = dict(context)
    for field in (
        "execution_custody_sha256",
        "guard_receipt",
        "terminal_evidence_sha256",
    ):
        custody_context.pop(field, None)
    material = {
        "run_id": run_id,
        "execution_nonce_sha256": custody_context.get("execution_nonce_sha256"),
        "command_returncode": returncode,
        "receipt_context": custody_context,
    }
    return hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def terminal_evidence_sha256(
    context: Mapping[str, object], *, run_id: str, returncode: int
) -> str:
    terminal_context = dict(context)
    terminal_context.pop("terminal_evidence_sha256", None)
    material = {
        "run_id": run_id,
        "execution_nonce_sha256": terminal_context.get("execution_nonce_sha256"),
        "command_returncode": returncode,
        "receipt_context": terminal_context,
    }
    return hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def execute_guarded_request(request_path: Path) -> int:
    """Run identity, preflight, proof, and completion custody under one guard."""
    request = json.loads(request_path.read_text(encoding="utf-8"))
    if not isinstance(request, dict):
        raise ValueError("proof execution request must be an object")
    if request.get("schema") != EXECUTION_SCHEMA:
        raise ValueError("proof execution request schema mismatch")
    command = request.get("command")
    envelope = request.get("envelope")
    result_path = Path(str(request["result_path"]))
    cwd = Path(str(request["cwd"]))
    run_id = request.get("run_id")
    execution_nonce = request.get("execution_nonce")
    timeout_seconds = request.get("timeout_seconds")
    override_names = request.get("env_override_names", [])
    if not isinstance(command, list) or not isinstance(envelope, dict):
        raise ValueError("proof execution request has no typed command envelope")
    if not isinstance(run_id, str) or not run_id:
        raise ValueError("proof execution request has no run identity")
    if not isinstance(execution_nonce, str) or not re.fullmatch(
        r"[0-9a-f]{64}", execution_nonce
    ):
        raise ValueError("proof execution request has no canonical nonce")
    if (
        not isinstance(timeout_seconds, (int, float))
        or isinstance(timeout_seconds, bool)
        or not math.isfinite(float(timeout_seconds))
        or float(timeout_seconds) <= 0
    ):
        raise ValueError("proof execution request has no finite positive timeout")
    execution_deadline = time.monotonic() + float(timeout_seconds)
    shutdown_reserve = min(2.0, max(0.1, float(timeout_seconds) * 0.05))
    if not isinstance(override_names, list) or not all(
        isinstance(name, str) for name in override_names
    ):
        raise ValueError(
            "proof execution request has malformed environment override names"
        )
    command = [str(value) for value in command]
    validate_envelope(envelope, command)
    execution_custody.require_enforceable_process_closure(envelope)
    effective_cwd, overlay_paths = _execution_source_paths(envelope, cwd=cwd)
    _require_external_execution_outputs(
        result_path=result_path, effective_source=effective_cwd
    )
    result: dict[str, object] = {
        "schema": EXECUTION_SCHEMA,
        "run_id": run_id,
        "execution_nonce": execution_nonce,
        "envelope": envelope,
        "phase": "identity",
        "command_started": False,
    }
    custody_session: execution_custody.ExecutionCustodySession | None = None
    try:
        inherited_env = dict(os.environ)
        ambient_cargo_target = inherited_env.get("CARGO_TARGET_DIR", "").strip()
        applied_cargo_policies: tuple[str, ...] = ()
        if "cargo" in envelope.get("toolchains", []):
            inherited_env, applied_cargo_policies = normalize_cargo_environment(
                inherited_env
            )
            # Proof-produced executables require run provenance. Ordinary developer
            # builds keep the persistent target authority, while guarded proofs use
            # a fresh target and retain shared compiler caches outside that tree.
            run_target = (
                result_path.parent / "derived" / execution_nonce / "cargo-target"
            )
            run_target.mkdir(parents=True, exist_ok=False)
            inherited_env["CARGO_TARGET_DIR"] = str(run_target.resolve(strict=True))
        canonical_env = dict(_CANONICAL_EXECUTION_ENV)
        if "node" in envelope.get("toolchains", []):
            node_hook = (
                Path(execution_custody.__file__)
                .with_name("node_child_custody.cjs")
                .resolve(strict=True)
            )
            canonical_env["NODE_OPTIONS"] = (
                f"--no-global-search-paths --require={node_hook}"
            )
        inherited_env.update(canonical_env)
        execution_env, environment_contract = _deterministic_execution_environment(
            inherited_env,
            override_names=[
                *[str(name) for name in override_names],
                *sorted(canonical_env),
            ],
        )
        process_closure = envelope.get("process_closure")
        if not isinstance(process_closure, Mapping):
            raise ValueError("proof command envelope has no process closure")
        derived_root_provenance = _derived_root_provenance(
            descendants=process_closure.get("descendants"),
            env=execution_env,
            source_root=effective_cwd,
            result_path=result_path,
        )
        # Provisioning belongs before the custody snapshot.  No tool may change
        # after its bytes become the authority consumed by the proof command.
        from tools.proof_queue_pkg import policy

        preflight = policy._ensure_run_toolchain_preflight(
            repo_root=cwd, resource_family=str(request["resource_family"])
        )
        if preflight:
            raise ValueError("toolchain preflight failed: " + "; ".join(preflight))
        supervisor_build_env = dict(execution_env)
        if ambient_cargo_target:
            ambient_target = Path(ambient_cargo_target)
            if not ambient_target.is_absolute():
                ambient_target = cwd / ambient_target
            supervisor_target = ambient_target.resolve().parent / "proof-supervisor"
        else:
            supervisor_target = (
                Path(tempfile.gettempdir()).resolve()
                / "molt-proof-supervisor-target"
            )
        source_root = effective_cwd.resolve(strict=True)
        supervisor_target = Path(os.path.abspath(supervisor_target))
        if (
            supervisor_target == source_root
            or supervisor_target.is_relative_to(source_root)
        ):
            raise ValueError("native proof supervisor target overlaps admitted source")
        supervisor_target.mkdir(parents=True, exist_ok=True)
        supervisor_build_env["CARGO_TARGET_DIR"] = str(
            supervisor_target.resolve(strict=True)
        )
        built_supervisor, supervisor_provision_telemetry = _provision_proof_supervisor(
            cwd=cwd, env=supervisor_build_env
        )
        supervisor_binary_artifact = custody_cas.put_file(
            result_path.parent / "custody-cas",
            built_supervisor,
            logical_name=built_supervisor.name,
            executable=True,
        ).as_dict()
        supervisor_binary = Path(str(supervisor_binary_artifact["path"])).resolve(
            strict=True
        )
        environment_fingerprint_key = secrets.token_bytes(32)
        exact = _exact_command(envelope, cwd=cwd, env=execution_env)
        payload_executable_pre = _payload_executable_identity(envelope, exact)
        guarded_exec_pre, delegated_pre = _bind_delegated_command(
            envelope,
            exact,
            cwd=cwd,
            env=execution_env,
        )
        executable_pre = _executable_identity(Path(exact[0]))
        overlay_pre = [_file_identity(path) for path in overlay_paths]
        pre_identities = [executable_pre, *overlay_pre]
        if payload_executable_pre is not None:
            pre_identities.append(payload_executable_pre)
        if guarded_exec_pre is not None:
            pre_identities.append(guarded_exec_pre)
        if delegated_pre is not None:
            pre_identities.append(delegated_pre)
        if not all(
            _content_identity_available(identity) for identity in pre_identities
        ):
            raise ValueError(
                "proof command or overlay input has unavailable content identity"
            )
        pre_source = _git_snapshot(effective_cwd, execution_env)
        plan = proof_plan.ProofPlan.load()
        located_roots, policy_identities, location_telemetry = (
            _locate_toolchain_watch_roots(
                envelope,
                exact,
                cwd=cwd,
                env=execution_env,
            )
        )
        child_policy = execution_custody.child_policy(envelope, policy_identities)
        python_authority = envelope.get("python")
        python_has_payload = isinstance(python_authority, Mapping) and (
            parse_python_invocation(
                _python_invocation_argv(
                    [str(value) for value in envelope["argv"]], python_authority
                )
            ).mode
            != "terminal"
        )
        expected_child_runtime = (
            "python"
            if python_has_payload
            else (
                "node"
                if _basename(str(envelope["argv"][0])) in {"node", "node.exe"}
                else None
            )
        )
        child_event_server = execution_custody.ChildCustodyEventServer(
            expected_child_runtime, child_policy
        )
        execution_env[execution_custody.CHILD_POLICY_ENV] = json.dumps(
            child_policy, sort_keys=True, separators=(",", ":")
        )
        execution_env.update(child_event_server.environment())
        passed_names = environment_contract["passed_names"]
        override_names_contract = environment_contract["override_names"]
        assert isinstance(passed_names, list)
        assert isinstance(override_names_contract, list)
        for name in (
            execution_custody.CHILD_POLICY_ENV,
            execution_custody.CHILD_ENDPOINT_ENV,
            execution_custody.CHILD_TOKEN_ENV,
        ):
            if name not in passed_names:
                passed_names.append(name)
            if name not in override_names_contract:
                override_names_contract.append(name)
        passed_names.sort(key=str.casefold)
        override_names_contract.sort(key=str.casefold)
        execution_command, python_launcher_environment = _supervised_execution_command(
            envelope, exact, policy_identities
        )
        execution_env.update(python_launcher_environment)
        for name in sorted(python_launcher_environment):
            if name not in passed_names:
                passed_names.append(name)
            if name not in override_names_contract:
                override_names_contract.append(name)
        passed_names.sort(key=str.casefold)
        override_names_contract.sort(key=str.casefold)
        environment_executables_pre = _execution_environment_executable_identities(
            execution_env, cwd=cwd
        )
        process_closure = envelope.get("process_closure")
        if not isinstance(process_closure, Mapping):
            raise ValueError("proof envelope has no process-closure authority")
        platform_process_images_pre = process_image_capture.platform_auxiliary_images(
            process_closure.get("descendants")
        )
        custody_authority_paths = [
            Path(execution_custody.__file__).resolve(strict=True),
            supervisor_binary,
        ]
        supervisor_source = _REPO_ROOT / "tools" / "proof_supervisor"
        custody_authority_paths.extend(
            path.resolve(strict=True)
            for path in (
                supervisor_source / "build.py",
                supervisor_source / "Cargo.toml",
                supervisor_source / "Cargo.lock",
                *sorted((supervisor_source / "src").rglob("*.rs")),
            )
        )
        if python_has_payload:
            custody_authority_paths.append(
                _PYTHON_CUSTODY_BOOTSTRAP.resolve(strict=True)
            )
        if "node" in envelope.get("toolchains", []):
            custody_authority_paths.extend(
                Path(execution_custody.__file__).with_name(name).resolve(strict=True)
                for name in (
                    "node_child_custody.cjs",
                    "node_child_custody_worker.cjs",
                )
            )
        custody_authorities_pre = [
            _file_identity(path) for path in dict.fromkeys(custody_authority_paths)
        ]
        if not all(
            _content_identity_available(identity)
            for identity in custody_authorities_pre
        ):
            raise ValueError("proof custody authority has unavailable content identity")
        tracked_paths = _git_tracked_paths(effective_cwd, execution_env)
        source_root_raw = pre_source.get("root")
        if not isinstance(source_root_raw, str):
            raise ValueError("proof source custody has no canonical Git root")
        watch_identities: list[object] = [
            executable_pre,
            *overlay_pre,
            policy_identities,
            environment_executables_pre,
            custody_authorities_pre,
            platform_process_images_pre,
        ]
        if payload_executable_pre is not None:
            watch_identities.append(payload_executable_pre)
        if guarded_exec_pre is not None:
            watch_identities.append(guarded_exec_pre)
        if delegated_pre is not None:
            watch_identities.append(delegated_pre)
        source_root_path = Path(source_root_raw).resolve(strict=True)
        broad_roots = [
            root
            for root in located_roots
            if root != source_root_path and not root.is_relative_to(source_root_path)
        ]
        live_watch_specs = execution_custody.watch_specs(
            source_root=source_root_path,
            tracked_paths=tracked_paths,
            identities=watch_identities,
            broad_roots=broad_roots,
        )
        monitor = execution_custody.LiveCustodyMonitor(live_watch_specs)
        custody_session = execution_custody.ExecutionCustodySession(
            monitor=monitor,
            child_server=child_event_server,
        )
        custody_session.__enter__()

        platform_process_images_armed = process_image_capture.revalidate_images(
            platform_process_images_pre
        )
        if platform_process_images_armed != platform_process_images_pre:
            raise ValueError("platform process-image custody changed while arming")

        # Python's mutable package inventory is captured once after custody is
        # armed. Non-Python selection ran exactly once pre-arm so its complete
        # executable/config closure could itself be watched; only exact-path
        # content revalidation is permitted here.
        pre_source = _git_snapshot(effective_cwd, execution_env)
        if pre_source.get("root") != source_root_raw:
            raise ValueError("proof source root changed while live custody armed")
        executable_pre = _executable_identity(Path(exact[0]))
        payload_executable_pre = _payload_executable_identity(envelope, exact)
        overlay_pre = [_file_identity(path) for path in overlay_paths]
        guarded_exec_pre = (
            _file_identity(Path(str(guarded_exec_pre["path"])))
            if guarded_exec_pre is not None
            else None
        )
        delegated_pre = (
            _executable_identity(Path(str(delegated_pre["path"])))
            if delegated_pre is not None
            else None
        )
        _proof_python_full, toolchains_full = _capture_toolchains(
            envelope,
            exact,
            cwd=cwd,
            env=execution_env,
            source_root=effective_cwd,
            hash_workers=plan.inventory_hash_workers,
            located_toolchains=policy_identities,
        )
        for name, identity in toolchains_full.items():
            assert isinstance(identity, Mapping)
            _validate_toolchain_identity(plan, name, identity)
        if execution_custody.child_policy(envelope, toolchains_full) != child_policy:
            raise ValueError("toolchain closure changed while live custody armed")
        toolchains, capture_ref, capture_telemetry = toolchain_capture.publish_capture(
            result_path.parent / "custody-cas", toolchains_full
        )
        frozen = toolchain_capture.frozen_files(toolchains_full)
        uncovered = [
            row.path
            for row in frozen
            if not any(spec.owns(Path(row.path)) for spec in live_watch_specs)
        ]
        if uncovered:
            raise ValueError(
                "toolchain capture contains paths outside armed custody: "
                + ", ".join(uncovered[:3])
            )
        proof_python = toolchains.get("python")
        if proof_python is not None and not isinstance(proof_python, dict):
            raise ValueError("compact Python toolchain summary is malformed")
        supervisor_policy_path = result_path.with_suffix(".supervisor-policy.json")
        supervisor_receipt_path = result_path.with_suffix(".supervisor-receipt.json")
        for supervisor_output in (supervisor_policy_path, supervisor_receipt_path):
            try:
                supervisor_output.unlink()
            except FileNotFoundError:
                pass
        supervisor_policy = _supervisor_policy(
            envelope=envelope,
            execution_command=execution_command,
            execution_env=execution_env,
            cwd=cwd,
            nonce=execution_nonce,
            toolchains=toolchains,
            environment_executables=environment_executables_pre,
            platform_process_images=platform_process_images_pre,
        )
        _atomic_json(supervisor_policy_path, supervisor_policy)
        supervisor_policy_identity = _file_identity(supervisor_policy_path)
        custody_session.mark_captured()
        del _proof_python_full, toolchains_full, frozen
        environment_executables_pre = _execution_environment_executable_identities(
            execution_env, cwd=cwd
        )
        custody_authorities_pre = [
            _file_identity(path) for path in custody_authority_paths
        ]
        authoritative_pre_identities = [
            executable_pre,
            *overlay_pre,
            *custody_authorities_pre,
        ]
        for optional_identity in (
            payload_executable_pre,
            guarded_exec_pre,
            delegated_pre,
        ):
            if optional_identity is not None:
                authoritative_pre_identities.append(optional_identity)
        if not all(
            _content_identity_available(identity)
            for identity in authoritative_pre_identities
        ):
            raise ValueError(
                "proof execution input became unavailable after live custody armed"
            )
        python_version = "none"
        if proof_python is not None:
            match = re.match(r"(\d+\.\d+)", str(proof_python["version"]))
            if match is None:
                raise ValueError("proof Python identity has no major.minor version")
            python_version = match.group(1)
        context: dict[str, object] = {
            "schema": plan.receipt_schema,
            "authority_sha256": proof_plan._authority_sha256(plan),
            "run_id": run_id,
            "execution_nonce_sha256": hashlib.sha256(
                execution_nonce.encode()
            ).hexdigest(),
            "source_commit": pre_source.get("commit"),
            "source_tree": pre_source.get("tree"),
            "source_tree_state": "clean" if pre_source.get("clean") else "dirty",
            "environment": {
                "os": proof_plan._normalized_os(),
                "arch": proof_plan._normalized_arch(),
                "python": python_version,
            },
            "toolchains": toolchains,
            "toolchain_custody": {
                "capture_semantic_sha256": capture_ref["semantic_sha256"],
            },
            "toolchain_capture": {
                "schema": "molt.proof-toolchain-custody.v1",
                "artifact": capture_ref,
                "telemetry": {
                    "location": location_telemetry,
                    "capture": capture_telemetry,
                },
            },
            "command_envelope": envelope,
            "command_envelope_sha256": hashlib.sha256(
                json.dumps(envelope, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest(),
            "exact_command_sha256": hashlib.sha256(
                json.dumps(execution_command, separators=(",", ":")).encode()
            ).hexdigest(),
            "command_executable": {"prelaunch": executable_pre},
            "payload_command_executable": (
                {"prelaunch": payload_executable_pre}
                if payload_executable_pre is not None
                else None
            ),
            "guarded_exec": (
                {"prelaunch": guarded_exec_pre}
                if guarded_exec_pre is not None
                else None
            ),
            "delegated_command_executable": (
                {"prelaunch": delegated_pre} if delegated_pre is not None else None
            ),
            "execution_environment": {
                "prelaunch": _execution_environment_authority(
                    execution_env,
                    applied_cargo_policies=applied_cargo_policies,
                    fingerprint_key=environment_fingerprint_key,
                    contract=environment_contract,
                ),
                "executable_inputs": {"prelaunch": environment_executables_pre},
            },
            "platform_process_custody": {
                "schema": process_image_capture.PROCESS_IMAGE_SCHEMA,
                "prelaunch": platform_process_images_pre,
                "prelaunch_sha256": _canonical_payload_sha256(
                    platform_process_images_pre
                ),
            },
            "python_interpreters": {
                "queue_control_plane": {
                    "executable": sys.executable,
                    "implementation": platform.python_implementation(),
                    "version": platform.python_version(),
                    "role": "queue-runner-and-memory-guard",
                },
                "proof_command": (
                    {
                        **{
                            key: proof_python.get(key)
                            for key in (
                                "executable",
                                "implementation",
                                "version",
                                "identity_sha256",
                            )
                        },
                        "role": "proof-command-envelope",
                    }
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
            "child_process_custody": {
                "policy": child_policy,
                "transport": "parent-owned-authenticated-loopback",
            },
            "derived_root_custody": {
                "prelaunch": derived_root_provenance,
                "policy_roots": [
                    {"role": row["role"], "path": row["path"]}
                    for row in derived_root_provenance
                ],
            },
            "process_supervisor": {
                "schema": "molt.proof-process-supervision.v1",
                "binary": _file_identity(supervisor_binary),
                "binary_artifact": supervisor_binary_artifact,
                "policy": supervisor_policy_identity,
                "provision_telemetry": supervisor_provision_telemetry,
            },
            "custody_authorities": {"prelaunch": custody_authorities_pre},
            "live_input_custody": {
                "state": custody_session.state,
                "watch_roots": len(live_watch_specs),
            },
        }
        result.update(
            {
                "phase": "command",
                "receipt_context": context,
                "exact_command_sha256": context["exact_command_sha256"],
            }
        )
        _atomic_json(result_path, result)
        result["command_started"] = True
        stdout_path = result_path.with_suffix(".stdout.bin")
        stderr_path = result_path.with_suffix(".stderr.bin")
        for transcript_path in (stdout_path, stderr_path):
            try:
                transcript_path.unlink()
            except FileNotFoundError:
                pass
        custody_session.mark_running()
        supervisor_started = time.perf_counter()
        with (
            stdout_path.open("xb") as stdout_handle,
            stderr_path.open("xb") as stderr_handle,
        ):
            supervisor_process = _COMMANDS.start_owned(
                (
                    str(supervisor_binary),
                    "run",
                    "--policy",
                    str(supervisor_policy_path),
                    "--receipt",
                    str(supervisor_receipt_path),
                ),
                cwd=cwd,
                env=execution_env,
                stdout=stdout_handle,
                stderr=stderr_handle,
            )
            supervisor_timeout = execution_deadline - time.monotonic() - shutdown_reserve
            if supervisor_timeout <= 0:
                raise subprocess.TimeoutExpired(
                    [str(supervisor_binary), "run"], float(timeout_seconds)
                )
            supervisor_returncode = _COMMANDS.wait_owned(
                supervisor_process,
                timeout=supervisor_timeout,
                terminate_timeout=shutdown_reserve,
            )
            stdout_handle.flush()
            stderr_handle.flush()
            os.fsync(stdout_handle.fileno())
            os.fsync(stderr_handle.fileno())
        supervisor_run_s = time.perf_counter() - supervisor_started
        supervisor_receipt = _validated_supervisor_receipt(
            binary=supervisor_binary,
            policy_path=supervisor_policy_path,
            receipt_path=supervisor_receipt_path,
            cwd=cwd,
            env=execution_env,
        )
        supervisor_event_artifact = _publish_supervisor_event_artifact(
            receipt_path=supervisor_receipt_path,
            receipt=supervisor_receipt,
            cas_root=result_path.parent / "custody-cas",
        )
        root_exit_code = supervisor_receipt.get("root_exit_code")
        if not isinstance(root_exit_code, int):
            root_exit_code = (
                int(supervisor_returncode)
                if supervisor_returncode != 0
                else 2
            )
        completed = subprocess.CompletedProcess(
            execution_command, int(root_exit_code)
        )
        custody_session.mark_quiescent()
        _replay_transcript(stdout_path, sys.stdout)
        _replay_transcript(stderr_path, sys.stderr)
        result["command_returncode"] = int(completed.returncode)
        process_supervisor = context["process_supervisor"]
        assert isinstance(process_supervisor, dict)
        process_supervisor.update(
            {
                "receipt": supervisor_receipt,
                "receipt_file": _file_identity(supervisor_receipt_path),
                "event_artifact": supervisor_event_artifact,
                "supervisor_returncode": int(supervisor_returncode),
                "run_s": supervisor_run_s,
            }
        )
        transcript = {
            "stdout": _transcript_identity(stdout_path),
            "stderr": _transcript_identity(stderr_path),
        }
        transcript["identity_sha256"] = hashlib.sha256(
            json.dumps(transcript, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        if int(completed.returncode) == 0 and _requires_structured_test_counts(
            envelope
        ):
            if not any(
                isinstance(value, Mapping)
                and value.get("structured_test_output") is True
                for key, value in transcript.items()
                if key in {"stdout", "stderr"}
            ):
                raise ValueError(
                    "successful test command produced no structured test-count authority"
                )
        context["command_transcript"] = transcript
        custody_session.mark_verifying()
        post_source = _git_snapshot(effective_cwd, execution_env)
        overlay_post = [_file_identity(path) for path in overlay_paths]
        executable_post = _executable_identity(Path(exact[0]))
        payload_executable_post = _payload_executable_identity(envelope, exact)
        environment_executables_post = _execution_environment_executable_identities(
            execution_env, cwd=cwd
        )
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
        capture_verification = toolchain_capture.verify_capture(
            capture_ref,
            workers=plan.inventory_hash_workers,
            cas_root=result_path.parent / "custody-cas",
        )
        environment_post = _execution_environment_authority(
            execution_env,
            applied_cargo_policies=applied_cargo_policies,
            fingerprint_key=environment_fingerprint_key,
            contract=environment_contract,
        )
        custody_authorities_post = [
            _file_identity(path) for path in custody_authority_paths
        ]
        platform_process_images_post = process_image_capture.revalidate_images(
            platform_process_images_pre
        )
        custody_session.drain()
        session_receipt = custody_session.receipt()
        live_custody_receipt = session_receipt["live_input_custody"]
        child_custody_receipt = session_receipt["child_process_custody"]
        assert isinstance(live_custody_receipt, dict)
        assert isinstance(child_custody_receipt, dict)
        context["execution_custody_session"] = {
            "schema": session_receipt["schema"],
            "state": session_receipt["state"],
            "lifecycle": session_receipt["lifecycle"],
        }
        context["live_input_custody"] = _publish_live_custody_receipt(
            live_custody_receipt, cas_root=result_path.parent / "custody-cas"
        )
        child_process_custody = context["child_process_custody"]
        assert isinstance(child_process_custody, dict)
        child_process_custody["receipt"] = child_custody_receipt
        source_identical = pre_source == post_source
        executable_identical = executable_pre == executable_post
        payload_executable_identical = payload_executable_pre == payload_executable_post
        guarded_exec_identical = guarded_exec_pre == guarded_exec_post
        delegated_identical = delegated_pre == delegated_post
        toolchains_identical = capture_verification.get("stable") is True
        environment_pre_container = context["execution_environment"]
        assert isinstance(environment_pre_container, dict)
        environment_pre = environment_pre_container["prelaunch"]
        environment_identical = environment_pre == environment_post
        environment_executables_identical = (
            environment_executables_pre == environment_executables_post
        )
        custody_authorities_identical = (
            custody_authorities_pre == custody_authorities_post
        )
        platform_process_images_identical = (
            platform_process_images_pre == platform_process_images_post
        )
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
        if not payload_executable_identical:
            ineligible_reasons.append("payload-command-executable-changed")
        if not guarded_exec_identical:
            ineligible_reasons.append("guarded-exec-changed")
        if not delegated_identical:
            ineligible_reasons.append("delegated-command-executable-changed")
        if not toolchains_identical:
            ineligible_reasons.append("toolchain-frozen-manifest-changed")
        if not environment_identical:
            ineligible_reasons.append("execution-environment-changed")
        if not environment_executables_identical:
            ineligible_reasons.append("execution-environment-executable-changed")
        if not custody_authorities_identical:
            ineligible_reasons.append("execution-custody-authority-changed")
        if not platform_process_images_identical:
            ineligible_reasons.append("platform-process-image-changed")
        if not all(
            _content_identity_available(identity)
            for identity in custody_authorities_post
        ):
            ineligible_reasons.append("execution-custody-authority-unavailable")
        if live_custody_receipt.get("stable") is not True:
            if live_custody_receipt.get("events"):
                ineligible_reasons.append("transient-input-mutation")
            if live_custody_receipt.get("errors"):
                ineligible_reasons.append("live-input-monitor-incomplete")
        if child_custody_receipt.get("broker_complete") is not True:
            ineligible_reasons.append("child-custody-broker-incomplete")
        if supervisor_receipt.get("complete") is not True:
            ineligible_reasons.append("native-process-supervision-incomplete")
        ineligible_reasons.extend(
            _python_editable_ineligible_reasons(
                proof_python,
                source_snapshot=pre_source,
            )
        )
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
        if payload_executable_pre is not None:
            payload_executable = context["payload_command_executable"]
            assert isinstance(payload_executable, dict)
            payload_executable.update(
                {
                    "postcompletion": payload_executable_post,
                    "identical": payload_executable_identical,
                }
            )
        if guarded_exec_pre is not None:
            guarded_exec = context["guarded_exec"]
            assert isinstance(guarded_exec, dict)
            guarded_exec.update(
                {
                    "postcompletion": guarded_exec_post,
                    "identical": guarded_exec_identical,
                }
            )
        if delegated_pre is not None:
            delegated_executable = context["delegated_command_executable"]
            assert isinstance(delegated_executable, dict)
            delegated_executable.update(
                {"postcompletion": delegated_post, "identical": delegated_identical}
            )
        toolchain_custody = context["toolchain_custody"]
        assert isinstance(toolchain_custody, dict)
        toolchain_custody.update(
            {
                "verification_identity_sha256": capture_verification.get(
                    "identity_sha256"
                ),
                "identical": toolchains_identical,
            }
        )
        capture_context = context["toolchain_capture"]
        assert isinstance(capture_context, dict)
        capture_context["verification"] = capture_verification
        environment_pre_container.update(
            {
                "postcompletion_identity_sha256": environment_post.get(
                    "identity_sha256"
                ),
                "identical": environment_identical,
            }
        )
        executable_inputs = environment_pre_container["executable_inputs"]
        assert isinstance(executable_inputs, dict)
        executable_inputs.update(
            {
                "postcompletion_sha256": _canonical_payload_sha256(
                    environment_executables_post
                ),
                "identical": environment_executables_identical,
            }
        )
        custody_authorities = context["custody_authorities"]
        assert isinstance(custody_authorities, dict)
        custody_authorities.update(
            {
                "postcompletion_sha256": _canonical_payload_sha256(
                    custody_authorities_post
                ),
                "identical": custody_authorities_identical,
            }
        )
        platform_process_custody = context["platform_process_custody"]
        assert isinstance(platform_process_custody, dict)
        platform_process_custody.update(
            {
                "postcompletion_sha256": _canonical_payload_sha256(
                    platform_process_images_post
                ),
                "identical": platform_process_images_identical,
            }
        )
        telemetry = capture_context["telemetry"]
        assert isinstance(telemetry, dict)
        # Reserve the fixed-width custody digest before measuring so telemetry
        # reports the final serialized context size, not a pre-digest estimate.
        context["execution_custody_sha256"] = "0" * 64
        for _iteration in range(2):
            telemetry["receipt_context_bytes"] = len(
                json.dumps(context, sort_keys=True, separators=(",", ":")).encode()
            )
        if int(telemetry["receipt_context_bytes"]) > 64 * 1024:
            raise ValueError("compact proof receipt context exceeds 64 KiB")
        context["execution_custody_sha256"] = execution_custody_sha256(
            context,
            run_id=run_id,
            returncode=int(completed.returncode),
        )
        result["phase"] = "complete"
        _atomic_json(result_path, result)
        return int(completed.returncode)
    except BaseException as exc:
        if custody_session is not None and custody_session.state != "DRAINED":
            try:
                custody_session.__exit__(type(exc), exc, exc.__traceback__)
            except BaseException as cleanup_exc:
                result["custody_cleanup_error"] = (
                    f"{type(cleanup_exc).__name__}: {cleanup_exc}"
                )
        result.update(
            {
                "phase": "failed",
                "error": f"{type(exc).__name__}: {exc}",
            }
        )
        _atomic_json(result_path, result)
        print(
            f"proof command envelope failed: {type(exc).__name__}: {exc}",
            file=sys.stderr,
        )
        return 2


def _main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", required=True)
    args = parser.parse_args(argv)
    return execute_guarded_request(Path(args.request))


if __name__ == "__main__":
    raise SystemExit(_main())
