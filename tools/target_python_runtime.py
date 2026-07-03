"""Cross-platform CPython command custody for target-version parity lanes."""

from __future__ import annotations

import os
import shlex
import shutil
import subprocess
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent
_SRC_ROOT = _REPO_ROOT / "src"
if str(_SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(_SRC_ROOT))

from molt.cli.target_python import (  # noqa: E402
    TargetPythonVersion,
    _parse_target_python_version,
)

_PYTHON_VERSION_PROBE = (
    "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}', end='')"
)


def parse_target_python_version(value: str | None) -> TargetPythonVersion:
    return _parse_target_python_version(value)


def _command_from_override(override: str) -> list[str]:
    explicit = override.strip()
    if not explicit:
        return []
    if Path(explicit).expanduser().exists():
        return [explicit]
    parsed = shlex.split(explicit, posix=os.name != "nt")
    return parsed if parsed else [explicit]


def target_python_command_candidates(
    target_python: TargetPythonVersion,
    *,
    override: str | None = None,
    prefer_current: bool = False,
) -> list[list[str]]:
    """Return direct interpreter command candidates for a target CPython minor."""

    if override is not None and override.strip():
        override_command = _command_from_override(override)
        return [override_command] if override_command else []

    candidates: list[list[str]] = []
    current = f"{sys.version_info.major}.{sys.version_info.minor}"
    if prefer_current and current == target_python.short:
        candidates.append([sys.executable])
    if os.name == "nt":
        candidates.append(["py", f"-{target_python.short}"])
    candidates.append([f"python{target_python.short}"])
    candidates.append([f"python{target_python.major}{target_python.minor}"])
    return candidates


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
        return exc.stdout or "", exc.stderr or "timeout", 124
    return result.stdout, result.stderr, result.returncode


def verify_target_python_command(
    command: Sequence[str],
    *,
    target_python: TargetPythonVersion,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    timeout: float = 10.0,
) -> tuple[bool, str]:
    probe_env = dict(os.environ if env is None else env)
    probe_env["PYTHONHASHSEED"] = "0"
    stdout, stderr, rc = _run_command(
        [*command, "-c", _PYTHON_VERSION_PROBE],
        timeout=timeout,
        env=probe_env,
        cwd=_REPO_ROOT if cwd is None else cwd,
    )
    if rc != 0:
        detail = (stderr or stdout).strip()
        return False, detail or f"returncode={rc}"
    actual = stdout.strip()
    if actual != target_python.short:
        return False, f"reported Python {actual or '<empty>'}"
    return True, ""


def resolve_target_python_command(
    target_python: TargetPythonVersion,
    *,
    override: str | None = None,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    prefer_current: bool = False,
) -> list[str]:
    """Return a verified CPython command for an exact target minor."""

    probe_env = dict(os.environ if env is None else env)
    probe_cwd = _REPO_ROOT if cwd is None else cwd
    failures: list[str] = []

    if override is None or not override.strip():
        uv = shutil.which("uv")
        if uv is not None:
            stdout, stderr, rc = _run_command(
                [uv, "python", "find", target_python.short],
                timeout=10.0,
                env=probe_env,
                cwd=probe_cwd,
            )
            if rc == 0 and stdout.strip():
                candidate = [stdout.splitlines()[0].strip()]
                ok, detail = verify_target_python_command(
                    candidate,
                    target_python=target_python,
                    env=probe_env,
                    cwd=probe_cwd,
                )
                if ok:
                    return candidate
                failures.append(f"{' '.join(candidate)}: {detail}")
            else:
                detail = (stderr or stdout).strip()
                failures.append(
                    f"{uv} python find {target_python.short}: "
                    f"{detail or f'returncode={rc}'}"
                )

    for candidate in target_python_command_candidates(
        target_python,
        override=override,
        prefer_current=prefer_current,
    ):
        ok, detail = verify_target_python_command(
            candidate,
            target_python=target_python,
            env=probe_env,
            cwd=probe_cwd,
        )
        if ok:
            return candidate
        failures.append(f"{' '.join(candidate)}: {detail}")

    attempted = "; ".join(failures) if failures else "no candidates"
    raise RuntimeError(
        f"no verified CPython {target_python.short} command available ({attempted})"
    )


def python_command_for_min_version(
    min_version: tuple[int, int],
    *,
    override: str | None = None,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
) -> list[str]:
    """Return a CPython command satisfying at least `min_version`."""

    if sys.version_info[:2] >= min_version:
        return [sys.executable]
    target = parse_target_python_version(f"{min_version[0]}.{min_version[1]}")
    return resolve_target_python_command(
        target,
        override=override,
        env=env,
        cwd=cwd,
    )
