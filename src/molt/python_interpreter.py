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

from typing import TYPE_CHECKING, Literal, TypedDict, cast

from molt.host_python import frontend_python_supported
from molt.target_python import (
    TargetPythonVersion,
    _parse_target_python_version,
    require_supported_target_python,
)

if TYPE_CHECKING:
    from molt.process_environment import ProcessEnvironment
    from molt.python_environment_identity import PythonIdentityVerifier
    from molt.toolchain_identity import StableRegularFileVersion


class PythonInterpreterError(RuntimeError):
    """Raised when a requested CPython interpreter cannot be proven."""


PythonDiscoveryPolicy = Literal["general", "uv-managed"]
MANAGED_PYTHON_ORIGIN_KIND: Literal["uv-managed"] = "uv-managed"
BENCHMARK_PYTHON_ARGUMENTS = ("-I", "-S", "-B")
BENCHMARK_PYTHON_LAUNCH_POLICY: Literal["isolated-randomized-hash"] = (
    "isolated-randomized-hash"
)
MANAGED_PYTHON_LOOKUP_ARGS = (
    "python",
    "find",
    "--managed-python",
    "--no-project",
    "--no-python-downloads",
    "--system",
    "--no-config",
)


@dataclass(frozen=True, slots=True)
class PythonInterpreterRequest:
    """A physical CPython selection, independent of admitted guest semantics.

    A minor request admits that minor's patches; an explicit patch is exact.
    Guest callers project their admitted target into an exact release request, while
    development images preserve the full patch in the repository pin.
    """

    major: int
    minor: int
    micro: int | None = None

    def __post_init__(self) -> None:
        parts = (self.major, self.minor)
        if self.micro is not None:
            parts += (self.micro,)
        if any(type(part) is not int or part < 0 for part in parts):
            raise ValueError("CPython request components must be nonnegative integers")
        if (self.major, self.minor) < (3, 12):
            raise ValueError("CPython interpreter selection requires 3.12 or newer")

    @property
    def series(self) -> str:
        return f"{self.major}.{self.minor}"

    @property
    def selector(self) -> str:
        return self.series if self.micro is None else f"{self.series}.{self.micro}"

    def matches_version(self, value: str) -> bool:
        from packaging.version import InvalidVersion, Version

        try:
            observed = Version(value)
        except InvalidVersion:
            return False
        if (
            len(observed.release) != 3
            or observed.is_prerelease
            or observed.is_postrelease
            or observed.local is not None
        ):
            return False
        return self.matches_release(observed.release)

    def matches_release(self, release: tuple[int, ...]) -> bool:
        """Compare an already-parsed final CPython release without packaging."""
        return len(release) == 3 and (
            release[:2] == (self.major, self.minor)
            if self.micro is None
            else release == (self.major, self.minor, self.micro)
        )


def parse_python_interpreter_request(value: str) -> PythonInterpreterRequest:
    """Parse a canonical dotted host selection without consulting guest policy."""

    if (
        re.fullmatch(r"[1-9][0-9]*\.(?:0|[1-9][0-9]*)(?:\.(?:0|[1-9][0-9]*))?", value)
        is None
    ):
        raise ValueError("Python interpreter version must use major.minor[.patch]")
    parts = tuple(map(int, value.split(".")))
    return PythonInterpreterRequest(
        parts[0], parts[1], parts[2] if len(parts) == 3 else None
    )


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
class BenchmarkPythonHost:
    """Process platform and architecture observed by the interpreter."""

    sys_platform: str
    machine: str
    arch: str
    pointer_bits: Literal[32, 64]

    @classmethod
    def from_observation(
        cls, sys_platform: str, machine: str, pointer_bits: Literal[32, 64]
    ) -> BenchmarkPythonHost:
        from molt.native_target_shape import normalize_native_architecture

        if sys_platform == "win32":
            machine = "x86" if machine == "win32" else machine.removeprefix("win-")
        try:
            arch = normalize_native_architecture(machine)
        except RuntimeError as exc:
            raise PythonInterpreterError(str(exc)) from exc
        return cls(sys_platform, machine, arch, pointer_bits)


