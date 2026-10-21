"""Proof-command argv: the program an argv runs and the tests pytest selects.

This is the one authority for reading a proof command's argv (HF-179). Proof
admission registers and admits commands through it, and the proof plan reads it
for CI selection and pytest memory units, as the test-ownership gate does. The
plan classifies changes with this module, so it is a proof-plan authority input
and keeps a light import closure: nothing here imports the compiler. Every
function that resolves a path takes the checkout it reads as ``root``.
"""

from __future__ import annotations

from dataclasses import dataclass
import functools
import os
from pathlib import Path
import re
import shlex
from typing import Any, Sequence

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


@dataclass(frozen=True, slots=True)
class PytestTarget:
    """One test path that a pytest argv selects, optionally narrowed to a node.

    ``path`` is pytest's spelling of the file or directory, relative to the
    invocation directory unless absolute, without a ``./`` prefix or a trailing
    slash. ``node`` is the text after the first ``::``, where pytest splits it
    (a parametrization may hold more ``::``), or None for the whole path.
    """

    path: str
    node: str | None

    @classmethod
    def from_argument(cls, value: str) -> PytestTarget:
        path, separator, node = value.partition("::")
        while path.startswith("./"):
            path = path[2:]
        return cls(path.rstrip("/") or path, node if separator else None)

    def __str__(self) -> str:
        return self.path if self.node is None else f"{self.path}::{self.node}"


@dataclass(frozen=True, slots=True)
class PytestArguments:
    """pytest's own reading of one argument list, before any configuration."""

    targets: tuple[PytestTarget, ...]
    pyargs: bool
    override_ini: tuple[str, ...]
    inifile: str | None
    rootdir: str | None


@functools.lru_cache(maxsize=1)
def _pytest_option_parser() -> Any:
    """pytest's own argument parser with its builtin and installed plugins.

    Only pytest knows which of its options take a value, so its parser, not a
    second option table, tells test targets from option values. Conftest files
    and ``-p`` modules are never imported here. An option they add is unknown
    to this parser, and its separate value then reads as a target: that can
    only add a path the caller must account for. Building the parser costs
    about 0.3 s once per process.
    """
    try:
        # pytest exposes no public parser constructor; get_config is the entry
        # its own plugin bootstrap uses.
        from _pytest.config import get_config
    except ImportError as exc:  # pragma: no cover - the queue's environment has it
        raise ValueError(
            "pytest test targets need pytest in the reading environment"
        ) from exc
    config = get_config()
    config.pluginmanager.load_setuptools_entrypoints("pytest11")
    return config._parser


def parse_pytest_arguments(arguments: Sequence[str]) -> PytestArguments:
    """Read one pytest argument list with pytest's own parser (HF-179)."""
    from _pytest.config.exceptions import UsageError

    try:
        namespace, _unknown = _pytest_option_parser().parse_known_and_unknown_args(
            [str(value) for value in arguments]
        )
    except UsageError as exc:
        raise ValueError(f"invalid pytest arguments: {exc}") from exc
    return PytestArguments(
        targets=tuple(
            PytestTarget.from_argument(str(value))
            for value in getattr(namespace, "file_or_dir", None) or ()
        ),
        pyargs=bool(getattr(namespace, "pyargs", False)),
        override_ini=tuple(getattr(namespace, "override_ini", None) or ()),
        inifile=getattr(namespace, "inifilename", None) or None,
        rootdir=getattr(namespace, "rootdir", None) or None,
    )


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


@functools.lru_cache(maxsize=4096)
def _command_pytest_targets(
    argv: tuple[str, ...], root: Path
) -> tuple[PytestTarget, ...] | None:
    arguments = pytest_arguments(argv, root=root)
    return None if arguments is None else parse_pytest_arguments(arguments).targets


