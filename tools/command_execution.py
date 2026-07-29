"""Canonical typed-argv execution authority for repository tools.

Tool modules bind one :class:`CommandExecutor` from their ``__file__``. Every
completed child then receives a stable telemetry/memory prefix, canonical DX
environment, timeout custody, and subprocess-compatible check/output behavior.
"""

from __future__ import annotations

import hashlib
import re
import subprocess
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import Any

try:
    from tools.import_file import load_sibling_package_module_from_path
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from import_file import load_sibling_package_module_from_path  # type: ignore


def _harness_memory_guard() -> Any:
    try:
        from tools import harness_memory_guard
    except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
        import harness_memory_guard  # type: ignore

    return harness_memory_guard


def _repo_root(source_file: str | Path) -> Path:
    source = Path(source_file).resolve()
    for candidate in (source.parent, *source.parents):
        if (candidate / "pyproject.toml").is_file() and (candidate / "tools").is_dir():
            return candidate
    raise RuntimeError(f"cannot locate Molt repository root from {source}")


def _prefix(source_file: str | Path, root: Path) -> str:
    relative = Path(source_file).resolve().relative_to(root).with_suffix("")
    identity = re.sub(r"[^A-Za-z0-9]+", "_", relative.as_posix()).strip("_")
    return f"MOLT_{identity.upper()}"


@lru_cache(maxsize=None)
def _process_guard_authority(repo_root: str) -> Any:
    path = Path(repo_root) / "src" / "molt" / "process_guard.py"
    root_identity = hashlib.sha256(str(Path(repo_root).resolve()).encode()).hexdigest()
    package_name = f"_molt_process_guard_authority_{root_identity[:16]}"
    return load_sibling_package_module_from_path(
        f"{package_name}.process_guard",
        path,
    )


_READ_ONLY_GIT_SUBCOMMANDS = frozenset(
    {
        "check-attr",
        "diff",
        "grep",
        "log",
        "rev-parse",
        "show",
        "status",
    }
)
_VERSION_FLAGS = frozenset({"--help", "--version", "-V", "-vV", "-version"})


def _is_bounded_metadata_probe(command: Sequence[str]) -> bool:
    executable = Path(command[0]).name.lower()
    if executable.endswith(".exe"):
        executable = executable[:-4]
    if executable == "git":
        index = 1
        while index < len(command) and command[index] == "-C":
            index += 2
        if index >= len(command):
            return False
        subcommand = command[index]
        if subcommand in _READ_ONLY_GIT_SUBCOMMANDS:
            return True
        return subcommand == "config" and "--get" in command[index + 1 :]
    return any(flag in _VERSION_FLAGS for flag in command[1:])