def benchmark_python_host() -> BenchmarkPythonHost:
    """Identify the host process ABI used to accept a benchmark interpreter."""
    if sys.platform == "win32":
        import sysconfig

        machine = sysconfig.get_platform()
    else:
        machine = platform.machine()
    return BenchmarkPythonHost.from_observation(
        sys.platform, machine, 64 if sys.maxsize > 2**32 else 32
    )


def benchmark_python_environment(environment: Mapping[str, str]) -> ProcessEnvironment:
    """Remove inherited CPython configuration before startup."""
    from molt.process_environment import ProcessEnvironment

    env = ProcessEnvironment(environment)
    return env.without(
        name
        for name in env
        if env.rule.key(name).startswith("PYTHON")
        or env.rule.key(name) == "__PYVENV_LAUNCHER__"
    )


class ManagedPythonOriginRecord(TypedDict):
    kind: Literal["uv-managed"]
    requested_release: str
    executable: str
    lookup_command: list[str]


@dataclass(frozen=True, slots=True)
class ManagedPythonOrigin:
    """An exact installed release witnessed by uv and an isolated probe."""

    request: PythonInterpreterRequest
    executable: Path
    uv: Path
    host: BenchmarkPythonHost

    def as_record(self) -> ManagedPythonOriginRecord:
        return {
            "kind": MANAGED_PYTHON_ORIGIN_KIND,
            "requested_release": self.request.selector,
            "executable": str(self.executable),
            "lookup_command": list(_managed_python_lookup(self.uv, self.request)),
        }


@dataclass(frozen=True, slots=True)
class PythonInterpreter:
    """A runnable command and the identity reported by that interpreter."""

    command: tuple[str, ...]
    executable: str
    version: str
    implementation: str
    managed_origin: ManagedPythonOrigin | None = None

    @property
    def major_minor(self) -> str:
        parts = self.version.split(".")
        return ".".join(parts[:2])


class BenchmarkPythonMetadata(TypedDict):
    cmd: list[str]
    executable: str
    implementation: str
    version: str
    sys_platform: str
    machine: str
    arch: str
    pointer_bits: Literal[32, 64]
    managed_origin: ManagedPythonOriginRecord
    launch_policy: Literal["isolated-randomized-hash"]


def benchmark_python_metadata(
    interpreter: PythonInterpreter,
) -> BenchmarkPythonMetadata:
    """Project the admitted benchmark oracle into the retained evidence format."""
    origin = interpreter.managed_origin
    if origin is None:
        raise PythonInterpreterError("benchmark CPython has no managed origin")
    return {
        "cmd": list(interpreter.command),
        "executable": interpreter.executable,
        "implementation": interpreter.implementation,
        "version": interpreter.version,
        "sys_platform": origin.host.sys_platform,
        "machine": origin.host.machine,
        "arch": origin.host.arch,
        "pointer_bits": origin.host.pointer_bits,
        "managed_origin": origin.as_record(),
        "launch_policy": BENCHMARK_PYTHON_LAUNCH_POLICY,
    }


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

_MANAGED_IDENTITY_PROBE = "\n".join(
    (
        "import json, platform, sys, sysconfig",
        "print(json.dumps({",
        "    'executable': sys.executable,",
        "    'base_executable': sys._base_executable,",
        "    'prefix': sys.prefix,",
        "    'base_prefix': sys.base_prefix,",
        "    'implementation': platform.python_implementation(),",
        "    'version': platform.python_version(),",
        "    'sys_platform': sys.platform,",
        "    'machine': sysconfig.get_platform() if sys.platform == 'win32' else platform.machine(),",
        "    'pointer_bits': 64 if sys.maxsize > 2**32 else 32,",
        "}, sort_keys=True))",
    )
)


def parse_target_python_version(value: str | None) -> TargetPythonVersion:
    return _parse_target_python_version(value)


def is_python_version_selector(value: str) -> bool:
    """Classify version-shaped input; strict request parsing owns its validity."""
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


def _run_command(
    command: Sequence[str],
    *,
    verifier: PythonIdentityVerifier,
    timeout: float,
    env: Mapping[str, str],
    cwd: Path,
) -> tuple[str, str, int]:
    result = verifier.run(
        command,
        memory_guard_prefix="MOLT_PYTHON_IDENTITY",
        capture_output=True,
        text=True,
        timeout=timeout,
        env=env,
        cwd=cwd,
        encoding="utf-8",
    )
    return result.stdout or "", result.stderr or "", result.returncode


