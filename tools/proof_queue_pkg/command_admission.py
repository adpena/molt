"""Proof-command parsing, registration, and admission authority."""

from __future__ import annotations

from dataclasses import dataclass
import functools
import os
from pathlib import Path
import re
from typing import Literal, Mapping, Sequence

from molt.exact_json import canonical_json_bytes
from molt.rust_toolchain import canonical_rust_codegen_flags, rust_flag_spans
from tools import proof_plan
from tools.command_execution import CommandExecutor
from tools.proof_queue_pkg import cargo_output_layout
from tools.proof_queue_pkg.python_payload_authority import is_molt_cli_payload


_REPO_ROOT = Path(__file__).resolve().parents[2]
_PYTHON_CUSTODY_BOOTSTRAP = Path(__file__).with_name("python_custody_bootstrap.py")

ENVELOPE_SCHEMA = "molt.proof-command-envelope.v6"
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


def _command_wrapper(
    command: Sequence[str],
) -> tuple[dict[str, object], list[str]] | None:
    """Model one environment wrapper using its actual argument-parser authority."""
    payload = list(command)
    if not payload:
        return None
    transport_prefix = None
    if _basename(payload[0]) in {"uv", "uv.exe"}:
        transport_prefix, payload = _uv_prefix_and_payload(payload)
    first = _basename(payload[0])
    if first in _PY_LAUNCHERS:
        offset = 2 if len(payload) > 1 and _PY_SELECTOR.fullmatch(payload[1]) else 1
        payload = [payload[0], *payload[offset:]]
    elif not _PYTHON_COMMAND.fullmatch(first):
        return None
    invocation = parse_python_invocation(payload)
    if invocation.mode == "module":
        name = {"tools.venv_exec": "venv", "tools.uv_project_env": "uv-project"}.get(
            str(invocation.target)
        )
    elif invocation.mode == "script":
        # Model only the repository-owned wrapper actually selected by Python.
        # Resolving components covers equivalent spellings without treating a
        # nonexistent case alias (on case-sensitive hosts) as executable code.
        candidate = Path(str(invocation.target))
        if not candidate.is_absolute():
            candidate = _REPO_ROOT / candidate
        try:
            selected = candidate.resolve(strict=True)
        except (OSError, RuntimeError):
            return None
        name = next(
            (
                kind
                for filename, kind in (
                    ("venv_exec.py", "venv"),
                    ("uv_project_env.py", "uv-project"),
                )
                if selected == (_REPO_ROOT / "tools" / filename).resolve(strict=True)
            ),
            None,
        )
    else:
        return None
    if name is None:
        return None
    if transport_prefix is not None:
        allowed = {
            "run",
            "--active",
            "--project",
            ".",
            "--python",
            "3.12",
            "--no-sync",
            "--no-config",
            "--offline",
        }
        if (
            any(value not in allowed for value in transport_prefix[1:])
            or not {"--active", "--no-sync", "--no-config"}.issubset(transport_prefix)
            or _uv_option_values(transport_prefix, "--project") != ["."]
            or _uv_option_values(transport_prefix, "--python") != ["3.12"]
        ):
            raise ValueError(
                "wrapper uv transport requires the active no-sync/no-config project contract"
            )
    from tools import venv_exec, uv_project_env

    authority = venv_exec if name == "venv" else uv_project_env
    try:
        options = authority.argument_parser().parse_args(list(invocation.arguments))
    except SystemExit as exc:
        if exc.code == 0:
            return None  # A terminal help query executes no wrapped payload.
        raise ValueError(f"invalid {name} wrapper options") from exc
    wrapped = list(options.command)
    if wrapped[:1] == ["--"]:
        wrapped = wrapped[1:]
    if name == "uv-project" and options.print_env:
        if wrapped:
            raise ValueError(
                "uv-project --print-env with execution is not a modeled proof wrapper"
            )
        return None
    if not wrapped:
        raise ValueError(f"{name} wrapper requires a payload command")
    descriptor = (
        {"kind": name, "venv": options.venv}
        if name == "venv"
        else {
            "kind": name,
            "python": options.python,
            "purpose": options.purpose,
            "venv": options.venv,
        }
    )
    return descriptor, wrapped


