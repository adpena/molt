"""Canonical typed-argv execution authority for repository tools.

Tool modules bind one :class:`CommandExecutor` from their ``__file__``. Every
completed child then receives a stable telemetry/memory prefix, canonical DX
environment, timeout custody, and subprocess-compatible check/output behavior.
"""

from __future__ import annotations

import hashlib
import json
import uuid
from datetime import datetime, timezone
import re
import subprocess
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import IO, Any

try:
    from tools.import_file import (
        bind_repository_imports,
        load_sibling_package_module_from_path,
    )
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from import_file import (
        bind_repository_imports,
        load_sibling_package_module_from_path,
    )


def _harness_memory_guard() -> Any:
    from tools import harness_memory_guard

    return harness_memory_guard


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
        "ls-files",
        "rev-parse",
        "show",
        "status",
    }
)
# Global git options that only shape how a read proceeds. None of them can
# name a hook, a pager, or a config value, so the subcommand after them keeps
# its class. ``-c key=value`` stays outside this set: it can arm
# ``core.fsmonitor`` or ``core.pager`` and so turns any read into a launch.
_INERT_GIT_GLOBAL_OPTIONS = frozenset({"--no-optional-locks", "--no-pager"})
_VERSION_FLAGS = frozenset({"--help", "--version", "-V", "-vV", "-version"})


def _is_bounded_metadata_probe(command: Sequence[str]) -> bool:
    executable = Path(command[0]).name.lower()
    if executable.endswith(".exe"):
        executable = executable[:-4]
    if executable == "git":
        index = 1
        while index < len(command):
            option = command[index]
            if option == "-C":
                index += 2
            elif option in _INERT_GIT_GLOBAL_OPTIONS:
                index += 1
            else:
                break
        if index >= len(command):
            return False
        subcommand = command[index]
        if subcommand in _READ_ONLY_GIT_SUBCOMMANDS:
            return True
        return subcommand == "config" and "--get" in command[index + 1 :]
    return any(flag in _VERSION_FLAGS for flag in command[1:])