def _validate_python_command_options(command: tuple[str, ...], *, purpose: str) -> None:
    """Prove appended startup policy cannot be swallowed by a prior payload.

    One canonical parser owns both metadata and comparison launch prefixes.
    Prior cache-prefix options are refused instead of depending on duplicate
    option precedence. The launcher's selector remains outside CPython options.
    """
    if not command:
        raise PythonInterpreterError(f"{purpose} Python command must not be empty")
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
        raise PythonInterpreterError(
            f"invalid {purpose} Python options: {exc}"
        ) from exc
    if (
        invocation.mode != "command"
        or invocation.target != _IDENTITY_PROBE
        or invocation.arguments
    ):
        raise PythonInterpreterError(
            f"{purpose} Python command must contain only launcher/interpreter options"
        )
    if python_environment_options(invocation)[2]:
        raise PythonInterpreterError(
            f"{purpose} Python command cannot preselect a bytecode prefix"
        )


def readonly_bytecode_arguments(root: Path) -> tuple[str, str, str]:
    """``-B`` and a cache prefix below ``root``: the child neither reads the
    base install's bytecode nor writes any (a hermetic, no-write namespace)."""
    return ("-B", "-X", f"pycache_prefix={root.absolute() / 'bytecode'}")


def readonly_python_command(
    command: Sequence[str], *, environment_root: Path
) -> tuple[str, ...]:
    """Apply the canonical no-write read namespace before ordinary site startup.

    The caller owns or has admitted the root; this projection does not admit an image.
    Comparison callers may own a fresh empty root for their reference workload.
    Payload and launcher semantics, including environment and site policy, stay
    explicit in the original command.
    """
    normalized = tuple(str(part) for part in command)
    _validate_python_command_options(normalized, purpose="read-only")
    return (*normalized, *readonly_bytecode_arguments(environment_root))


def _verifier_python_command(
    command: tuple[str, ...], verifier_bytecode_prefix: Path, *, site_enabled: bool
) -> tuple[str, ...]:
    """Fence metadata imports without changing the selected launcher's prefix."""
    from molt.python_environment_identity import python_identity_verifier_arguments

    _validate_python_command_options(command, purpose="verifier")
    return (
        *command,
        *python_identity_verifier_arguments(verifier_bytecode_prefix),
        *(() if site_enabled else ("-S",)),
    )


def probe_python_command(
    command: Sequence[str],
    *,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    timeout: float = 10.0,
    probe_timeout: Callable[[float], float] | None = None,
    verifier: PythonIdentityVerifier,
) -> PythonInterpreter:
    """Run one identity probe and require a real CPython interpreter."""

    normalized = tuple(str(part) for part in command if str(part))
    if not normalized:
        raise PythonInterpreterError("Python interpreter command must not be empty")
    probe_env = dict(os.environ if env is None else env)
    probe_env["PYTHONHASHSEED"] = "0"
    probe_cwd = Path.cwd() if cwd is None else cwd
    payload = _probe_python_identity_payload(
        normalized,
        _IDENTITY_PROBE,
        site_enabled=False,
        env=probe_env,
        cwd=probe_cwd,
        timeout=timeout if probe_timeout is None else probe_timeout(timeout),
        verifier=verifier,
    )
    return _interpreter_from_probe(normalized, payload)


def _probe_python_identity_payload(
    command: tuple[str, ...],
    probe: str,
    *,
    site_enabled: bool,
    env: Mapping[str, str],
    cwd: Path,
    timeout: float,
    verifier: PythonIdentityVerifier,
) -> dict[str, object]:
    probe_command = _verifier_python_command(
        command, verifier.prefix, site_enabled=site_enabled
    )
    stdout, stderr, rc = _run_command(
        [*probe_command, "-c", probe],
        verifier=verifier,
        timeout=timeout,
        env=env,
        cwd=cwd,
    )
    if rc != 0:
        detail = (stderr or stdout).strip() or f"returncode={rc}"
        raise PythonInterpreterError(
            f"{format_python_command(command)} identity probe failed: {detail}"
        )
    lines = [line for line in stdout.splitlines() if line.strip()]
    try:
        payload: object = json.loads(lines[-1])
    except (IndexError, json.JSONDecodeError) as exc:
        raise PythonInterpreterError(
            f"{format_python_command(command)} emitted invalid identity JSON"
        ) from exc
    if not isinstance(payload, dict):
        raise PythonInterpreterError(
            f"{format_python_command(command)} emitted invalid identity JSON"
        )
    return cast(dict[str, object], payload)