def command_pytest_targets(
    argv: Sequence[object], *, root: Path
) -> tuple[PytestTarget, ...] | None:
    """The test targets a proof-plan argv selects, or None when it is not pytest.

    This is the one test-target authority (HF-179). The test-ownership gate,
    CI selection, pytest memory units and focused queue admission all read it.
    pytest's own parser reads the argv, so an option value (``--deselect``,
    ``--ignore``, ``-k``, ``--basetemp``) is never a target, and a node id keeps
    its node. A plan row is read without the checkout's ``addopts``;
    ``checkout_pytest_configuration_errors`` holds those to selecting nothing.
    ``root`` is the checkout the command runs in.
    """
    return _command_pytest_targets(tuple(str(value) for value in argv), root)


def _configured_pytest_addopts(
    arguments: PytestArguments, *, root: Path
) -> tuple[str, ...]:
    """The ``addopts`` of the configuration pytest selects for a run in ``root``.

    pytest's own ``determine_setup`` chooses the configuration file from the
    run's ``-c``, ``--rootdir`` and target paths, as ``pytest`` does when it is
    invoked in ``root``. Relative spellings are anchored at ``root``, not at the
    admitting process's working directory.
    """
    from _pytest.config.exceptions import UsageError
    from _pytest.config.findpaths import determine_setup
    from _pytest.outcomes import Failed

    def anchored(value: str) -> str:
        path = Path(value)
        return str(path if path.is_absolute() else root / path)

    try:
        _rootpath, _inipath, inicfg, _ignored = determine_setup(
            inifile=anchored(arguments.inifile) if arguments.inifile else None,
            override_ini=arguments.override_ini,
            args=[anchored(target.path) for target in arguments.targets],
            rootdir_cmd_arg=(
                anchored(os.path.expandvars(arguments.rootdir))
                if arguments.rootdir
                else None
            ),
            invocation_dir=root,
        )
    except (OSError, UsageError, Failed) as exc:
        raise ValueError(
            f"pytest configuration in {root} is unreadable: {exc}"
        ) from exc
    configured = inicfg.get("addopts")
    if configured is None:
        return ()
    value = configured.value
    if isinstance(value, str) and configured.mode == "ini":
        return tuple(shlex.split(value))
    if isinstance(value, list) and all(isinstance(item, str) for item in value):
        return tuple(value)
    raise ValueError(f"pytest addopts in {root} must be a string or a string list")


def checkout_pytest_configuration_errors(root: Path) -> list[str]:
    """Plan rows are read without addopts; the checkout's must select nothing.

    ``command_pytest_targets`` reads a plan row's own argv. That equals what
    pytest selects only while the checkout's ``addopts`` add no target and no
    ``--pyargs``.
    """
    configured = parse_pytest_arguments(
        _configured_pytest_addopts(parse_pytest_arguments(()), root=root)
    )
    errors = [
        f"pytest addopts in {root} must not select tests; {target} does"
        for target in configured.targets
    ]
    if configured.pyargs:
        errors.append(f"pytest addopts in {root} must not use --pyargs")
    return errors


def pytest_run_selections(arguments: Sequence[str], *, root: Path) -> tuple[str, ...]:
    """The checkout test paths a focused run selects, read as pytest reads them.

    pytest puts the ``addopts`` of its configuration before the argv. That
    configuration is the checkout's file as ``-c``, ``--rootdir`` and the
    targets choose it, with ``-o``/``--override-ini`` applied over it, so
    ``-o addopts=...`` selects what it names, exactly as pytest runs it.
    pytest reads ``testpaths`` only for a run that names no target, which a
    focused proof refuses. ``python_files``, ``python_functions`` and the other
    collection keys choose tests inside the selected paths, whose owners
    already cover them. The result holds checkout-relative POSIX paths in the
    order pytest receives them.
    """
    submitted = parse_pytest_arguments(arguments)
    effective = parse_pytest_arguments(
        (*_configured_pytest_addopts(submitted, root=root), *arguments)
    )
    if effective.pyargs:
        raise ValueError(
            "a focused pytest proof selects test paths; --pyargs names modules "
            "whose owning proof cells cannot be resolved"
        )
    checkout = root.resolve()
    selections: dict[str, None] = {}
    for target in effective.targets:
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
