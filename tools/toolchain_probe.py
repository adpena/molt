"""Exact developer-tool probe execution and path selection."""

from __future__ import annotations

import errno
import os
from pathlib import Path
import subprocess
from typing import Literal, Mapping

from molt.cargo_execution_policy import cargo_subprocess_environment


ToolchainProbeStage = Literal["content resolver", "version query"]


def probe_failure(
    policy_name: str,
    argv: list[str],
    cwd: Path,
    cause: str,
    *,
    stage: ToolchainProbeStage,
    stdout: str | bytes | None = None,
    stderr: str | bytes | None = None,
) -> ValueError:
    """Retain the failed selection boundary without inventing an image path."""

    def excerpt(value: str | bytes | None) -> str | None:
        if value is None:
            return None
        limit = 2048
        omitted = len(value) - limit
        half = limit // 2
        parts = [value] if omitted <= 0 else [value[:half], value[-half:]]
        decoded = [
            part.decode("utf-8", errors="replace") if isinstance(part, bytes) else part
            for part in parts
        ]
        return (
            decoded[0]
            if omitted <= 0
            else f"{decoded[0]}...<omitted {omitted} "
            f"{'bytes' if isinstance(value, bytes) else 'characters'}>...{decoded[1]}"
        )

    return ValueError(
        f"{policy_name}: {stage} failed: cause={excerpt(cause)!r}; "
        f"argv={argv!r}; cwd={str(cwd)!r}; "
        f"stdout={excerpt(stdout)!r}; stderr={excerpt(stderr)!r}"
    )


def run_toolchain_probe(
    policy_name: str,
    argv: list[str],
    *,
    stage: ToolchainProbeStage,
    cwd: Path,
    environment: Mapping[str, str],
) -> subprocess.CompletedProcess[str]:
    """Require a successful bounded probe under the canonical cargo environment.

    A bare ``argv[0]`` is found in the child's own environment (its PATH and
    PATHEXT), never in this process: Windows' CreateProcess searches the
    caller's PATH, not the child's, and admission does not write its
    projection into the caller. An explicit path launches as given.
    """
    from molt.toolchain_identity import find_executable

    try:
        child_environment = cargo_subprocess_environment(argv, environment, cwd=cwd)[0]
        command = argv[0]
        if not Path(command).is_absolute() and not any(
            separator in command for separator in "/\\"
        ):
            found = find_executable(command, environment=child_environment, cwd=cwd)
            if found is None:
                raise FileNotFoundError(
                    errno.ENOENT, "not on the probe environment's PATH", command
                )
            command = str(found)
        completed = subprocess.run(
            [command, *argv[1:]],
            cwd=cwd,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE
            if stage == "content resolver"
            else subprocess.STDOUT,
            text=True,
            timeout=5,
            env=child_environment,
            encoding="utf-8",
        )
    except subprocess.TimeoutExpired as exc:
        raise probe_failure(
            policy_name,
            argv,
            cwd,
            f"timed out after {exc.timeout}s",
            stage=stage,
            stdout=exc.stdout,
            stderr=exc.stderr,
        ) from exc
    except (OSError, ValueError) as exc:
        raise probe_failure(
            policy_name,
            argv,
            cwd,
            f"{type(exc).__name__}: {exc}",
            stage=stage,
        ) from exc
    if completed.returncode != 0:
        raise probe_failure(
            policy_name,
            argv,
            cwd,
            f"exit status {completed.returncode}",
            stage=stage,
            stdout=completed.stdout,
            stderr=completed.stderr,
        )
    return completed


def resolve_single_file_path(
    output: str, *, probe_cwd: Path, preserve_entrypoint: bool = False
) -> Path:
    """Decode one file; optionally retain its canonical runtime launch alias."""

    candidates = tuple(line.strip() for line in output.splitlines() if line.strip())
    if len(candidates) != 1:
        raise ValueError("toolchain path probe must emit exactly one non-empty line")
    candidate = Path(candidates[0])
    if not candidate.is_absolute():
        candidate = probe_cwd / candidate
    resolved = candidate.resolve(strict=True)
    if not resolved.is_file():
        raise ValueError(f"toolchain path probe did not name a file: {resolved}")
    if preserve_entrypoint:
        # Runtime entrypoints may be venv symlinks. Normalize the launch spelling
        # without resolving that link, but prove that removing parent components
        # did not change the image selected through a symbolic directory.
        entrypoint = Path(os.path.abspath(candidate))
        if entrypoint != candidate and entrypoint.resolve(strict=True) != resolved:
            raise ValueError(
                "canonical entrypoint alias selects a different physical image"
            )
        return entrypoint
    return resolved