def _interpreter_from_probe(
    command: tuple[str, ...], payload: Mapping[str, object]
) -> PythonInterpreter:
    implementation = str(payload.get("implementation", ""))
    executable = str(payload.get("executable", ""))
    version = str(payload.get("version", ""))
    if implementation != "CPython":
        raise PythonInterpreterError(
            f"{format_python_command(command)} is {implementation or '<unknown>'}, not CPython"
        )
    if not executable or not version:
        raise PythonInterpreterError(
            f"{format_python_command(command)} emitted incomplete identity metadata"
        )
    return PythonInterpreter(command, executable, version, implementation)


def verify_target_python_command(
    command: Sequence[str],
    *,
    target_python: TargetPythonVersion,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    timeout: float = 10.0,
    verifier: PythonIdentityVerifier,
) -> tuple[bool, str]:
    try:
        require_supported_target_python(target_python)
    except ValueError as exc:
        return False, str(exc)
    try:
        interpreter = probe_python_command(
            command,
            env=env,
            cwd=cwd,
            timeout=timeout,
            verifier=verifier,
        )
    except PythonInterpreterError as exc:
        return False, str(exc)
    try:
        request = target_release_request(target_python)
    except PythonInterpreterError as exc:
        return False, str(exc)
    if not request.matches_version(interpreter.version):
        return False, f"reported CPython {interpreter.version}"
    return True, ""


def python_command_candidates(
    request: PythonInterpreterRequest,
    *,
    override: str | None = None,
    prefer_current: bool = False,
) -> list[list[str]]:
    """Return physical commands whose reported version must satisfy the request."""

    if override is not None and override.strip():
        return [list(explicit_python_command(override))]
    candidates: list[list[str]] = []
    current = ".".join(map(str, sys.version_info[:3]))
    if (
        prefer_current
        and platform.python_implementation() == "CPython"
        and request.matches_version(current)
    ):
        candidates.append([sys.executable])
    if os.name == "nt":
        candidates.append(["py", f"-{request.series}"])
    candidates.append([f"python{request.series}"])
    candidates.append([f"python{request.major}{request.minor}"])
    return candidates


def resolve_python_interpreter(
    request: PythonInterpreterRequest | None,
    *,
    override: str | None = None,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    prefer_current: bool = False,
    prefer_uv: bool = True,
    policy: PythonDiscoveryPolicy = "general",
    probe_timeout: Callable[[float], float] | None = None,
    verifier: PythonIdentityVerifier,
) -> PythonInterpreter:
    """Resolve a physical interpreter under the caller's discovery policy."""

    probe_env = dict(os.environ if env is None else env)
    probe_cwd = Path.cwd() if cwd is None else cwd
    if policy == "uv-managed":
        return _resolve_uv_managed_python(
            request,
            override=override,
            env=probe_env,
            cwd=probe_cwd.absolute(),
            timeout=10.0 if probe_timeout is None else probe_timeout(10.0),
            verifier=verifier,
        )
    if request is None:
        raise PythonInterpreterError(
            "general Python discovery requires a release request"
        )
    failures: list[str] = []
    current = ".".join(map(str, sys.version_info[:3]))
    if (
        (override is None or not override.strip())
        and prefer_current
        and platform.python_implementation() == "CPython"
        and request.matches_version(current)
    ):
        try:
            interpreter = probe_python_command(
                (sys.executable,),
                env=probe_env,
                cwd=probe_cwd,
                probe_timeout=probe_timeout,
                verifier=verifier,
            )
            if request.matches_version(interpreter.version):
                return interpreter
            failures.append(f"{sys.executable}: reported CPython {interpreter.version}")
        except PythonInterpreterError as exc:
            failures.append(f"{sys.executable}: {exc}")
    if override is None or not override.strip():
        from molt.toolchain_identity import find_executable

        uv_path = (
            find_executable("uv", environment=probe_env, cwd=probe_cwd)
            if prefer_uv
            else None
        )
        uv = None if uv_path is None else os.fspath(uv_path)
        if uv is not None:
            stdout, stderr, rc = _run_command(
                [uv, "python", "find", request.selector],
                verifier=verifier,
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
                        verifier=verifier,
                    )
                except PythonInterpreterError as exc:
                    failures.append(f"{format_python_command(candidate)}: {exc}")
                else:
                    if request.matches_version(interpreter.version):
                        return interpreter
                    failures.append(
                        f"{format_python_command(candidate)}: reported CPython "
                        f"{interpreter.version}"
                    )
            else:
                detail = (stderr or stdout).strip() or f"returncode={rc}"
                failures.append(f"{uv} python find {request.selector}: {detail}")
    for candidate in python_command_candidates(
        request, override=override, prefer_current=False
    ):
        if override is None or not override.strip():
            from molt.toolchain_identity import find_executable

            selected = find_executable(
                candidate[0], environment=probe_env, cwd=probe_cwd
            )
            if selected is None:
                failures.append(
                    f"{format_python_command(candidate)}: not found on selected PATH"
                )
                continue
            candidate = [str(selected), *candidate[1:]]
        try:
            interpreter = probe_python_command(
                candidate,
                env=probe_env,
                cwd=probe_cwd,
                probe_timeout=probe_timeout,
                verifier=verifier,
            )
        except PythonInterpreterError as exc:
            failures.append(f"{format_python_command(candidate)}: {exc}")
            continue
        if request.matches_version(interpreter.version):
            return interpreter
        failures.append(
            f"{format_python_command(candidate)}: reported CPython {interpreter.version}"
        )
    attempted = "; ".join(failures) if failures else "no candidates"
    raise PythonInterpreterError(
        f"no verified CPython {request.selector} command available ({attempted}); "
        f"run `uv python install {request.selector}`"
    )


