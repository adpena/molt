"""Proof-command argv: the program an argv runs and the tests pytest selects.

This is the one reading of a proof command's argv (HF-179): the Python
invocation and its ``uv run`` transport, the typed wrappers, the program
entrypoint and, for a focused pytest proof, the paths pytest selects. Proof
admission and the queue's command identity read it. The proof plan does not: a
plan row declares its pytest targets as data. Every function that resolves a
path takes the checkout it reads as ``root``.
"""

from __future__ import annotations

from pathlib import Path
import re
from typing import Mapping, Sequence

from molt import python_interpreter
from tools.proof_queue_pkg.python_payload_authority import is_molt_cli_payload


_PYTHON_COMMAND = re.compile(r"^python(?:\d+(?:\.\d+)*)?(?:\.exe)?$", re.IGNORECASE)


def is_python_command(value: str) -> bool:
    return (
        _PYTHON_COMMAND.fullmatch(value.replace("\\", "/").rsplit("/", 1)[-1])
        is not None
    )


def _windows_python_program(value: str) -> tuple[str, int]:
    """The CRT's special pathname argv0 and its raw end coordinate."""
    if not value or "\x00" in value:
        raise ValueError("Windows Python command line is empty or contains NUL")
    index = 0
    quoted = False
    program: list[str] = []
    while index < len(value):
        character = value[index]
        if character == '"':
            quoted = not quoted
        elif character in " \t" and not quoted:
            break
        else:
            program.append(character)
        index += 1
    return "".join(program), index


def validate_windows_python_launch_prefix(value: str) -> None:
    """Require one option boundary across actual Python launcher families.

    uv's Python proxy and CPython's venv redirector remove argv0 themselves
    before forwarding to the real CRT interpreter. Escaped/concatenated quotes
    or control whitespace can make them remove different startup options.
    Accept only an identical remaining raw suffix; decoded argv0 is irrelevant.

    https://github.com/astral-sh/uv/blob/main/crates/uv-trampoline/src/bounce.rs
    https://github.com/python/cpython/blob/3.12/PC/launcher.c
    """
    _program, crt_end = _windows_python_program(value)
    uv_end = 0
    quoted = False
    while uv_end < len(value):
        character = value[uv_end]
        if character == '"':
            quoted = not quoted
        elif character == "\\" and value[uv_end + 1 : uv_end + 2] in {'"', "\\"}:
            uv_end += 1
        elif character in " \t\r\n\v\f" and not quoted:
            break
        uv_end += 1
    delimiter = '"' if value.startswith('"') else " "
    delimiter_at = value.find(delimiter, 1 if delimiter == '"' else 0)
    venv_end = len(value) if delimiter_at < 0 else delimiter_at + 1
    while venv_end < len(value) and value[venv_end] in " \t\r\n\v\f":
        venv_end += 1

    def suffix_start(index: int) -> int:
        # The downstream CRT consumes only space/tab argument delimiters.
        while index < len(value) and value[index] in " \t":
            index += 1
        return index

    if len({suffix_start(crt_end), suffix_start(uv_end), suffix_start(venv_end)}) != 1:
        raise ValueError("Windows Python launcher argv0 has ambiguous forwarding")


