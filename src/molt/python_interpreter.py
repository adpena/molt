"""Deterministic cross-platform custody for CPython interpreter commands."""

from __future__ import annotations

import json
import os
import platform
import re
import shlex
import subprocess
import sys
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from molt.target_python import TargetPythonVersion, _parse_target_python_version


class PythonInterpreterError(RuntimeError):
    """Raised when a requested CPython interpreter cannot be proven."""


PYTHON_LAUNCHER_NAMES = frozenset({"py", "py.exe"})
PYTHON_LAUNCHER_SELECTOR = re.compile(
    r"(?:-\d+(?:\.\d+)?(?:-(?:32|64))?|-V:[^\s/:]+(?:/[^\s/:]+)?)",
    re.IGNORECASE,
)
_PYTHON_FLAG_CHARACTERS = frozenset("bBdEhiIOPqRsStuvVx?")
_PYTHON_TERMINAL_OPTIONS = frozenset(
    {"-h", "-?", "--help", "--help-all", "--help-env", "--help-xoptions", "--version"}
)


@dataclass(frozen=True)
class PythonInvocation:
    """Canonical CPython command-line split at the payload boundary.

    Interpreter options remain interpreter options when the payload is routed
    through the custody bootstrap. Payload arguments are never reparsed, so
    values beginning with ``-`` retain their ordinary ``sys.argv`` meaning.
    """

    interpreter_options: tuple[str, ...]
    mode: str
    target: str | None
    arguments: tuple[str, ...]


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


def python_environment_options(
    invocation: PythonInvocation,
) -> tuple[bool, bool, tuple[str, ...]]:
    """Project startup environment isolation, no-write and cache-prefix options."""
    ignored_environment = False
    no_bytecode = False
    prefixes = []
    options = iter(invocation.interpreter_options)
    for option in options:
        if option == "--check-hash-based-pycs":
            next(options)
        elif option == "-W":
            next(options)
        elif option.startswith("-W"):
            continue
        elif option.startswith("-X"):
            value = next(options) if option == "-X" else option[2:]
            if value.split("=", 1)[0] == "pycache_prefix":
                prefixes.append(value.split("=", 1)[1] if "=" in value else "")
        elif option.startswith("-") and not option.startswith("--"):
            ignored_environment |= any(flag in option[1:] for flag in "IE")
            no_bytecode |= "B" in option[1:]
    return ignored_environment, no_bytecode, tuple(prefixes)


@dataclass(frozen=True, slots=True)
class PythonInterpreter:
    """A runnable command and the identity reported by that interpreter."""

    command: tuple[str, ...]
    executable: str
    version: str
    implementation: str

    @property
    def major_minor(self) -> str:
        parts = self.version.split(".")
        return ".".join(parts[:2])


_IDENTITY_PROBE = "\n".join(
    (
        "import json, platform, sys",
        "print(json.dumps({",
        "    'executable': sys.executable,",
        "    'implementation': platform.python_implementation(),",
        "    'version': platform.python_version(),",
        "}, sort_keys=True))",
    )
)


def parse_target_python_version(value: str | None) -> TargetPythonVersion:
    return _parse_target_python_version(value)


def _looks_like_version_selector(value: str) -> bool:
    raw = value.strip().lower()
    if raw.startswith("py"):
        raw = raw[2:]
    return bool(raw) and all(part.isdigit() for part in raw.split("."))