_EXACT_RELEASE = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)")


def target_release_request(
    target_python: TargetPythonVersion, value: str | None = None
) -> PythonInterpreterRequest:
    """The exact CPython release that serves an admitted target.

    A dotted patch in ``value`` is exact. Otherwise the target's pinned source
    release (``CPYTHON_RELEASES``) is the release, never whatever patch a uv
    minor link names today (HF-760, HF-726).
    """
    require_supported_target_python(target_python)
    if target_python.release != "final":
        raise PythonInterpreterError(
            f"target Python {target_python.short} pins a {target_python.release} "
            "release; an exact interpreter needs a final release"
        )
    micro = target_python.micro
    raw = "" if value is None else value.strip()
    if _EXACT_RELEASE.fullmatch(raw):
        request = parse_python_interpreter_request(raw)
        if (request.major, request.minor) != target_python.feature_version:
            raise PythonInterpreterError(
                f"CPython {raw} does not serve target Python {target_python.short}"
            )
        assert request.micro is not None
        micro = request.micro
    return PythonInterpreterRequest(target_python.major, target_python.minor, micro)


def resolve_benchmark_python(
    *,
    override: str | None = None,
    request: PythonInterpreterRequest | None = None,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    verifier: PythonIdentityVerifier,
) -> PythonInterpreter:
    """Admit an installed base interpreter for the exact benchmark protocol.

    A blank override selects the canonical guest release. A series selects its
    canonical patch, an exact release selects that patch, and a physical path
    selects the exact patch its interpreter reports.
    """
    explicit = "" if override is None else override.strip()
    if explicit and is_python_version_selector(explicit):
        raw = explicit.lower().removeprefix("py")
        try:
            selected = parse_python_interpreter_request(raw)
        except ValueError as exc:
            raise PythonInterpreterError(str(exc)) from exc
        if request is not None and request != selected:
            raise PythonInterpreterError(
                f"benchmark CPython override {explicit!r} conflicts with {request.selector}"
            )
        request = selected
        explicit = ""
    if not explicit and request is None:
        request = target_release_request(parse_target_python_version(None))
    if request is not None and request.micro is None:
        try:
            request = target_release_request(
                parse_target_python_version(request.series)
            )
        except ValueError as exc:
            raise PythonInterpreterError(str(exc)) from exc
    try:
        return resolve_python_interpreter(
            request,
            override=explicit or None,
            env=env,
            cwd=cwd,
            policy="uv-managed",
            verifier=verifier,
        )
    except (OSError, ValueError, subprocess.TimeoutExpired) as exc:
        raise PythonInterpreterError(
            f"benchmark CPython admission failed: {exc}"
        ) from exc