def _command_entrypoint(
    argv: Sequence[str], *, _wrapper_depth: int = 0
) -> tuple[str, str] | None:
    """Return the stable program entrypoint whose plan authority cannot drift.

    Arguments are deliberately excluded.  If an argv is close enough to execute
    the same Python/Node program as a proof-plan command, it must be an exact plan
    command; otherwise changing one selector could silently discard that
    command's declared toolchain closure.
    """
    if not argv:
        return None
    modeled = _command_wrapper(argv)
    if modeled is not None:
        if _wrapper_depth >= 1:
            raise ValueError("command wrappers are limited to one typed layer")
        return _command_entrypoint(modeled[1], _wrapper_depth=_wrapper_depth + 1)
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
        invocation = parse_python_invocation([payload[0], *payload[python_index:]])
        if invocation.mode == "module":
            module = str(invocation.target)
            if module == "tools.guarded_exec":
                return None
            is_cli = is_molt_cli_payload("module", module, repo_root=_REPO_ROOT)
            entrypoint = ("python-module", module.casefold())
        elif invocation.mode == "script":
            target = str(invocation.target)
            if _basename(target) == "guarded_exec.py":
                return None
            normalized = _normalized_entrypoint_target(target)
            is_cli = is_molt_cli_payload(
                "script", str(invocation.target), repo_root=_REPO_ROOT
            )
            entrypoint = ("python-script", normalized)
        else:
            return None
        if is_cli and invocation.arguments:
            command = invocation.arguments[0]
            if not command.startswith("-"):
                return ("python-cli-command", f"molt.cli:{command}".casefold())
        return entrypoint
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
        # Selected SDK policies are declared by their owning command. A bare
        # executable spelling carries no SDK selection and infers only native policy.
        if (
            policy.identity_kind != "executable"
            or policy.data.get("wasi_sdk_tool") is not None
        ):
            continue
        executable = str(policy.data.get("executable") or policy.name)
        if executable == "{python}":
            continue
        for basename in sorted(_executable_registry_names(executable)):
            prior = policy_executables.get(basename)
            if prior is not None and prior != policy.name:
                raise ValueError(
                    f"proof plan executable {basename!r} has ambiguous toolchain policies"
                )
            policy_executables[basename] = policy.name
    named: dict[tuple[str, ...], dict[str, object]] = {}
    named_entrypoints: dict[tuple[str, str], list[str]] = {}
    for lane in plan.named_lanes:
        argv = tuple(lane.argv)
        named[argv] = {
            "id": lane.id,
            "toolchains": tuple(lane.toolchains),
            "cargo_native_c_units": proof_plan.cargo_native_c_units(lane.data),
        }
        entrypoint = _command_entrypoint(argv)
        if entrypoint is not None:
            entrypoints.setdefault(entrypoint, []).append(lane.id)
            entrypoint_variants.setdefault(entrypoint, set()).add(argv)
            named_entrypoints.setdefault(entrypoint, []).append(lane.id)
    for command in plan.commands:
        argv = tuple(str(value) for value in command.argv)
        declared = tuple(command.toolchains)
        existing = exact.get(argv)
        if existing is None:
            exact[argv] = {
                "ids": [command.id],
                "toolchains": declared,
                "cargo_native_c_units": proof_plan.cargo_native_c_units(command.data),
            }
        else:
            if existing["toolchains"] != declared or existing[
                "cargo_native_c_units"
            ] != proof_plan.cargo_native_c_units(command.data):
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
        "named": named,
        "named_entrypoints": named_entrypoints,
        "console_tools": {
            name: tuple(sorted(toolchains))
            for name, toolchains in sorted(console_tools.items())
        },
        "policy_executables": policy_executables,
        "plan": plan,
        "entrypoints": entrypoints,
        "entrypoint_variants": entrypoint_variants,
    }


def _registered_console_toolchains(name: str) -> tuple[str, ...] | None:
    registry = _proof_command_registry()
    console_tools = registry["console_tools"]
    assert isinstance(console_tools, dict)
    value = console_tools.get(name)
    return tuple(value) if isinstance(value, tuple) else None


def _toolchain_dependency_closure(names: Sequence[str]) -> list[str]:
    registry = _proof_command_registry()
    plan = registry["plan"]
    if not isinstance(plan, proof_plan.ProofPlan):
        raise TypeError("proof command registry has no canonical proof plan")
    return list(plan.toolchain_closure(str(name) for name in names))