def explicit_python_command(value: str) -> tuple[str, ...]:
    """Parse an explicit interpreter path or command without changing its meaning."""

    explicit = value.strip()
    if not explicit:
        raise PythonInterpreterError("Python interpreter command must not be empty")
    expanded = Path(explicit).expanduser()
    if expanded.exists():
        return (str(expanded),)
    # Share host quoting with configured compiler/linker commands. Import only
    # when command parsing is needed, not for interpreter metadata readers.
    from molt.toolchain_identity import split_native_command

    try:
        command = split_native_command(explicit)
    except ValueError as exc:
        raise PythonInterpreterError(f"invalid Python command line: {exc}") from exc
    if not command:
        raise PythonInterpreterError("Python interpreter command must not be empty")
    first = Path(command[0]).expanduser()
    path_separators = tuple(
        separator for separator in (os.sep, os.altsep) if separator is not None
    )
    is_path = (
        first.is_absolute()
        or bool(first.drive)
        or any(separator in command[0] for separator in path_separators)
    )
    if is_path:
        if not first.exists():
            raise PythonInterpreterError(f"Python interpreter not found: {first}")
        first_command = str(first) if command[0].startswith("~") else command[0]
        command = (first_command, *command[1:])
    return command


def format_python_command(command: Sequence[str]) -> str:
    if os.name == "nt":
        return subprocess.list2cmdline(list(command))
    return shlex.join(command)


def _subprocess_text(value: str | bytes | None) -> str:
    """Normalize subprocess payloads at the text-mode process boundary.

    ``TimeoutExpired`` may expose captured bytes even when ``run`` was called
    with ``text=True``.  Downstream interpreter diagnostics are text-only, so
    keep that platform quirk inside the custody layer.
    """

    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode(errors="replace")
    return value


def _run_command(
    command: Sequence[str],
    *,
    timeout: float,
    env: Mapping[str, str],
    cwd: Path,
) -> tuple[str, str, int]:
    try:
        result = subprocess.run(
            list(command),
            capture_output=True,
            text=True,
            timeout=timeout,
            env=dict(env),
            cwd=str(cwd),
            encoding="utf-8",
        )
    except FileNotFoundError as exc:
        return "", str(exc), 127
    except OSError as exc:
        return "", str(exc), 126
    except subprocess.TimeoutExpired as exc:
        stdout = _subprocess_text(exc.stdout)
        stderr = _subprocess_text(exc.stderr)
        return stdout, stderr or "timeout", 124
    return result.stdout, result.stderr, result.returncode


def _verifier_python_command(
    command: tuple[str, ...], verifier_bytecode_prefix: Path
) -> tuple[str, ...]:
    """Fence probe imports without changing the selected launcher's command.

    The canonical parser proves the original command has no earlier payload or
    incomplete option that could swallow the appended policy. Prior cache-prefix
    options are refused rather than relying on CPython's duplicate-option order.
    """
    from molt.python_environment_identity import python_identity_verifier_arguments

    basename = command[0].replace("\\", "/").rsplit("/", 1)[-1].casefold()
    option_start = 1
    if (
        basename in PYTHON_LAUNCHER_NAMES
        and len(command) > 1
        and PYTHON_LAUNCHER_SELECTOR.fullmatch(command[1]) is not None
    ):
        option_start = 2
    try:
        invocation = parse_python_invocation(
            [command[0], *command[option_start:], "-c", _IDENTITY_PROBE]
        )
    except ValueError as exc:
        raise PythonInterpreterError(f"invalid verifier Python options: {exc}") from exc
    if (
        invocation.mode != "command"
        or invocation.target != _IDENTITY_PROBE
        or invocation.arguments
    ):
        raise PythonInterpreterError(
            "verifier Python command must contain only launcher/interpreter options"
        )
    if python_environment_options(invocation)[2]:
        raise PythonInterpreterError(
            "verifier Python command cannot preselect a bytecode prefix"
        )
    return (
        *command,
        *python_identity_verifier_arguments(verifier_bytecode_prefix),
        "-S",
    )