def _managed_python_lookup(
    uv: Path, request: PythonInterpreterRequest
) -> tuple[str, ...]:
    return (str(uv), *MANAGED_PYTHON_LOOKUP_ARGS, request.selector)


def _resolve_uv_managed_python(
    request: PythonInterpreterRequest | None,
    *,
    override: str | None,
    env: Mapping[str, str],
    cwd: Path,
    timeout: float,
    verifier: PythonIdentityVerifier,
) -> PythonInterpreter:
    """Join the isolated base probe to uv's exact installed selection."""
    from molt.default_paths import expand_user_path
    from molt.toolchain_identity import (
        executable_content_path,
        find_executable,
        verify_stable_regular_file_identity,
    )

    if request is not None and request.micro is None:
        raise PythonInterpreterError("managed-only discovery requires an exact release")
    observation: (
        tuple[PythonInterpreter, BenchmarkPythonHost, StableRegularFileVersion] | None
    ) = None
    if override is not None:
        path = expand_user_path(override, environment=env)
        entry = path if path.is_absolute() else cwd / path
        observation = _probe_managed_python_base(
            entry, env=env, cwd=cwd, timeout=timeout, verifier=verifier
        )
        observed, _, _ = observation
        if request is None:
            request = parse_python_interpreter_request(observed.version)
        elif not request.matches_version(observed.version):
            raise PythonInterpreterError(
                f"{entry} reports CPython {observed.version}, expected {request.selector}"
            )

    if request is None or request.micro is None:
        raise PythonInterpreterError("managed-only discovery requires an exact release")
    uv = find_executable("uv", environment=env, cwd=cwd)
    if uv is None:
        raise PythonInterpreterError(
            "uv is required to select an installed benchmark CPython"
        )
    command = _managed_python_lookup(uv, request)
    stdout, stderr, rc = _run_command(
        command, verifier=verifier, timeout=timeout, env=env, cwd=cwd
    )
    lines = stdout.splitlines()
    if rc != 0 or len(lines) != 1 or not Path(lines[0]).is_absolute():
        detail = (stderr or stdout).strip() or f"returncode={rc}"
        raise PythonInterpreterError(
            f"no installed uv-managed CPython {request.selector} ({detail}); "
            f"run `uv python install {request.selector}`"
        )
    selected_entry = Path(lines[0])
    if observation is None:
        observation = _probe_managed_python_base(
            selected_entry, env=env, cwd=cwd, timeout=timeout, verifier=verifier
        )
    observed, host, image = observation
    executable = image.path
    selected_executable = executable_content_path(
        selected_entry, label="uv-managed benchmark CPython"
    )
    if executable != selected_executable:
        raise PythonInterpreterError(
            f"{observed.executable} is not the uv-managed CPython {request.selector} "
            f"executable {selected_executable}"
        )
    if not request.matches_version(observed.version):
        raise PythonInterpreterError(
            f"{selected_entry} reports CPython {observed.version}, expected {request.selector}"
        )
    verify_stable_regular_file_identity(image, label="benchmark CPython executable")
    return PythonInterpreter(
        (str(executable), *BENCHMARK_PYTHON_ARGUMENTS),
        str(executable),
        observed.version,
        observed.implementation,
        ManagedPythonOrigin(request, executable, uv, host),
    )