def _command_registration(
    argv: Sequence[str],
    *,
    has_python: bool,
    has_uv: bool,
    typed_python: Mapping[str, object] | None = None,
    execution_argv: Sequence[str] | None = None,
) -> tuple[str, list[str], list[str], list[str]]:
    registry = _proof_command_registry()
    exact = registry["exact"]
    assert isinstance(exact, dict)
    exact_match = exact.get(tuple(str(value) for value in argv))
    if isinstance(exact_match, dict):
        command_ids = exact_match["ids"]
        declared = exact_match["toolchains"]
        assert isinstance(command_ids, list) and isinstance(declared, tuple)
        toolchains = _toolchain_dependency_closure([str(name) for name in declared])
        if not toolchains:
            raise ValueError(
                f"proof-plan commands {command_ids!r} have no toolchain authority"
            )
        return (
            "proof-plan",
            toolchains,
            [str(command_id) for command_id in command_ids],
            list(exact_match["cargo_native_c_units"]),
        )
    named = registry["named"]
    assert isinstance(named, dict)
    lane_match = named.get(tuple(str(value) for value in argv))
    if isinstance(lane_match, dict):
        declared = lane_match["toolchains"]
        assert isinstance(declared, tuple)
        toolchains = _toolchain_dependency_closure([str(name) for name in declared])
        if not toolchains:
            raise ValueError(
                f"named lane {lane_match['id']!r} has no toolchain authority"
            )
        return (
            "named-lane",
            toolchains,
            [str(lane_match["id"])],
            list(lane_match["cargo_native_c_units"]),
        )

    if typed_python is not None and typed_python.get("family") == "prepared-named-lane":
        lane_id = str(typed_python["lane_id"])
        plan = registry["plan"]
        assert isinstance(plan, proof_plan.ProofPlan)
        return (
            "named-lane",
            _toolchain_dependency_closure(plan.named_lane(lane_id).toolchains),
            [lane_id],
            list(proof_plan.cargo_native_c_units(plan.named_lane(lane_id).data)),
        )

    entrypoint = _command_entrypoint(argv)
    named_entrypoints = registry["named_entrypoints"]
    assert isinstance(named_entrypoints, dict)
    lane_near_matches = named_entrypoints.get(entrypoint)
    if isinstance(lane_near_matches, list):
        # A program registered as a named lane spawns processes by design; an
        # argv that differs from every registered lane must not silently
        # degrade into a leaf with children forbidden.
        raise ValueError(
            "named-lane entrypoint argv must match its registered command exactly; "
            f"near-match would discard the toolchain closure of {lane_near_matches!r}"
        )
    registered_entrypoints = registry["entrypoints"]
    assert isinstance(registered_entrypoints, dict)
    near_matches = registered_entrypoints.get(entrypoint)
    registered_variants = registry["entrypoint_variants"]
    assert isinstance(registered_variants, dict)
    variants = registered_variants.get(entrypoint)
    if (
        isinstance(near_matches, list)
        and isinstance(variants, set)
        and (
            len(variants) == 1
            or (entrypoint is not None and entrypoint[0] == "python-cli-command")
        )
    ):
        raise ValueError(
            "proof-plan entrypoint argv must match its registered command exactly; "
            f"near-match would discard toolchain authority for {near_matches!r}"
        )

    if execution_argv is not None:
        argv = execution_argv

    toolchains: list[str] = []

    def add(name: str) -> None:
        if name not in toolchains:
            toolchains.append(name)

    if has_python:
        add("python")
        if typed_python is not None:
            add("source-extension")
            return (
                "typed-python-family",
                _toolchain_dependency_closure(toolchains),
                [],
                [],
            )
        if has_uv:
            add("uv")
        if argv and _basename(argv[0]) in {"uv", "uv.exe"}:
            _prefix, payload = _uv_prefix_and_payload(argv)
            console = _registered_console_toolchains(_basename(payload[0]))
            if console is not None:
                for name in console:
                    add(name)
        return "python", _toolchain_dependency_closure(toolchains), [], []

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
        invocation = parse_cargo_invocation(argv)
        if invocation.subcommand == "deny":
            add("cargo-deny")
        elif invocation.subcommand == "audit":
            add("cargo-audit")
    return "toolchain", _toolchain_dependency_closure(toolchains), [], []


_CARGO_LEAF_SUBCOMMANDS = frozenset(
    {
        "help",
        "locate-project",
        "metadata",
        "pkgid",
        "search",
        "tree",
        "version",
    }
)
_CARGO_QUERY_FLAGS = frozenset({"--help", "-h", "--version", "-V"})
ProofKind = Literal["build", "test-execution", "query", "command"]
CargoOutputLifetime = Literal["retain", "terminal-success"]


_CARGO_OPTIONS_WITH_VALUES = frozenset(
    {
        "-p",
        "--package",
        "--manifest-path",
        "--target",
        "--target-dir",
        "--features",
        "-F",
        "--profile",
        "--jobs",
        "-j",
        "--config",
        "--message-format",
        "--color",
        "--bin",
        "--example",
        "--test",
        "--bench",
        "--exclude",
        "--lockfile-path",
        "--artifact-dir",
        "--crate-type",
        "-C",
        "-Z",
    }
)
_CARGO_SHORT_VALUE_OPTIONS = {
    "-p": "--package",
    "-F": "--features",
    "-j": "--jobs",
    "-C": "-C",
    "-Z": "-Z",
}


_LIBTEST_OPTIONS_WITH_VALUES = frozenset(
    {
        "--logfile",
        "--test-threads",
        "--skip",
        "--color",
        "--format",
        "-Z",
        "--shuffle-seed",
    }
)


def _libtest_is_query(arguments: Sequence[str]) -> bool:
    """Recognize harness queries without treating filter operands as flags.

    Value-taking options follow Rust 1.99 library/test/src/cli.rs. In
    particular, --report-time and --ensure-time are flags, not operands.
    """
    index = 0
    while index < len(arguments):
        value = str(arguments[index])
        if value == "--":
            break
        name, equal, _operand = value.partition("=")
        if name in _LIBTEST_OPTIONS_WITH_VALUES:
            index += 1 if equal else 2
            continue
        if value.startswith("-Z") and len(value) > 2:
            index += 1
            continue
        if value in {"--help", "--list", "-h"}:
            return True
        # getopts accepts combined short flags; libtest defines only h/q.
        if value.startswith("-") and not value.startswith("--"):
            short_flags = value[1:]
            if "h" in short_flags and set(short_flags) <= {"h", "q"}:
                return True
        index += 1
    return False