def probe_python_command(
    command: Sequence[str],
    *,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    timeout: float = 10.0,
    probe_timeout: Callable[[float], float] | None = None,
    verifier_bytecode_prefix: Path | None = None,
) -> PythonInterpreter:
    """Run one identity probe and require a real CPython interpreter."""

    normalized = tuple(str(part) for part in command if str(part))
    if not normalized:
        raise PythonInterpreterError("Python interpreter command must not be empty")
    probe_env = dict(os.environ if env is None else env)
    probe_env["PYTHONHASHSEED"] = "0"
    probe_cwd = Path.cwd() if cwd is None else cwd
    probe_command = normalized
    if verifier_bytecode_prefix is not None:
        probe_command = _verifier_python_command(normalized, verifier_bytecode_prefix)
    stdout, stderr, rc = _run_command(
        [*probe_command, "-c", _IDENTITY_PROBE],
        timeout=timeout if probe_timeout is None else probe_timeout(timeout),
        env=probe_env,
        cwd=probe_cwd,
    )
    if rc != 0:
        detail = (stderr or stdout).strip() or f"returncode={rc}"
        raise PythonInterpreterError(
            f"{format_python_command(normalized)} identity probe failed: {detail}"
        )
    lines = [line for line in stdout.splitlines() if line.strip()]
    try:
        payload = json.loads(lines[-1])
    except (IndexError, json.JSONDecodeError) as exc:
        raise PythonInterpreterError(
            f"{format_python_command(normalized)} emitted invalid identity JSON"
        ) from exc
    implementation = str(payload.get("implementation", ""))
    executable = str(payload.get("executable", ""))
    version = str(payload.get("version", ""))
    if implementation != "CPython":
        raise PythonInterpreterError(
            f"{format_python_command(normalized)} is {implementation or '<unknown>'}, not CPython"
        )
    if not executable or not version:
        raise PythonInterpreterError(
            f"{format_python_command(normalized)} emitted incomplete identity metadata"
        )
    return PythonInterpreter(normalized, executable, version, implementation)


def verify_target_python_command(
    command: Sequence[str],
    *,
    target_python: TargetPythonVersion,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    timeout: float = 10.0,
    verifier_bytecode_prefix: Path | None = None,
) -> tuple[bool, str]:
    try:
        interpreter = probe_python_command(
            command,
            env=env,
            cwd=cwd,
            timeout=timeout,
            verifier_bytecode_prefix=verifier_bytecode_prefix,
        )
    except PythonInterpreterError as exc:
        return False, str(exc)
    if interpreter.major_minor != target_python.short:
        return False, f"reported CPython {interpreter.version}"
    return True, ""


def target_python_command_candidates(
    target_python: TargetPythonVersion,
    *,
    override: str | None = None,
    prefer_current: bool = False,
) -> list[list[str]]:
    """Return ordered direct-command candidates for one supported CPython minor."""

    if override is not None and override.strip():
        return [list(explicit_python_command(override))]
    candidates: list[list[str]] = []
    current = f"{sys.version_info.major}.{sys.version_info.minor}"
    if (
        prefer_current
        and platform.python_implementation() == "CPython"
        and current == target_python.short
    ):
        candidates.append([sys.executable])
    if os.name == "nt":
        candidates.append(["py", f"-{target_python.short}"])
    candidates.append([f"python{target_python.short}"])
    candidates.append([f"python{target_python.major}{target_python.minor}"])
    return candidates