def _probe_managed_python_base(
    entry: Path,
    *,
    env: Mapping[str, str],
    cwd: Path,
    timeout: float,
    verifier: PythonIdentityVerifier,
) -> tuple[PythonInterpreter, BenchmarkPythonHost, StableRegularFileVersion]:
    from molt.python_environment_location import python_venv_config_candidates
    from molt.toolchain_identity import (
        executable_content_path,
        is_searchable_executable,
        stable_regular_file_version,
        verify_stable_regular_file_identity,
    )

    if not is_searchable_executable(entry):
        raise PythonInterpreterError(
            f"benchmark CPython executable is unavailable: {entry}"
        )
    for config in python_venv_config_candidates(Path(os.path.abspath(entry))):
        try:
            config.lstat()
        except FileNotFoundError:
            continue
        raise PythonInterpreterError(
            f"{entry} is a virtual environment interpreter ({config})"
        )
    entry_stat = entry.stat()
    command = (str(entry),)
    # The canonical lexical venv fence runs before this no-site probe. On
    # 3.12, -S alone hides venv prefix evidence; resolving links first would
    # also erase the original placement. No rejected image runs site startup.
    payload = _probe_python_identity_payload(
        command,
        _MANAGED_IDENTITY_PROBE,
        site_enabled=False,
        env=benchmark_python_environment(env),
        cwd=cwd,
        timeout=timeout,
        verifier=verifier,
    )
    interpreter = _interpreter_from_probe(command, payload)
    prefix = payload.get("prefix")
    if (
        not isinstance(prefix, str)
        or not prefix
        or prefix != payload.get("base_prefix")
    ):
        raise PythonInterpreterError(
            f"{entry} is not a base CPython interpreter (virtual environment or missing prefix)"
        )
    base_executable = payload.get("base_executable")
    if (
        not isinstance(base_executable, str)
        or not Path(base_executable).is_absolute()
        or Path(interpreter.executable) != entry
    ):
        raise PythonInterpreterError(
            f"{entry} did not launch directly as a CPython base executable"
        )
    sys_platform = payload.get("sys_platform")
    machine = payload.get("machine")
    pointer_bits = payload.get("pointer_bits")
    if (
        not isinstance(sys_platform, str)
        or not isinstance(machine, str)
        or type(pointer_bits) is not int
        or pointer_bits not in (32, 64)
    ):
        raise PythonInterpreterError(
            f"{entry} emitted incomplete process platform metadata"
        )
    host = BenchmarkPythonHost.from_observation(sys_platform, machine, pointer_bits)
    expected = benchmark_python_host()
    if (host.sys_platform, host.arch, host.pointer_bits) != (
        expected.sys_platform,
        expected.arch,
        expected.pointer_bits,
    ):
        raise PythonInterpreterError(
            f"{entry} reports {host.sys_platform}/{host.arch}/{host.pointer_bits}-bit, "
            f"expected {expected.sys_platform}/{expected.arch}/{expected.pointer_bits}-bit"
        )
    executable = executable_content_path(entry, label="benchmark CPython executable")
    image = stable_regular_file_version(
        executable, label="benchmark CPython executable"
    )
    verify_stable_regular_file_identity(
        image,
        expected_path_stat=entry_stat,
        label="probed benchmark CPython executable",
    )
    if (
        executable_content_path(
            Path(base_executable), label="benchmark CPython base executable"
        )
        != executable
    ):
        raise PythonInterpreterError(
            f"{entry} differs from its reported CPython base executable"
        )
    return interpreter, host, image


def resolve_target_python(
    target_python: TargetPythonVersion,
    *,
    override: str | None = None,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    prefer_current: bool = False,
    prefer_uv: bool = True,
    probe_timeout: Callable[[float], float] | None = None,
    verifier: PythonIdentityVerifier,
) -> PythonInterpreter:
    """Select the pinned oracle for an admitted guest through physical custody."""

    require_supported_target_python(target_python)
    return resolve_python_interpreter(
        target_release_request(target_python),
        override=override,
        env=env,
        cwd=cwd,
        prefer_current=prefer_current,
        prefer_uv=prefer_uv,
        probe_timeout=probe_timeout,
        verifier=verifier,
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
    verifier: PythonIdentityVerifier,
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
        verifier=verifier,
    ).command


def resolve_python_selector(
    selector: str | None,
    *,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    probe_timeout: Callable[[float], float] | None = None,
    verifier: PythonIdentityVerifier,
) -> tuple[str, ...]:
    """Resolve a CLI selector: default interpreter, exact version, path, or command."""

    if selector is None or not selector.strip():
        return (sys.executable,)
    if is_python_version_selector(selector):
        request = parse_python_interpreter_request(selector)
        interpreter = resolve_python_interpreter(
            request,
            env=env,
            cwd=cwd,
            prefer_current=True,
            probe_timeout=probe_timeout,
            verifier=verifier,
        )
        from packaging.version import Version

        observed = Version(interpreter.version)
        if not frontend_python_supported(
            (observed.major, observed.minor, observed.micro), implementation="cpython"
        ):
            raise PythonInterpreterError(
                f"CPython {interpreter.version} is not an admitted compiler host"
            )
        return interpreter.command
    return explicit_python_command(selector)