@dataclass(frozen=True)
class CargoInvocation:
    """Cargo-owned tokens, distinct from operands and forwarded harness argv."""

    toolchain_selector: str | None
    subcommand: str | None
    flags: frozenset[str]
    option_values: tuple[tuple[str, str], ...]
    positionals: tuple[str, ...]
    forwarded: tuple[str, ...]

    @property
    def crate_types(self) -> tuple[str, ...] | None:
        values = [value for name, value in self.option_values if name == "--crate-type"]
        if not values:
            return None
        if self.subcommand != "rustc" or len(values) != 1:
            raise ValueError("Cargo crate types require one cargo rustc selector")
        return _rust_crate_types(values[0])

    @property
    def forwarded_crate_types(self) -> tuple[str, ...]:
        return rustc_crate_types(self.forwarded)

    @property
    def requires_documenter(self) -> bool:
        return self.subcommand in {"test", "doc", "rustdoc"} and not self.is_cargo_query

    @property
    def is_cargo_query(self) -> bool:
        if self.subcommand in _CARGO_LEAF_SUBCOMMANDS:
            return True
        # External subcommands receive their own options and may spawn tools
        # even for help; do not give them Cargo's built-in leaf semantics.
        return self.subcommand in {
            None,
            "test",
            "bench",
            "build",
            "check",
            "rustc",
            "run",
            "doc",
            "rustdoc",
        } and bool(self.flags & _CARGO_QUERY_FLAGS)

    @property
    def proof_kind(self) -> ProofKind:
        if self.is_cargo_query:
            return "query"
        # Cargo run_tests/run_benches compile first and return before executing
        # any harness when their own --no-run option is set (Cargo 1.99).
        if self.subcommand in {"test", "bench"} and "--no-run" in self.flags:
            return "build"
        if self.subcommand in {"test", "bench"} and _libtest_is_query(self.forwarded):
            return "query"
        if self.subcommand == "test":
            return "test-execution"
        if self.subcommand in {"build", "check", "rustc"}:
            return "build"
        return "command"


def parse_cargo_invocation(argv: Sequence[str]) -> CargoInvocation:
    """Parse operation boundaries once for proof admission and test policy.

    Option operands are consumed before flag/positional classification. In
    particular, a value named test, pytest, or --no-run is never an operation;
    tokens after -- belong to libtest/rustc, not Cargo's compile-only switch.
    Only the first argument can select a Rustup toolchain with +toolchain.
    """
    if not argv or _basename(str(argv[0])) not in {"cargo", "cargo.exe"}:
        raise ValueError("Cargo invocation has no Cargo executable")
    command = [str(value) for value in argv]
    toolchain_selector = None
    subcommand = None
    flags: set[str] = set()
    values: list[tuple[str, str]] = []
    positionals: list[str] = []
    forwarded: tuple[str, ...] = ()
    index = 1
    if index < len(command) and command[index].startswith("+"):
        toolchain_selector = command[index][1:]
        if not toolchain_selector:
            raise ValueError("Cargo +toolchain selector requires a non-empty toolchain")
        index += 1
    while index < len(command):
        value = command[index]
        if value == "--":
            forwarded = tuple(command[index + 1 :])
            break
        name, equal, operand = value.partition("=")
        if name in _CARGO_OPTIONS_WITH_VALUES:
            if not equal:
                index += 1
                if index >= len(command):
                    raise ValueError(f"Cargo {name} requires a value")
                operand = command[index]
            values.append((_CARGO_SHORT_VALUE_OPTIONS.get(name, name), operand))
        elif any(
            value.startswith(short) and len(value) > 2
            for short in _CARGO_SHORT_VALUE_OPTIONS
        ):
            values.append((_CARGO_SHORT_VALUE_OPTIONS[value[:2]], value[2:]))
        elif value.startswith("-"):
            flags.add(value)
        elif subcommand is None:
            subcommand = {"b": "build", "c": "check", "t": "test", "r": "run"}.get(
                value, value
            )
        else:
            positionals.append(value)
        index += 1
    return CargoInvocation(
        toolchain_selector,
        subcommand,
        frozenset(flags),
        tuple(values),
        tuple(positionals),
        forwarded,
    )


