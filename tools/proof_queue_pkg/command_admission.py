"""Proof-command parsing, registration, and admission authority."""

from __future__ import annotations

from dataclasses import dataclass
import functools
import os
from pathlib import Path
import re
from typing import Mapping, Sequence

from tools import proof_plan
from tools.command_execution import CommandExecutor


_REPO_ROOT = Path(__file__).resolve().parents[2]
_PYTHON_IDENTITY_PROBE = (
    _REPO_ROOT / "tools" / "proof_queue_pkg" / "python_identity_probe.py"
)
_PYTHON_CUSTODY_BOOTSTRAP = Path(__file__).with_name("python_custody_bootstrap.py")
_PYTHON_TOOLCHAIN_LOCATOR = Path(__file__).with_name("python_toolchain_locator.py")

ENVELOPE_SCHEMA = "molt.proof-command-envelope.v3"
EXECUTION_SCHEMA = "molt.proof-command-execution.v4"
_COMMANDS = CommandExecutor.for_file(__file__)

_PYTHON_COMMAND = re.compile(r"^python(?:\d+(?:\.\d+)*)?(?:\.exe)?$", re.IGNORECASE)
_PY_LAUNCHERS = frozenset({"py", "py.exe"})
_PY_SELECTOR = re.compile(
    r"(?:-\d+(?:\.\d+)?(?:-(?:32|64))?|-V:[^\s/:]+(?:/[^\s/:]+)?)",
    re.IGNORECASE,
)


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
            return PythonInvocation(
                tuple(options), "stdin", None, tuple(values[index + 1 :])
            )
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
            module = _PYTHON_CONSOLE_MODULES.get(
                _basename(str(python["console_script"]))
            )
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
        option.startswith("-") and not option.startswith("-X") and "x" in option[1:]
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
    elif (
        registration_kind == "toolchain"
        and (descendants := _registered_toolchain_descendants(argv))
        == "declared-toolchains"
    ):
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