def python_command_for_min_version(
    min_version: tuple[int, int],
    *,
    verifier: PythonIdentityVerifier,
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
    request = PythonInterpreterRequest(*min_version)
    return resolve_python_interpreter(
        request, verifier=verifier, override=override, env=env, cwd=cwd
    ).command


INTERPRETER_RECORD = "molt-python-interpreter.json"
_SCHEMA = "molt.realized-python-interpreter.v1"
_PROBE = f"""import json,platform,sys
from pathlib import Path
print(json.dumps(dict(schema={_SCHEMA!r},
 executable=str(Path(getattr(sys,'_base_executable',None) or sys.executable).resolve(strict=True)),
 version=platform.python_version(),implementation=sys.implementation.name)))
"""


@dataclass(frozen=True)
class RealizedPythonInterpreter:
    executable: Path
    version: str

    @classmethod
    def parse(cls, payload: object) -> RealizedPythonInterpreter:
        from molt import python_environment_contract as contract

        if (
            not isinstance(payload, dict)
            or set(payload) != {"schema", "executable", "version", "implementation"}
            or payload["schema"] != _SCHEMA
            or payload["implementation"] != "cpython"
            or not isinstance(payload["executable"], str)
            or not Path(payload["executable"]).is_absolute()
        ):
            raise contract.SourceBuildEnvironmentError(
                "invalid realized Python interpreter"
            )
        version = contract.parse_canonical_python_runtime_version(payload["version"])
        return cls(Path(payload["executable"]), version.text)

    def publish(self, root: Path) -> None:
        from molt.exact_json import write_exact

        write_exact(
            root / INTERPRETER_RECORD,
            {
                "schema": _SCHEMA,
                "implementation": "cpython",
                "executable": str(self.executable),
                "version": self.version,
            },
        )


def observe_python_interpreter(
    executable: Path, environ: Mapping[str, str], *, cwd: Path | None = None
) -> RealizedPythonInterpreter:
    """Observe a fresh CPython process without importing the editable project."""
    from molt import process_guard
    from molt import python_environment_contract as contract
    from molt.exact_json import loads_exact
    from molt.process_environment import ProcessEnvironment
    from molt.toolchain_identity import stable_executable_probe

    with stable_executable_probe(executable, label="image interpreter") as (entry, _):
        result = process_guard.run_completed_command(
            [str(entry), "-I", "-B", "-S", "-c", _PROBE],
            env=ProcessEnvironment(environ),
            cwd=cwd,
            capture_output=True,
            text=True,
            check=False,
            timeout=60.0,
        )
    if result.returncode != 0:
        raise contract.SourceBuildEnvironmentError(
            f"cannot observe image interpreter {executable}: "
            + (
                result.stderr or result.stdout or f"returncode={result.returncode}"
            ).strip()
        )
    return RealizedPythonInterpreter.parse(loads_exact(result.stdout))


def development_interpreter_problem(
    root: Path, version: str, environ: Mapping[str, str], *, cwd: Path | None = None
) -> str | None:
    """Return drift, leaving malformed records and failed observations loud."""
    from molt.exact_json import read_exact
    from molt.uv_environment import python_path as environment_python_path
    from molt.process_environment import ProcessEnvironment

    environment = ProcessEnvironment(environ)
    if environment.get("UV_PROJECT_ENVIRONMENT", "").strip():
        # This coordinate has already been selected against its checkout.
        # Pass the physical selection across the guard boundary: rebasing a
        # relative activation against the guard's source root changes custody.
        environment = environment.updated({"UV_PROJECT_ENVIRONMENT": str(root)})
    found = observe_python_interpreter(
        environment_python_path(root), environment, cwd=cwd
    )
    record = root / INTERPRETER_RECORD
    if not record.exists() and not record.is_symlink():
        return f"missing realized interpreter record; found {found.executable} (CPython {found.version})"
    recorded = RealizedPythonInterpreter.parse(
        read_exact(record, max_bytes=65536, label="realized Python interpreter")
    )
    if found != recorded or found.version != version:
        return (
            f"recorded {recorded.executable} (CPython {recorded.version}), "
            f"found {found.executable} (CPython {found.version}); expected CPython {version}"
        )
    return None
