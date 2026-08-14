"""Canonical interpretation of toolchain launcher content-path probes."""

from __future__ import annotations

from pathlib import Path
import sys


CONTENT_PATH_STRATEGIES = frozenset({"executable-path", "git-exec-path"})


def resolve_content_path(
    launcher: Path,
    probe_output: str,
    *,
    strategy: str = "executable-path",
    probe_cwd: Path,
    platform_name: str | None = None,
) -> Path:
    """Resolve one exact executable image from a declared probe result."""

    if strategy not in CONTENT_PATH_STRATEGIES:
        raise ValueError(f"unknown toolchain content-path strategy {strategy!r}")
    candidates = tuple(
        line.strip() for line in probe_output.splitlines() if line.strip()
    )
    if len(candidates) != 1:
        raise ValueError("toolchain content-path probe must return exactly one path")
    candidate = Path(candidates[0])
    if not candidate.is_absolute():
        candidate = probe_cwd / candidate

    if strategy == "executable-path":
        resolved = candidate.resolve(strict=True)
        if not resolved.is_file():
            raise ValueError(
                f"toolchain content-path probe returned no file: {resolved}"
            )
        return resolved

    platform_value = sys.platform if platform_name is None else platform_name
    launcher_path = launcher.resolve(strict=True)
    if platform_value != "win32":
        return launcher_path

    git_exec_path = candidate.resolve(strict=True)
    if (
        not git_exec_path.is_dir()
        or git_exec_path.name.casefold() != "git-core"
        or git_exec_path.parent.name.casefold() != "libexec"
    ):
        raise ValueError(
            "Git for Windows --exec-path did not identify its canonical runtime root"
        )
    runtime_root = git_exec_path.parents[1]
    runtime = (runtime_root / "bin" / "git.exe").resolve(strict=True)
    if not runtime.is_file():
        raise ValueError(f"Git for Windows runtime executable is missing: {runtime}")
    if (
        launcher_path.parent.name.casefold() == "bin"
        and launcher_path.parent.parent == runtime_root
        and launcher_path != runtime
    ):
        raise ValueError("Git runtime launcher disagrees with --exec-path authority")
    return runtime