@dataclass(slots=True)
class GuardedCommand:
    """Launch handle and admitted guard identity, with cancellation custody.

    Only the guard may terminate its sampled/Job-owned child tree. The caller
    requests cancellation and waits; a stalled owner keeps its evidence and
    handle instead of being killed while a child might still be alive.
    """

    process: subprocess.Popen[Any]
    cancellation_path: Path
    summary_path: Path
    evidence_path: Path
    launch_id: str
    startup_path: Path
    command: tuple[str, ...]
    terminal: bool = False
    guard_pid: int | None = None
    child_identity: dict[str, object] | None = None

    @property
    def stdin(self):
        return self.process.stdin

    @property
    def stdout(self):
        return self.process.stdout

    @property
    def stderr(self):
        return self.process.stderr

    @property
    def pid(self) -> int:
        """The owned launcher PID; admitted guard_pid can be different."""
        return self.process.pid

    @property
    def args(self):
        return self.process.args

    @property
    def returncode(self):
        return self.process.returncode

    def poll(self):
        return self.process.poll()

    def request_cancel(self) -> None:
        # The exclusive launch directory is the capability. No PID lookup,
        # signal, process-group kill, or authority transfer occurs here.
        try:
            with self.cancellation_path.open("x", encoding="utf-8") as handle:
                handle.write("cancel\n")
        except FileExistsError:
            pass

    def wait(self, timeout: float | None = None) -> int:
        self.terminal = False
        result = int(self.process.wait(timeout=timeout))
        try:
            startup = json.loads(self.startup_path.read_text(encoding="utf-8"))
            payload = json.loads(self.summary_path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            raise RuntimeError(
                f"guard exited without startup/terminal child custody; inspect {self.evidence_path}"
            ) from exc
        valid_startup = bool(
            isinstance(startup, dict)
            and startup.get("launch_id") == self.launch_id
            and startup.get("command") == list(self.command)
            and type(startup.get("guard_pid")) is int
            and startup["guard_pid"] > 0
            and (self.guard_pid is None or startup["guard_pid"] == self.guard_pid)
            and isinstance(startup.get("child_process"), dict)
            and type(startup["child_process"].get("pid")) is int
            and startup["child_process"]["pid"] > 0
            and isinstance(startup["child_process"].get("started_at"), str)
        )
        self.terminal = bool(
            valid_startup
            and isinstance(payload, dict)
            and payload.get("launch_id") == self.launch_id
            and payload.get("guard_pid") == startup["guard_pid"]
            and payload.get("command") == list(self.command)
            and payload.get("child_process") == startup["child_process"]
            and payload.get("descendants_closed") is True
            and type(payload.get("child_returncode")) is int
        )
        if not self.terminal:
            raise RuntimeError(
                f"guard exited with unresolved child custody; inspect {self.evidence_path}"
            )
        self.guard_pid = startup["guard_pid"]
        self.child_identity = startup["child_process"]
        return result


@dataclass(frozen=True, slots=True)
class CommandExecutor:
    prefix: str
    repo_root: Path

    @classmethod
    def for_file(cls, source_file: str | Path) -> "CommandExecutor":
        root = bind_repository_imports(source_file)
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
    ) -> subprocess.CompletedProcess[Any]:
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
        *,
        cwd: str | Path | None = None,
        env: Mapping[str, str] | None = None,
        input: str | bytes | None = None,
        stderr: int | None = None,
        text: bool | None = False,
        timeout: float | None = None,
        encoding: str | None = None,
        errors: str | None = None,
    ) -> str | bytes:
        result = self.run(
            args,
            cwd=cwd,
            env=env,
            input=input,
            stdout=subprocess.PIPE,
            stderr=stderr,
            text=text,
            timeout=timeout,
            check=True,
            encoding=encoding,
            errors=errors,
        )
        assert result.stdout is not None
        return result.stdout

    def start_owned(
        self,
        args: Sequence[str],
        *,
        cwd: str | Path | None = None,
        env: Mapping[str, str] | None = None,
        stdin: int | IO[Any] | None = None,
        stdout: int | IO[Any] | None = None,
        stderr: int | IO[Any] | None = None,
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

    def wait_owned(
        self,
        process: subprocess.Popen[Any] | GuardedCommand,
        *,
        timeout: float,
        terminate_timeout: float = 5.0,
    ) -> int:
        """Wait finitely and clean only the exact process this caller owns."""

        if timeout <= 0 or terminate_timeout <= 0:
            raise ValueError("owned process timeouts must be positive")
        try:
            return int(process.wait(timeout=timeout))
        except subprocess.TimeoutExpired as timeout_error:
            if isinstance(process, GuardedCommand):
                try:
                    process.request_cancel()
                    process.wait(timeout=terminate_timeout)
                except (
                    subprocess.SubprocessError,
                    OSError,
                    RuntimeError,
                ) as cleanup_error:
                    timeout_error.add_note(
                        f"guard cancellation remains unresolved: {cleanup_error}; "
                        f"custody: {process.evidence_path}"
                    )
                    # Keep both errors and the exact live handle inspectable.
                    timeout_error.guard_command = process
                    timeout_error.cleanup_error = cleanup_error
                raise timeout_error
            process.terminate()
            try:
                process.wait(timeout=terminate_timeout)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=terminate_timeout)
            raise timeout_error

    def start_guarded(
        self,
        args: Sequence[str],
        *,
        cwd: str | Path | None = None,
        env: Mapping[str, str] | None = None,
        stdin: int | IO[Any] | None = None,
        stdout: int | IO[Any] | None = None,
        stderr: int | IO[Any] | None = None,
        text: bool = False,
        encoding: str | None = None,
        errors: str | None = None,
        bufsize: int = -1,
        timeout: float | None = None,
        summary_json: str | Path | None = None,
    ) -> GuardedCommand:
        """Start an interactive command with admitted actual-worker custody."""

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
        from molt.memory_guard_paths import memory_guard_state_root

        launch_id = uuid.uuid4().hex
        custody = (
            memory_guard_state_root(self.repo_root, context.env)
            / "commands"
            / launch_id
        )
        custody.mkdir(parents=True, exist_ok=False)
        cancellation_path = custody / "cancel"
        startup_path = custody / "startup.json"
        summary_path = (
            custody / "guard.json"
            if summary_json is None
            else Path(summary_json).resolve(strict=False)
        )
        # Revoke a caller-selected summary from any previous launch before
        # spawning; a worker crash must never borrow an old terminal receipt.
        summary_path.parent.mkdir(parents=True, exist_ok=True)
        summary_path.write_text(
            json.dumps(
                {
                    "status": "launching",
                    "launch_id": launch_id,
                    "guard_pid": None,
                    "descendants_closed": False,
                }
            )
            + "\n",
            encoding="utf-8",
        )
        evidence_path = custody / "custody.json"
        evidence = {
            "command": command,
            "cwd": str(Path.cwd() if cwd is None else Path(cwd).resolve()),
            "launch_id": launch_id,
            "launch_pid": None,
            "guard_pid": None,
            "child_pid": None,
            "startup_path": str(startup_path),
            "status": "launching",
            "recorded_at": datetime.now(timezone.utc).isoformat(),
            "summary_path": str(summary_path),
            "cancellation_path": str(cancellation_path),
        }
        evidence_path.write_text(
            json.dumps(evidence, sort_keys=True) + "\n", encoding="utf-8"
        )
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
        guarded_argv.extend(
            (
                "--summary-json",
                str(summary_path),
                "--cancel-file",
                str(cancellation_path),
            )
        )
        # Use the existing hidden-command worker contract directly. On Windows
        # memory_guard's command-line facade otherwise adds a detached wrapper
        # whose Popen is not the worker that owns the Job or active marker.
        worker_environment = harness_memory_guard.memory_guard._worker_env(
            context.env, command, launch_id=launch_id, startup_json=str(startup_path)
        )
        process = self.start_owned(
            guarded_argv,
            cwd=cwd,
            env=worker_environment,
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            text=text,
            encoding=encoding,
            errors=errors,
            bufsize=bufsize,
        )
        owned = GuardedCommand(
            process,
            cancellation_path,
            summary_path,
            evidence_path,
            launch_id,
            startup_path,
            tuple(command),
        )
        evidence.update(launch_pid=process.pid, status="launched")
        try:
            evidence_path.write_text(
                json.dumps(evidence, sort_keys=True) + "\n", encoding="utf-8"
            )
        except OSError as exc:
            exc.guard_command = owned
            try:
                owned.request_cancel()
                self.wait_owned(owned, timeout=5.0)
            except (OSError, RuntimeError, subprocess.SubprocessError) as cleanup_error:
                exc.add_note(f"guard cancellation failed: {cleanup_error}")
            exc.add_note(f"guard custody retained at {evidence_path}")
            raise
        return owned
