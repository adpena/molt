"""Proof-command argv: the program an argv runs and the tests pytest selects.

This is the one reading of a proof command's argv (HF-179): the Python
invocation and its ``uv run`` transport, the typed wrappers, the program
entrypoint and, for a focused pytest proof, the paths pytest selects. Proof
admission and the queue's command identity read it. The proof plan does not: a
plan row declares its pytest targets as data. Every function that resolves a
path takes the checkout it reads as ``root``.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re
from typing import Sequence

from tools.proof_queue_pkg.python_payload_authority import is_molt_cli_payload


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


_PYTHON_FLAG_CHARACTERS = frozenset("bBdEhiIOPqRsStuvVx?")
_PYTHON_TERMINAL_OPTIONS = frozenset(
    {
        "-h",
        "-?",
        "--help",
        "--help-all",
        "--help-env",
        "--help-xoptions",
        "--version",
    }
)


def parse_python_invocation(argv: Sequence[str]) -> PythonInvocation:
    """Split the CPython prefix, including grouped argument-taking options.

    c/m stop parsing; W/X consume their token remainder or the next argument.
    This follows CPython 3.12's getopt and initialization boundary.
    """
    if not argv:
        raise ValueError("Python invocation has no interpreter")
    values = [str(value) for value in argv]
    options: list[str] = []
    version = False

    def payload(
        mode: str, target: str | None, arguments: Sequence[str]
    ) -> PythonInvocation:
        if version:
            return PythonInvocation(tuple(values[1:]), "terminal", None, ())
        return PythonInvocation(tuple(options), mode, target, tuple(arguments))

    index = 1
    while index < len(values):
        value = values[index]
        if value == "--":
            index += 1
            break
        if value == "-":
            return payload("stdin", None, values[index + 1 :])
        if not value.startswith("-"):
            break
        if value in _PYTHON_TERMINAL_OPTIONS:
            if value == "--version":
                version = True
                options.append(value)
                index += 1
                continue
            return PythonInvocation(tuple(values[1:]), "terminal", None, ())
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
        if value.startswith("--"):
            raise ValueError(f"unsupported Python interpreter option {value!r}")
        for offset, character in enumerate(value[1:], 1):
            if character in "cmWX":
                if offset > 1:
                    options.append(value[:offset])
                attached = value[offset + 1 :]
                if attached:
                    option_value = attached
                else:
                    index += 1
                    if index >= len(values):
                        label = {
                            "c": "a command",
                            "m": "a module",
                            "W": "a value",
                            "X": "a value",
                        }[character]
                        raise ValueError(f"Python -{character} requires {label}")
                    option_value = values[index]
                if character in "cm":
                    if character == "m" and not option_value:
                        raise ValueError("Python -m requires a non-empty module")
                    return payload(
                        "command" if character == "c" else "module",
                        option_value,
                        values[index + 1 :],
                    )
                options.extend(
                    ("-" + character + attached,)
                    if attached
                    else ("-" + character, option_value)
                )
                break
            if character not in _PYTHON_FLAG_CHARACTERS:
                raise ValueError(f"unsupported Python interpreter option {value!r}")
            if character in "h?":
                return PythonInvocation(tuple(values[1:]), "terminal", None, ())
            if character == "V":
                version = True
        else:
            options.append(value)
        index += 1
    if index < len(values):
        if values[index] == "-":
            return payload("stdin", None, values[index + 1 :])
        return payload("script", values[index], values[index + 1 :])
    return payload("stdin", None, ())


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


def _basename(value: str) -> str:
    return value.replace("\\", "/").rsplit("/", 1)[-1].casefold()


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


def project_python(root: Path) -> str:
    """The interpreter that `uv run --python` selects in ``root``, as major.minor.

    uv runs the project interpreter that ``.python-version`` pins (the plan's
    ``python`` toolchain policy records the same pin as setup evidence). The
    default compile target is a different fact and may move on its own.
    """
    pin_path = root / ".python-version"
    pin = pin_path.read_text(encoding="utf-8").strip()
    match = re.fullmatch(r"(3\.[0-9]+)(?:\.[0-9]+)?", pin)
    if match is None:
        raise ValueError(f"{pin_path} must pin one CPython 3.x version, not {pin!r}")
    return match.group(1)


def uv_project_contract(root: Path) -> tuple[str, ...]:
    """The `uv run` launch prefix of every queued Python proof in ``root``.

    One owner for the contract: queue policy refuses an incomplete `uv run`
    command with ``uv_project_contract_gaps``, a typed wrapper's transport must
    have no gap, and the queue's own commands and help text spell it from here.
    """
    return (
        "uv",
        "run",
        "--active",
        "--project",
        ".",
        "--python",
        project_python(root),
        "--no-sync",
        "--no-config",
    )


def uv_project_contract_gaps(prefix: Sequence[str], *, root: Path) -> list[str]:
    """What a `uv run` prefix lacks of ``uv_project_contract(root)``."""
    python = project_python(root)
    gaps = []
    if "--active" not in prefix:
        gaps.append("--active")
    if _uv_option_values(prefix, "--project") != ["."]:
        gaps.append("--project .")
    if _uv_option_values(prefix, "--python") != [python]:
        gaps.append(f"--python {python}")
    if "--no-sync" not in prefix:
        gaps.append("--no-sync")
    if "--no-config" not in prefix:
        gaps.append("--no-config")
    return gaps


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


def _normalized_entrypoint_target(value: str, *, root: Path) -> str:
    normalized = value.replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    candidate = Path(value)
    if candidate.is_absolute():
        try:
            normalized = candidate.resolve(strict=False).relative_to(root).as_posix()
        except ValueError:
            pass
    return normalized.casefold()


def _command_wrapper(
    command: Sequence[str], *, root: Path
) -> tuple[dict[str, object], list[str]] | None:
    """Model one environment wrapper using its actual argument-parser authority.

    ``root`` is the checkout the command runs in.
    """
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
            candidate = root / candidate
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
                if selected == (root / "tools" / filename).resolve(strict=True)
            ),
            None,
        )
    else:
        return None
    if name is None:
        return None
    if transport_prefix is not None:
        # The contract's own tokens, plus --offline; nothing else.
        allowed = {*uv_project_contract(root)[1:], "--offline"}
        if any(
            value not in allowed for value in transport_prefix[1:]
        ) or uv_project_contract_gaps(transport_prefix, root=root):
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
    argv: Sequence[str], *, root: Path, _wrapper_depth: int = 0
) -> tuple[str, str] | None:
    """Return the stable program entrypoint whose plan authority cannot drift.

    Arguments are deliberately excluded.  If an argv is close enough to execute
    the same Python/Node program as a proof-plan command, it must be an exact plan
    command; otherwise changing one selector could silently discard that
    command's declared toolchain closure. ``root`` is the checkout the command
    runs in.
    """
    if not argv:
        return None
    modeled = _command_wrapper(argv, root=root)
    if modeled is not None:
        if _wrapper_depth >= 1:
            raise ValueError("command wrappers are limited to one typed layer")
        return _command_entrypoint(
            modeled[1], root=root, _wrapper_depth=_wrapper_depth + 1
        )
    payload = [str(value) for value in argv]
    if _basename(payload[0]) in {"uv", "uv.exe"}:
        _prefix, payload = _uv_prefix_and_payload(payload)
    first = _basename(payload[0])
    console_module = _PYTHON_CONSOLE_MODULES.get(first)
    if console_module is not None:
        # A Python console script is its module: `pytest` and `python -m pytest`
        # run one program, so they share one registered entrypoint (HF-179).
        return ("python-module", console_module.casefold())
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
            is_cli = is_molt_cli_payload("module", module, repo_root=root)
            entrypoint = ("python-module", module.casefold())
        elif invocation.mode == "script":
            target = str(invocation.target)
            if _basename(target) == "guarded_exec.py":
                return None
            normalized = _normalized_entrypoint_target(target, root=root)
            is_cli = is_molt_cli_payload(
                "script", str(invocation.target), repo_root=root
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
            return ("node-script", _normalized_entrypoint_target(target, root=root))
    return None


def pytest_arguments(argv: Sequence[str], *, root: Path) -> tuple[str, ...] | None:
    """pytest's own arguments when ``argv`` runs pytest, however it is spelled.

    ``uv run pytest``, ``python -m pytest``, a py launcher and one typed
    wrapper run one program (``_command_entrypoint``); any other argv gives
    None. ``root`` is the checkout the command runs in.
    """
    if _command_entrypoint(argv, root=root) != ("python-module", "pytest"):
        return None
    payload = [str(value) for value in argv]
    modeled = _command_wrapper(payload, root=root)
    if modeled is not None:
        payload = list(modeled[1])
    if _basename(payload[0]) in {"uv", "uv.exe"}:
        _prefix, payload = _uv_prefix_and_payload(payload)
    first = _basename(payload[0])
    if first in _PYTHON_CONSOLE_MODULES:
        return tuple(payload[1:])
    offset = (
        2
        if first in _PY_LAUNCHERS
        and len(payload) > 1
        and _PY_SELECTOR.fullmatch(payload[1])
        else 1
    )
    return parse_python_invocation([payload[0], *payload[offset:]]).arguments


def pytest_run_selections(arguments: Sequence[str], *, root: Path) -> tuple[str, ...]:
    """The checkout test paths a focused run selects, read as pytest reads them.

    The pytest custody check (``read_pytest_run``) reads the run as the
    sitecustomize bootstrap does when it starts: the configuration pytest
    selects in ``root``, with ``-o`` applied, its ``addopts``, then the argv.
    Admission refuses what custody refuses, among them ``@`` argument files and
    an ``addopts`` override. Admission runs outside pytest, so it also reads
    the installed and custody plugins' options, and a plugin option's value
    never reads as a target. The queue strips ``PYTEST_ADDOPTS`` from every
    run, so none applies here. ``testpaths`` applies only to a run that names
    no target, which a focused proof refuses. The result holds
    checkout-relative POSIX paths in the order pytest receives them.
    """
    from molt.pytest_memory_guard_bootstrap import read_pytest_run

    run = read_pytest_run(
        arguments,
        environ={},
        invocation_dir=root,
        plugin_options=True,
        repository_root=root,
    )
    if run.violations:
        raise ValueError("; ".join(run.violations))
    assert run.effective is not None
    if run.effective.pyargs:
        raise ValueError(
            "a focused pytest proof selects test paths; --pyargs names modules "
            "whose owning proof cells cannot be resolved"
        )
    checkout = root.resolve()
    selections: dict[str, None] = {}
    for target in run.effective.targets:
        candidate = Path(target.path)
        resolved = (
            candidate if candidate.is_absolute() else checkout / candidate
        ).resolve()
        if not resolved.exists():
            # pytest reports a missing path itself, and the separate value of an
            # option this parser does not know names nothing here either.
            continue
        try:
            relative = resolved.relative_to(checkout).as_posix()
        except ValueError:
            raise ValueError(
                f"focused pytest selection {str(target)!r} is outside the checkout"
            ) from None
        selections[relative] = None
    return tuple(selections)