def split_windows_python_command_line(value: str) -> tuple[str, ...]:
    """Decode the CRT ``wmain`` argv received by Windows CPython.

    CPython's Programs/python.c passes wmain's argv directly to Py_Main.
    The CRT grammar differs from CommandLineToArgvW: doubled quotes inside
    quoted arguments produce a literal quote without ending the quoted span.
    argv[0] has its own pathname grammar, without backslash escaping.

    https://github.com/python/cpython/blob/3.12/Programs/python.c
    https://learn.microsoft.com/cpp/c-language/parsing-c-command-line-arguments
    """
    program, index = _windows_python_program(value)
    arguments = [program]
    while index < len(value):
        while index < len(value) and value[index] in " \t":
            index += 1
        if index == len(value):
            break
        argument: list[str] = []
        quoted = False
        while index < len(value):
            backslashes = 0
            while index < len(value) and value[index] == "\\":
                backslashes += 1
                index += 1
            if index < len(value) and value[index] == '"':
                argument.append("\\" * (backslashes // 2))
                if backslashes % 2:
                    argument.append('"')
                    index += 1
                elif quoted and index + 1 < len(value) and value[index + 1] == '"':
                    argument.append('"')
                    index += 2
                else:
                    quoted = not quoted
                    index += 1
                continue
            argument.append("\\" * backslashes)
            if index == len(value) or (value[index] in " \t" and not quoted):
                break
            argument.append(value[index])
            index += 1
        arguments.append("".join(argument))
    return tuple(arguments)


def _selected_python_prefix(prefixes: tuple[str, ...], expected: str) -> None:
    if len(prefixes) > 1:
        raise ValueError("Python cache prefix must be selected exactly once")
    if prefixes and prefixes != (expected,):
        raise ValueError(
            "Python cache prefix conflicts with the selected development image"
        )


def python_environment_command(
    argv: Sequence[str], *, environment_root: Path
) -> list[str]:
    """Bind a selected image's cache read namespace before Python starts."""
    from molt.python_environment_location import (
        environment_bytecode_arguments,
        environment_bytecode_prefix,
    )

    invocation = python_interpreter.parse_python_invocation(argv)
    if invocation.mode == "terminal":
        return list(argv)
    _ignored, _no_bytecode, prefixes = python_interpreter.python_environment_options(
        invocation
    )
    _selected_python_prefix(
        prefixes, str(environment_bytecode_prefix(environment_root))
    )
    policy = environment_bytecode_arguments(environment_root)
    return [str(argv[0]), *(policy[:1] if prefixes else policy), *argv[1:]]


def validate_python_environment_command(
    argv: Sequence[str],
    *,
    environment_root: Path,
    environment: Mapping[str, str],
) -> None:
    """Validate an already assembled child at the existing launch boundary."""
    from molt.python_environment_location import environment_bytecode_prefix

    invocation = python_interpreter.parse_python_invocation(argv)
    if invocation.mode == "terminal":
        return
    ignored, no_bytecode, prefixes = python_interpreter.python_environment_options(
        invocation
    )
    expected = str(environment_bytecode_prefix(environment_root))
    _selected_python_prefix(prefixes, expected)
    if not prefixes and (ignored or environment.get("PYTHONPYCACHEPREFIX") != expected):
        raise ValueError(
            "Python child does not select the development image cache prefix"
        )
    if not no_bytecode and (
        ignored or environment.get("PYTHONDONTWRITEBYTECODE") != "1"
    ):
        raise ValueError("Python child must disable bytecode writes before startup")


_PYTHON_CONSOLE_MODULES = {
    "pytest": "pytest",
    "pytest.exe": "pytest",
    "py.test": "pytest",
    "py.test.exe": "pytest",
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
    match = re.fullmatch(r"([0-9]+\.[0-9]+)(?:\.[0-9]+)?", pin)
    if match is None:
        raise ValueError(
            f"{pin_path} must pin one CPython release version, not {pin!r}"
        )
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


def uv_environment_command(
    argv: Sequence[str],
    *,
    repo_root: Path,
    environment_root: Path,
    python_selector: str | None = None,
) -> list[str]:
    """Run UV against one selected image without replacing its environment."""
    from molt.python_environment_location import environment_python_path

    prefix, payload = _uv_prefix_and_payload(argv)
    selected_python = str(environment_python_path(environment_root))
    allowed_python = {selected_python}
    if python_selector is not None:
        allowed_python.add(python_selector)
    python_options = {
        name
        for name, (_shape, role) in _UV_OPTION_SEMANTICS.items()
        if role == "python-selection"
    }
    if any(
        value not in allowed_python
        for option in python_options
        for value in _uv_option_values(prefix, option)
    ):
        raise ValueError(
            "uv Python selector conflicts with the selected development image"
        )
    if any(
        value not in {".", str(repo_root)}
        for value in _uv_option_values(prefix, "--project")
    ):
        raise ValueError("uv project selector conflicts with the selected checkout")
    if any(
        value not in {".", str(repo_root)}
        for value in _uv_option_values(prefix, "--directory")
    ):
        raise ValueError("uv directory selector conflicts with the selected checkout")
    retained = []
    index = 2
    while index < len(prefix):
        value = prefix[index]
        if value == "--":
            break
        option = value.split("=", 1)[0]
        shape, _role = _UV_OPTION_SEMANTICS[option]
        width = 2 if shape == "value" and "=" not in value else 1
        if option == "--isolated":
            raise ValueError(
                "uv --isolated conflicts with the selected development image"
            )
        if option in python_options | {"--project"}:
            pass
        elif option not in {"--active", "--no-sync", "--no-config"}:
            retained.extend(prefix[index : index + width])
        index += width
    return [
        str(argv[0]),
        "run",
        "--active",
        "--project",
        str(repo_root),
        "--python",
        selected_python,
        "--no-sync",
        "--no-config",
        *retained,
        "--",
        *payload,
    ]


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
    if first in python_interpreter.PYTHON_LAUNCHER_NAMES:
        offset = (
            2
            if len(payload) > 1
            and python_interpreter.PYTHON_LAUNCHER_SELECTOR.fullmatch(payload[1])
            else 1
        )
        payload = [payload[0], *payload[offset:]]
    elif not _PYTHON_COMMAND.fullmatch(first):
        return None
    invocation = python_interpreter.parse_python_invocation(payload)
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
    elif first in python_interpreter.PYTHON_LAUNCHER_NAMES:
        python_index = (
            2
            if len(payload) > 1
            and python_interpreter.PYTHON_LAUNCHER_SELECTOR.fullmatch(payload[1])
            else 1
        )
    if python_index is not None:
        if python_index >= len(payload):
            return None
        invocation = python_interpreter.parse_python_invocation(
            [payload[0], *payload[python_index:]]
        )
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
        if first in python_interpreter.PYTHON_LAUNCHER_NAMES
        and len(payload) > 1
        and python_interpreter.PYTHON_LAUNCHER_SELECTOR.fullmatch(payload[1])
        else 1
    )
    return python_interpreter.parse_python_invocation(
        [payload[0], *payload[offset:]]
    ).arguments


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