def resolve_target_python(
    target_python: TargetPythonVersion,
    *,
    override: str | None = None,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    prefer_current: bool = False,
    prefer_uv: bool = True,
    probe_timeout: Callable[[float], float] | None = None,
    verifier_bytecode_prefix: Path | None = None,
) -> PythonInterpreter:
    """Resolve and identity-probe an exact supported CPython minor once."""

    probe_env = dict(os.environ if env is None else env)
    probe_cwd = Path.cwd() if cwd is None else cwd
    failures: list[str] = []
    current = f"{sys.version_info.major}.{sys.version_info.minor}"
    if (
        (override is None or not override.strip())
        and prefer_current
        and platform.python_implementation() == "CPython"
        and current == target_python.short
    ):
        try:
            return probe_python_command(
                (sys.executable,),
                env=probe_env,
                cwd=probe_cwd,
                probe_timeout=probe_timeout,
                verifier_bytecode_prefix=verifier_bytecode_prefix,
            )
        except PythonInterpreterError as exc:
            failures.append(f"{sys.executable}: {exc}")
    if override is None or not override.strip():
        from molt.toolchain_identity import find_executable

        uv_path = find_executable("uv", environment=os.environ) if prefer_uv else None
        uv = None if uv_path is None else os.fspath(uv_path)
        if uv is not None:
            stdout, stderr, rc = _run_command(
                [uv, "python", "find", target_python.short],
                timeout=10.0 if probe_timeout is None else probe_timeout(10.0),
                env=probe_env,
                cwd=probe_cwd,
            )
            if rc == 0 and stdout.strip():
                candidate = (stdout.splitlines()[0].strip(),)
                try:
                    interpreter = probe_python_command(
                        candidate,
                        env=probe_env,
                        cwd=probe_cwd,
                        probe_timeout=probe_timeout,
                        verifier_bytecode_prefix=verifier_bytecode_prefix,
                    )
                except PythonInterpreterError as exc:
                    failures.append(f"{format_python_command(candidate)}: {exc}")
                else:
                    if interpreter.major_minor == target_python.short:
                        return interpreter
                    failures.append(
                        f"{format_python_command(candidate)}: reported CPython "
                        f"{interpreter.version}"
                    )
            else:
                detail = (stderr or stdout).strip() or f"returncode={rc}"
                failures.append(f"{uv} python find {target_python.short}: {detail}")
    for candidate in target_python_command_candidates(
        target_python, override=override, prefer_current=False
    ):
        try:
            interpreter = probe_python_command(
                candidate,
                env=probe_env,
                cwd=probe_cwd,
                probe_timeout=probe_timeout,
                verifier_bytecode_prefix=verifier_bytecode_prefix,
            )
        except PythonInterpreterError as exc:
            failures.append(f"{format_python_command(candidate)}: {exc}")
            continue
        if interpreter.major_minor == target_python.short:
            return interpreter
        failures.append(
            f"{format_python_command(candidate)}: reported CPython {interpreter.version}"
        )
    attempted = "; ".join(failures) if failures else "no candidates"
    raise PythonInterpreterError(
        f"no verified CPython {target_python.short} command available ({attempted})"
    )


def resolve_target_python_command(
    target_python: TargetPythonVersion,
    *,
    override: str | None = None,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    prefer_current: bool = False,
    prefer_uv: bool = True,
    probe_timeout: Callable[[float], float] | None = None,
    verifier_bytecode_prefix: Path | None = None,
) -> tuple[str, ...]:
    """Resolve an exact supported CPython minor and return its launch command."""

    return resolve_target_python(
        target_python,
        override=override,
        env=env,
        cwd=cwd,
        prefer_current=prefer_current,
        prefer_uv=prefer_uv,
        probe_timeout=probe_timeout,
        verifier_bytecode_prefix=verifier_bytecode_prefix,
    ).command


def resolve_python_selector(
    selector: str | None,
    *,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    probe_timeout: Callable[[float], float] | None = None,
    verifier_bytecode_prefix: Path | None = None,
) -> tuple[str, ...]:
    """Resolve a CLI selector: default interpreter, exact version, path, or command."""

    if selector is None or not selector.strip():
        return (sys.executable,)
    if _looks_like_version_selector(selector):
        target = parse_target_python_version(selector)
        return resolve_target_python_command(
            target,
            env=env,
            cwd=cwd,
            prefer_current=True,
            probe_timeout=probe_timeout,
            verifier_bytecode_prefix=verifier_bytecode_prefix,
        )
    return explicit_python_command(selector)


def python_command_for_min_version(
    min_version: tuple[int, int],
    *,
    override: str | None = None,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
) -> tuple[str, ...]:
    """Return a CPython command satisfying at least ``min_version``."""

    if (
        platform.python_implementation() == "CPython"
        and sys.version_info[:2] >= min_version
    ):
        return (sys.executable,)
    target = parse_target_python_version(f"{min_version[0]}.{min_version[1]}")
    return resolve_target_python_command(target, override=override, env=env, cwd=cwd)