def _rust_crate_types(value: str) -> tuple[str, ...]:
    kinds = tuple(value.split(","))
    if not kinds or any(
        kind not in {"bin", "lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}
        for kind in kinds
    ):
        raise ValueError("Rust crate-type selection is invalid")
    return kinds


def rust_link_arguments(arguments: Sequence[str]) -> tuple[str, ...]:
    """Project the existing compiler-selection arguments, consuming operands."""
    result: list[str] = []
    for span in rust_flag_spans(arguments):
        value = arguments[span.start]
        if span.codegen is not None:
            # This projection feeds real rustc and relative-path provenance;
            # retain the observed spelling, not only its semantic equivalent.
            result.extend(arguments[span.start : span.stop])
        elif value == "--":
            break
        elif span.option in {"--crate-type", "--sysroot"}:
            result.extend(arguments[span.start : span.stop])
    return tuple(result)


def rustc_crate_types(arguments: Sequence[str]) -> tuple[str, ...]:
    kinds: list[str] = []
    values = iter(canonical_rust_codegen_flags(rust_link_arguments(arguments)))
    for argument in values:
        if argument in {"-C", "--sysroot"}:
            next(values)
        elif argument == "--crate-type":
            kinds.extend(_rust_crate_types(next(values)))
        elif argument.startswith("--crate-type="):
            kinds.extend(_rust_crate_types(argument.partition("=")[2]))
    return tuple(kinds)


def rust_link_artifact_selection(
    argv: Sequence[str],
    *,
    cargo: bool,
    cargo_invocation: CargoInvocation | None,
    unit: str,
    manifest_crate_types: Sequence[str] | None = None,
) -> dict[str, object]:
    """Bind explicit output kinds without inventing an executable for libraries.

    Unspecified Cargo outputs retain the existing std capability probe. A
    Cargo-level override replaces the manifest base; forwarded rustc crate
    types require the retained selected manifest base and remain additive.
    Direct rustc crate types replace rustc's default bin.
    """
    if unit == "host-proc-macro":
        if manifest_crate_types is not None:
            raise ValueError(
                "host proc-macro probe cannot inherit target manifest kinds"
            )
        return {
            "cargo_crate_types": None,
            "rustc_crate_types": ["proc-macro"],
            "manifest_crate_types": None,
            "link_required": True,
        }
    # Admission owns the role before executable binding changes its basename.
    invocation = cargo_invocation
    if invocation is not None and not cargo:
        raise ValueError("Cargo artifact selection requires a Cargo owner")
    override = None if invocation is None else invocation.crate_types
    forwarded = (
        invocation.forwarded if invocation is not None else (() if cargo else argv[1:])
    )
    kinds = rustc_crate_types(forwarded)
    requires_manifest = cargo and override is None and bool(kinds)
    if requires_manifest != (manifest_crate_types is not None):
        raise ValueError(
            "forwarded Cargo crate types require the selected manifest base"
        )
    base = (
        None
        if manifest_crate_types is None
        else _rust_crate_types(",".join(manifest_crate_types))
    )
    effective = (
        *((override or base or ("bin",)) if cargo else (() if kinds else ("bin",))),
        *kinds,
    )
    return {
        "cargo_crate_types": None if override is None else list(override),
        "manifest_crate_types": None if base is None else list(base),
        "rustc_crate_types": list(kinds),
        "link_required": any(
            kind not in {"lib", "rlib", "staticlib"} for kind in effective
        ),
    }


def cargo_invocation_for_envelope(
    envelope: Mapping[str, object],
) -> CargoInvocation | None:
    """Distinguish a Cargo payload from a Python driver declaring Cargo children."""
    delegated = envelope.get("delegated")
    cargo = delegated if isinstance(delegated, Mapping) else envelope
    argv = cargo.get("argv")
    if not isinstance(argv, list) or not argv:
        raise ValueError("Cargo policy requires admitted executable argv")
    if _basename(str(argv[0])) in {"cargo", "cargo.exe"}:
        return parse_cargo_invocation(argv)
    if "cargo" in envelope.get("toolchains", []) and isinstance(
        cargo.get("python"), Mapping
    ):
        return None
    raise ValueError("Cargo policy requires a Cargo payload or declared Python driver")


def command_proof_kind(envelope: Mapping[str, object]) -> ProofKind:
    """Derive evidence obligations from the admitted execution payload."""
    delegated = envelope.get("delegated")
    if isinstance(delegated, Mapping):
        return command_proof_kind(delegated)
    argv = [str(value) for value in envelope["argv"]]  # type: ignore[index]
    python = envelope.get("python")
    if isinstance(python, Mapping):
        invocation = parse_python_invocation(_python_invocation_argv(argv, python))
        if invocation.mode == "module" and invocation.target in {"pytest", "py.test"}:
            return "test-execution"
        return "command"
    if argv and _basename(argv[0]) in {"cargo", "cargo.exe"}:
        return parse_cargo_invocation(argv).proof_kind
    return "command"


_TOOLCHAIN_LEAF_PROBES = frozenset({"--help", "-h", "--version", "-V", "-vV"})


def _registered_toolchain_descendants(argv: Sequence[str]) -> str:
    """Classify exact capability probes separately from process-spawning tools."""
    arguments = [str(value) for value in argv[1:]]
    if not arguments:
        return "forbidden"
    executable = _basename(str(argv[0]))
    if executable in {"cargo", "cargo.exe"}:
        invocation = parse_cargo_invocation(argv)
        return (
            "forbidden"
            if invocation.subcommand is None or invocation.is_cargo_query
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


def _execution_source_paths(envelope: Mapping[str, object], *, cwd: Path) -> Path:
    """The effective source directory a uv envelope executes in (inside cwd)."""
    python = envelope.get("python")
    if not isinstance(python, Mapping) or python.get("kind") not in {
        "uv",
        "uv-console-script",
    }:
        return cwd.resolve(strict=True)
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
    return effective


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
) -> tuple[list[str], Path]:
    python = envelope.get("python")
    if not isinstance(python, Mapping) or python.get("kind") not in {
        "uv",
        "uv-console-script",
    }:
        return [], cwd.resolve(strict=True)
    prefix = python.get("prefix")
    if not isinstance(prefix, list):
        raise ValueError("uv command envelope has no prefix")
    exact_prefix = [str(value) for value in prefix]
    effective = _execution_source_paths(envelope, cwd=cwd)
    replacements = {
        "--directory": [effective] if _uv_option_values(prefix, "--directory") else [],
        "--project": [effective] if _uv_option_values(prefix, "--project") else [],
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
    return exact_prefix, effective


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
    selector = (
        first in _PY_LAUNCHERS
        and len(payload) > 1
        and _PY_SELECTOR.fullmatch(payload[1]) is not None
    )
    if not (_PYTHON_COMMAND.fullmatch(first) or first in _PY_LAUNCHERS):
        return None
    invocation = parse_python_invocation(
        [payload[0], *payload[2:]] if selector else payload
    )
    # Only the interpreter's target can be this delegation authority. Payload
    # arguments, option operands and command strings keep their ordinary meaning.
    after_target = len(payload) - len(invocation.arguments)
    if invocation.mode == "module" and invocation.target == "tools.guarded_exec":
        mode = "module"
        first_target = after_target - (
            1 if payload[after_target - 1] == "-mtools.guarded_exec" else 2
        )
        target_indices = list(range(offset + first_target, offset + after_target))
    elif (
        invocation.mode == "script"
        and _basename(invocation.target or "") == "guarded_exec.py"
    ):
        mode = "script"
        target_indices = [offset + after_target - 1]
    else:
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


def _registered_named_python_invocation(lane: proof_plan.NamedLane) -> PythonInvocation:
    argv = list(lane.argv)
    if _basename(argv[0]) in {"uv", "uv.exe"}:
        _prefix, argv = _uv_prefix_and_payload(argv)
    if not _PYTHON_COMMAND.fullmatch(_basename(argv[0])):
        raise ValueError(f"named lane {lane.id!r} has no Python payload")
    return parse_python_invocation(argv)


def prepared_named_lane_command(lane_id: str, executable: Path) -> list[str]:
    """Bind the registered payload, without changing arguments, to prepared Python."""
    lane = proof_plan.ProofPlan.load().named_lane(lane_id)
    invocation = _registered_named_python_invocation(lane)
    if invocation.mode != "script" or invocation.interpreter_options:
        raise ValueError(f"named lane {lane_id!r} is not a plain Python script")
    assert invocation.target is not None
    return [str(executable), "-P", invocation.target, *invocation.arguments]


def _locked_python_environment_root(executable: str) -> Path:
    from molt.cli.source_build_environment_schema import (
        SOURCE_BUILD_ENVIRONMENT_MANIFEST,
    )
    from molt.cli.source_build_environment import _source_build_custody_root

    selected = Path(executable)
    if not selected.is_absolute() or not selected.is_file():
        raise ValueError(
            "prepared proof requires an absolute available locked interpreter"
        )
    selected = Path(os.path.abspath(selected))
    environment_root = selected.parent.parent
    if (
        selected.parent.name != ("Scripts" if os.name == "nt" else "bin")
        or re.fullmatch(r"[0-9a-f]{64}", environment_root.name) is None
        or environment_root.parent.resolve()
        != _source_build_custody_root(_REPO_ROOT).resolve()
        or not (environment_root / SOURCE_BUILD_ENVIRONMENT_MANIFEST).is_file()
    ):
        raise ValueError(
            "prepared proof requires a content-addressed locked source-build interpreter"
        )
    return environment_root


def _typed_python_command_family(
    argv: Sequence[str],
    python: Mapping[str, object],
    invocation: PythonInvocation,
) -> dict[str, object] | None:
    """Admit prepared payloads through their existing plan/CLI authorities."""
    if python.get("kind") == "direct" and invocation.mode == "script":
        plan = _proof_command_registry()["plan"]
        assert isinstance(plan, proof_plan.ProofPlan)
        matches: list[tuple[proof_plan.NamedLane, PythonInvocation]] = []
        registered_entrypoint = False
        for lane in plan.named_lanes:
            if _command_entrypoint(lane.argv) != (
                "python-script",
                _normalized_entrypoint_target(str(invocation.target)),
            ):
                continue
            registered = _registered_named_python_invocation(lane)
            if registered.mode != "script" or _normalized_entrypoint_target(
                str(registered.target)
            ) != _normalized_entrypoint_target(str(invocation.target)):
                continue
            registered_entrypoint = True
            if invocation.arguments == registered.arguments:
                matches.append((lane, registered))
        if registered_entrypoint and not matches:
            raise ValueError(
                "named-lane argv must match its registered command exactly"
            )
        if len(matches) > 1:
            raise ValueError(
                "prepared named-lane payload has ambiguous registered authorities: "
                + ", ".join(lane.id for lane, _registered in matches)
            )
        if matches:
            lane, registered = matches[0]
            if (
                invocation.interpreter_options != ("-P",)
                or registered.interpreter_options
            ):
                raise ValueError(
                    "prepared named lane requires a direct locked interpreter with -P"
                )
            return {
                "family": "prepared-named-lane",
                "lane_id": lane.id,
                "environment_root": str(_locked_python_environment_root(str(argv[0]))),
            }
    if not is_molt_cli_payload(
        invocation.mode, invocation.target, repo_root=_REPO_ROOT
    ) or invocation.arguments[:2] != ("extension", "produce-set"):
        return None
    if python.get("kind") != "direct" or invocation.interpreter_options != ("-P",):
        raise ValueError(
            "source-extension producer proof requires a direct locked interpreter with -P"
        )
    from molt.cli.source_extension_invocation import SourceExtensionSetInvocation
    from molt.cli.source_extension_set_registry import (
        SourceExtensionVariant,
        load_source_extension_registry,
        source_extension_set,
        source_extension_set_expected_identity,
    )
    from molt.cli.source_extension_target import resolve_source_extension_target_plan
    from molt.target_python import _parse_target_python_version

    producer = SourceExtensionSetInvocation.from_arguments(invocation.arguments)
    if not producer.prepared:
        raise ValueError(
            "source-extension producer proof requires --prepared; provisioning "
            "and interpreter re-execution must complete before proof custody"
        )
    registry = load_source_extension_registry()
    extension_set = source_extension_set(
        producer.package,
        producer.package_version,
        producer.module_set,
        registry=registry,
    )
    target_plan = resolve_source_extension_target_plan(producer.target)
    variant = SourceExtensionVariant(
        target_python=_parse_target_python_version(producer.python_version),
        abi_tier=producer.abi_tier,
        target_triple=target_plan.target_triple,
    )
    registered_identity = source_extension_set_expected_identity(
        extension_set,
        variant=variant,
        registry=registry,
    )
    if (
        producer.expected_candidate_identity_sha256 is not None
        and producer.expected_candidate_identity_sha256 != registered_identity
    ):
        raise ValueError(
            "source-extension expected candidate identity differs from the registered target cell"
        )
    environment_root = _locked_python_environment_root(str(argv[0]))
    for label, value in (
        ("source", producer.source),
        ("build-root", producer.build_root),
    ):
        path = Path(value)
        if not path.is_absolute():
            raise ValueError(f"source-extension producer {label} must be absolute")
        resolved = path.resolve(strict=label == "source")
        if path != resolved:
            raise ValueError(
                f"source-extension producer {label} must use its canonical path, not an alias"
            )
        if label == "source" and not resolved.is_dir():
            raise ValueError("source-extension producer source must be a directory")
    return {
        "family": "source-extension-producer",
        "prepared": producer.prepared,
        "environment_root": str(environment_root),
        "target": target_plan.requested,
        "target_triple": target_plan.target_triple,
        "source": producer.source,
        "build_root": producer.build_root,
        "json": producer.json_output,
    }


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
    skip_first_line = _interpreter_flag_present(invocation.interpreter_options, "x")
    # PYTHONDONTWRITEBYTECODE is a queue-owned canonical input, but -E and -I
    # make the interpreter ignore it while site still imports the repository
    # startup adapter from custody-inventoried source. -B is the same policy
    # as an interpreter option, so no admitted payload can drop it.
    interpreter_options = list(invocation.interpreter_options)
    if not _interpreter_flag_present(interpreter_options, "B"):
        interpreter_options.insert(0, "-B")
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
        return [supervised_executable, *interpreter_options, *payload]
    return [*outer, *interpreter_options, *payload]


def _interpreter_flag_present(options: Sequence[str], flag: str) -> bool:
    """Whether a single-letter CPython flag is set in parsed interpreter options.

    Only short-option groups carry flags; -W and -X take free-text values and
    long options are named, so none of those can match a letter.
    """
    return any(
        option.startswith("-")
        and not option.startswith(("--", "-W", "-X"))
        and flag in option[1:]
        for option in options
    )


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
    submitted_argv = list(argv)
    modeled_wrapper = _command_wrapper(argv)
    wrapper = None
    if modeled_wrapper is not None:
        wrapper, argv = modeled_wrapper
        if _command_wrapper(argv) is not None:
            raise ValueError("command wrappers are limited to one typed layer")
        if _guarded_exec_invocation(argv) is not None:
            raise ValueError("command wrappers cannot delegate guarded_exec")
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

    typed_python = None
    # An exact plan command is its own authority, even when a named lane runs
    # the same program with other arguments.
    if (
        python is not None
        and tuple(map(str, submitted_argv)) not in _proof_command_registry()["exact"]
    ):
        invocation = parse_python_invocation(_python_invocation_argv(argv, python))
        typed_python = _typed_python_command_family(argv, python, invocation)
    registration_kind, toolchains, proof_plan_command_ids, native_c_units = (
        _command_registration(
            submitted_argv,
            has_python=python is not None,
            has_uv=first in {"uv", "uv.exe"},
            typed_python=typed_python,
            execution_argv=argv if wrapper is not None else None,
        )
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
    if registration_kind in {"proof-plan", "named-lane"}:
        process_closure = {
            "kind": registration_kind,
            "descendants": "declared-toolchains",
            "toolchains": list(toolchains),
        }
    elif typed_python is not None:
        process_closure = {
            "kind": "typed-python-family",
            "family": typed_python["family"],
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
        **(
            {"submitted_argv": submitted_argv, "wrapper": wrapper}
            if wrapper is not None
            else {}
        ),
        "schema": ENVELOPE_SCHEMA,
        "kind": registration_kind,
        "argv": argv,
        "python": python,
        "toolchains": toolchains,
        "proof_plan_command_ids": proof_plan_command_ids,
        "cargo_native_c_units": (
            native_c_units if delegated is None else delegated["cargo_native_c_units"]
        ),
        "guarded_exec": (
            {key: value for key, value in guarded_exec.items() if key != "nested"}
            if guarded_exec is not None
            else None
        ),
        "delegated": delegated,
        "typed_command": typed_python,
        "process_closure": process_closure,
    }


def parse_cargo_output_lifetime(value: object) -> CargoOutputLifetime:
    if isinstance(value, str):
        if value == "retain":
            return "retain"
        if value == "terminal-success":
            return "terminal-success"
    raise ValueError("cargo_output_lifetime must be retain or terminal-success")


def validated_cargo_output_lifetime(
    envelope: Mapping[str, object],
) -> CargoOutputLifetime:
    """Validate the explicit output-disposition declaration, never infer one."""
    if not isinstance(envelope, Mapping):
        raise ValueError("Cargo output lifetime requires a command envelope object")
    lifetime = parse_cargo_output_lifetime(
        envelope.get("cargo_output_lifetime", "retain")
    )
    if lifetime == "terminal-success":
        delegated = envelope.get("delegated")
        cargo = delegated if isinstance(delegated, dict) else envelope
        argv = cargo.get("argv")
        if (
            not isinstance(argv, list)
            or not argv
            or not all(isinstance(value, str) for value in argv)
            or _basename(argv[0]) not in {"cargo", "cargo.exe"}
        ):
            raise ValueError(
                "terminal-success requires an explicit Cargo check or test execution"
            )
        invocation = parse_cargo_invocation(argv)
        if (
            invocation.is_cargo_query
            or invocation.flags & {"--no-run", "--unit-graph", "--build-plan"}
            or not (
                invocation.subcommand == "check"
                or (
                    invocation.subcommand == "test"
                    and invocation.proof_kind == "test-execution"
                )
            )
        ):
            raise ValueError(
                "terminal-success requires Cargo check or actual test execution, not a query or deferred output consumer"
            )
    return lifetime


def _bind_output_root_declaration(
    envelope: dict[str, object], declaration: Mapping[str, object]
) -> None:
    # Every admitted payload provisions a native Cargo supervisor. A typed
    # non-Cargo payload can place that control-plane build without acquiring
    # Cargo payload/descendant permissions or output-disposal semantics.
    toolchains = envelope.get("toolchains")
    if not isinstance(toolchains, list) or not all(
        isinstance(name, str) for name in toolchains
    ):
        raise ValueError("Cargo output placement requires typed toolchain authority")
    if "cargo" not in toolchains:
        envelope["cargo_output_root"] = dict(declaration)
        return
    invocation = cargo_invocation_for_envelope(envelope)
    if invocation is None:
        # Declared Python drivers inherit the same leased Cargo output family.
        # Their argv is Python's; it must not be parsed as a Cargo invocation.
        envelope["cargo_output_root"] = dict(declaration)
        return
    if (
        invocation.subcommand
        not in {"check", "test", "build", "bench", "doc", "rustdoc", "rustc", "run"}
        or invocation.is_cargo_query
    ):
        raise ValueError(
            "Cargo output placement requires a parsed Cargo build or execution command"
        )
    if any(
        name in {"--target-dir", "--artifact-dir"}
        for name, _value in invocation.option_values
    ):
        raise ValueError("Cargo output command option bypasses declared placement")
    envelope["cargo_output_root"] = dict(declaration)


def envelope_for_command(
    command: Sequence[str],
    *,
    cargo_output_lifetime: str = "retain",
    cargo_output_root: str | None = None,
) -> dict[str, object]:
    """Derive the sole executable, toolchain, and child-process authority."""
    envelope = _envelope_for_command(command, typed_delegation=False)
    # Absent means retain for existing immutable envelopes and receipts.
    if cargo_output_lifetime != "retain":
        envelope["cargo_output_lifetime"] = cargo_output_lifetime
    validated_cargo_output_lifetime(envelope)
    if cargo_output_root is not None:
        _bind_output_root_declaration(
            envelope, cargo_output_layout.declare_root(cargo_output_root)
        )
    return envelope


def admission_envelope(
    command: Sequence[str],
    *,
    cargo_output_lifetime: str = "retain",
    cargo_output_root: str | None = None,
) -> dict[str, object]:
    """Persist rejected argv without fabricating any executable authority."""
    # A disposition declaration is authority, not a rejected-command receipt.
    # Validate it before either scheduling insertion path can persist a row.
    if cargo_output_lifetime != "retain" or cargo_output_root is not None:
        return envelope_for_command(
            command,
            cargo_output_lifetime=cargo_output_lifetime,
            cargo_output_root=cargo_output_root,
        )
    try:
        return envelope_for_command(
            command, cargo_output_lifetime=cargo_output_lifetime
        )
    except ValueError as exc:
        return {
            "schema": ENVELOPE_SCHEMA,
            "kind": "rejected",
            "argv": [str(value) for value in command],
            "python": None,
            "toolchains": [],
            "proof_plan_command_ids": [],
            "cargo_native_c_units": [],
            "guarded_exec": None,
            "delegated": None,
            "typed_command": None,
            "process_closure": None,
            "error": str(exc),
        }


def validate_envelope(envelope: Mapping[str, object], command: Sequence[str]) -> None:
    lifetime = validated_cargo_output_lifetime(envelope)
    root = cargo_output_layout.declared_root(envelope.get("cargo_output_root"))
    expected = envelope_for_command(command, cargo_output_lifetime=lifetime)
    if root is not None:
        _bind_output_root_declaration(expected, root)
    try:
        matches = canonical_json_bytes(dict(envelope)) == canonical_json_bytes(expected)
    except (TypeError, ValueError) as exc:
        raise ValueError("persisted proof command envelope is not exact JSON") from exc
    # Python mapping equality treats True, 1 and 1.0 as equal. Persisted typed
    # execution preconditions must retain their exact JSON representation.
    if not matches:
        raise ValueError(
            "persisted proof command envelope does not match submitted argv"
        )