@dataclass(frozen=True, slots=True)
class CommandExecutor:
    prefix: str
    repo_root: Path

    @classmethod
    def for_file(cls, source_file: str | Path) -> "CommandExecutor":
        root = _repo_root(source_file)
        return cls(prefix=_prefix(source_file, root), repo_root=root)

    def run(
        self,
        args: Sequence[str],
        *,
        cwd: str | Path | None = None,
        env: Mapping[str, str] | None = None,
        input: str | bytes | None = None,
        capture_output: bool = False,
        stdout_capture_path: str | Path | None = None,
        stderr_capture_path: str | Path | None = None,
        capture_tail_bytes: int | None = None,
        text: bool | None = False,
        timeout: float | None = None,
        check: bool = False,
        stdout: int | None = None,
        stderr: int | None = None,
        encoding: str | None = None,
        errors: str | None = None,
    ) -> Any:
        if isinstance(args, (str, bytes)):
            raise TypeError("command must be typed argv, not shell text")
        command = [str(part) for part in args]
        if not command:
            raise ValueError("command argv must not be empty")
        if capture_output and (stdout is not None or stderr is not None):
            raise ValueError("capture_output cannot be combined with stdout or stderr")
        process_guard = _process_guard_authority(str(self.repo_root))
        return process_guard.run_completed_command(
            command,
            cwd=cwd,
            env=env,
            input=input,
            capture_output=capture_output,
            stdout_capture_path=stdout_capture_path,
            stderr_capture_path=stderr_capture_path,
            capture_tail_bytes=capture_tail_bytes,
            text=text,
            timeout=timeout,
            check=check,
            stdout=stdout,
            stderr=stderr,
            encoding=encoding,
            errors=errors,
            memory_guard_prefix=(
                None if _is_bounded_metadata_probe(command) else self.prefix
            ),
        )

    def check_output(
        self,
        args: Sequence[str],
        **kwargs: object,
    ) -> str | bytes:
        if "stdout" in kwargs:
            raise ValueError("stdout is owned by CommandExecutor.check_output")
        result = self.run(args, stdout=subprocess.PIPE, check=True, **kwargs)
        assert result.stdout is not None
        return result.stdout

    def start_owned(
        self,
        args: Sequence[str],
        *,
        cwd: str | Path | None = None,
        env: Mapping[str, str] | None = None,
        stdin: int | None = None,
        stdout: object | None = None,
        stderr: object | None = None,
        text: bool = False,
        encoding: str | None = None,
        errors: str | None = None,
        bufsize: int = -1,
        close_fds: bool = True,
        creationflags: int = 0,
        start_new_session: bool = False,
    ) -> subprocess.Popen[Any]:
        """Start one explicitly caller-owned typed-argv process."""

        if isinstance(args, (str, bytes)):
            raise TypeError("command must be typed argv, not shell text")
        command = [str(part) for part in args]
        if not command:
            raise ValueError("command argv must not be empty")
        harness_memory_guard = _harness_memory_guard()
        env, _cargo_policies = harness_memory_guard.cargo_subprocess_environment(
            command,
            env,
        )
        return subprocess.Popen(
            command,
            cwd=cwd,
            env=None if env is None else dict(env),
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            text=text,
            encoding=encoding,
            errors=errors,
            bufsize=bufsize,
            close_fds=close_fds,
            creationflags=creationflags,
            start_new_session=start_new_session,
        )

    def start_guarded(
        self,
        args: Sequence[str],
        *,
        cwd: str | Path | None = None,
        env: Mapping[str, str] | None = None,
        stdin: int | None = None,
        stdout: object | None = None,
        stderr: object | None = None,
        text: bool = False,
        encoding: str | None = None,
        errors: str | None = None,
        bufsize: int = -1,
        timeout: float | None = None,
        summary_json: str | Path | None = None,
    ) -> subprocess.Popen[Any]:
        """Start an interactive child through the canonical memory-guard owner."""

        if isinstance(args, (str, bytes)):
            raise TypeError("command must be typed argv, not shell text")
        command = [str(part) for part in args]
        if not command:
            raise ValueError("command argv must not be empty")
        harness_memory_guard = _harness_memory_guard()
        env, _cargo_policies = harness_memory_guard.cargo_subprocess_environment(
            command,
            env,
        )
        context = harness_memory_guard.HarnessExecutionContext.from_env(
            self.prefix,
            env,
            repo_root=self.repo_root,
        )
        limits = context.limits
        guarded_argv = [
            sys.executable,
            str(self.repo_root / "tools" / "memory_guard.py"),
            "--max-rss-gb",
            str(limits.max_process_rss_gb),
            "--max-total-rss-gb",
            str(limits.max_total_rss_gb),
            "--max-global-rss-gb",
            str(limits.max_global_rss_gb),
            "--poll-interval",
            str(limits.poll_interval),
            "--child-rlimit-gb",
            str(0 if limits.child_rlimit_gb is None else limits.child_rlimit_gb),
        ]
        if timeout is not None:
            if timeout <= 0:
                raise ValueError("timeout must be positive")
            guarded_argv.extend(("--timeout", str(timeout)))
        if summary_json is not None:
            guarded_argv.extend(("--summary-json", str(summary_json)))
        guarded_argv.extend(("--", *command))
        return self.start_owned(
            guarded_argv,
            cwd=cwd,
            env=context.env,
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            text=text,
            encoding=encoding,
            errors=errors,
            bufsize=bufsize,
        )
