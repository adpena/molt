"""Deterministic cross-platform custody for CPython interpreter commands."""

from __future__ import annotations

import json
import os
import platform
import shlex
import shutil
import subprocess
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from molt.target_python import TargetPythonVersion, _parse_target_python_version


class PythonInterpreterError(RuntimeError):
    """Raised when a requested CPython interpreter cannot be proven."""


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


def _split_explicit_command(value: str) -> tuple[str, ...]:
    if os.name != "nt":
        return tuple(shlex.split(value))

    # CommandLineToArgvW is the Windows command-line grammar used by native
    # launchers. shlex's POSIX and non-POSIX modes both mis-handle valid quoted
    # Windows paths in edge cases, so use the platform authority directly.
    import ctypes

    argc = ctypes.c_int()
    shell32 = ctypes.WinDLL("shell32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    command_line_to_argv = shell32.CommandLineToArgvW
    command_line_to_argv.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_int)]
    command_line_to_argv.restype = ctypes.POINTER(ctypes.c_wchar_p)
    argv = command_line_to_argv(value, ctypes.byref(argc))
    if not argv:
        raise PythonInterpreterError(
            f"invalid Windows Python command line (error {ctypes.get_last_error()})"
        )
    try:
        return tuple(argv[index] for index in range(argc.value))
    finally:
        kernel32.LocalFree(argv)


def explicit_python_command(value: str) -> tuple[str, ...]:
    """Parse an explicit interpreter path or command without changing its meaning."""

    explicit = value.strip()
    if not explicit:
        raise PythonInterpreterError("Python interpreter command must not be empty")
    expanded = Path(explicit).expanduser()
    if expanded.exists():
        return (str(expanded),)
    command = _split_explicit_command(explicit)
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


def probe_python_command(
    command: Sequence[str],
    *,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    timeout: float = 10.0,
) -> PythonInterpreter:
    """Run one identity probe and require a real CPython interpreter."""

    normalized = tuple(str(part) for part in command if str(part))
    if not normalized:
        raise PythonInterpreterError("Python interpreter command must not be empty")
    probe_env = dict(os.environ if env is None else env)
    probe_env["PYTHONHASHSEED"] = "0"
    probe_cwd = Path.cwd() if cwd is None else cwd
    stdout, stderr, rc = _run_command(
        [*normalized, "-c", _IDENTITY_PROBE],
        timeout=timeout,
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
) -> tuple[bool, str]:
    try:
        interpreter = probe_python_command(command, env=env, cwd=cwd, timeout=timeout)
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
                (sys.executable,), env=probe_env, cwd=probe_cwd
            )
        except PythonInterpreterError as exc:
            failures.append(f"{sys.executable}: {exc}")
    if override is None or not override.strip():
        uv = shutil.which("uv") if prefer_uv else None
        if uv is not None:
            stdout, stderr, rc = _run_command(
                [uv, "python", "find", target_python.short],
                timeout=10.0,
                env=probe_env,
                cwd=probe_cwd,
            )
            if rc == 0 and stdout.strip():
                candidate = (stdout.splitlines()[0].strip(),)
                try:
                    interpreter = probe_python_command(
                        candidate, env=probe_env, cwd=probe_cwd
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
            interpreter = probe_python_command(candidate, env=probe_env, cwd=probe_cwd)
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
) -> tuple[str, ...]:
    """Resolve an exact supported CPython minor and return its launch command."""

    return resolve_target_python(
        target_python,
        override=override,
        env=env,
        cwd=cwd,
        prefer_current=prefer_current,
        prefer_uv=prefer_uv,
    ).command


def resolve_python_selector(
    selector: str | None,
    *,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
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
