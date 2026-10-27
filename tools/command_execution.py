"""Canonical typed-argv execution authority for repository tools.

Tool modules bind one :class:`CommandExecutor` from their ``__file__``. Every
completed child then receives a stable telemetry/memory prefix, canonical DX
environment, timeout custody, and subprocess-compatible check/output behavior.
"""

from __future__ import annotations

import atexit
import hashlib
import json
import os
import time
import uuid
from datetime import datetime, timezone
import re
import subprocess
import sys
from collections.abc import Generator, Mapping, Sequence
from dataclasses import dataclass, field
from contextlib import ExitStack, contextmanager
from functools import lru_cache
from pathlib import Path
from typing import IO, Any

from molt.process_spawn import PROCESS_STATE_HANDOFF_TIMEOUT_S, ProcessStateUse

from tools.import_file import (
    bind_repository_imports,
    load_sibling_package_module_from_path,
)

from tools.memory_guard_core.cli_contract import guardian_argv


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
        "cat-file",
        "check-attr",
        "cherry",
        "diff",
        "for-each-ref",
        "grep",
        "log",
        "ls-files",
        "ls-tree",
        "merge-base",
        "rev-list",
        "rev-parse",
        "show",
        "show-ref",
        "status",
    }
)
# Subcommand families whose named action only reads.
_READ_ONLY_GIT_ACTIONS = frozenset({("worktree", "list")})
# Global git options that only shape how a read proceeds. None of them can
# name a hook, a pager, or a config value, so the subcommand after them keeps
# its class. ``-c key=value`` stays outside this set: it can arm
# ``core.fsmonitor`` or ``core.pager`` and so turns any read into a launch.
_INERT_GIT_GLOBAL_OPTIONS = frozenset({"--no-optional-locks", "--no-pager"})
_VERSION_FLAGS = frozenset({"--help", "--version", "-V", "-vV", "-version"})
# Git's own version report. `git --help` stays guarded: it opens a pager.
_GIT_VERSION_REQUESTS = frozenset({"--version", "version"})
_SYMBOLIC_REF_QUERY_FLAGS = frozenset(
    {"-q", "--quiet", "--short", "--recurse", "--no-recurse"}
)


def _is_git_symbolic_ref_query(arguments: Sequence[str]) -> bool:
    """Admit only the one-ref read, never assignment or deletion.

    https://git-scm.com/docs/git-symbolic-ref separates one-argument reads
    from two-argument writes and the explicit deletion/reflog options.
    """
    options = True
    operands = 0
    for argument in arguments:
        if options and argument == "--":
            options = False
        elif options and argument in _SYMBOLIC_REF_QUERY_FLAGS:
            continue
        elif not argument or argument.startswith("-"):
            return False
        else:
            operands += 1
    return operands == 1


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
        # Git also interprets `<subcommand> --help` as a documentation launch.
        # Keep pager/man execution guarded, including read-only subcommands.
        if "--help" in command[index:]:
            return False
        subcommand = command[index]
        if subcommand == "symbolic-ref":
            return _is_git_symbolic_ref_query(command[index + 1 :])
        if subcommand in _READ_ONLY_GIT_SUBCOMMANDS | _GIT_VERSION_REQUESTS:
            return True
        if tuple(command[index : index + 2]) in _READ_ONLY_GIT_ACTIONS:
            return True
        return subcommand == "config" and "--get" in command[index + 1 :]
    return any(flag in _VERSION_FLAGS for flag in command[1:])


# One observation window. It renews while the guard reports closure
# progress, so it bounds a stalled guard, not a slow one.
GUARD_CANCELLATION_OBSERVATION_SECONDS = 5.0


def detached_worker_pid(ready_path: Path, use_token: str) -> int | None:
    """Read the actual worker identity bound to this one-use launch capability."""
    if re.fullmatch(r"[0-9a-f]{32}", use_token) is None:
        raise ValueError("invalid detached use token")
    try:
        ready = json.loads(ready_path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    if (
        not isinstance(ready, dict)
        or set(ready) != {"schema_version", "pid", "use_token"}
        or type(ready.get("schema_version")) is not int
        or ready["schema_version"] != 1
        or ready["use_token"] != use_token
        or type(ready.get("pid")) is not int
        or ready["pid"] <= 0
    ):
        raise RuntimeError(f"invalid use handoff at {ready_path}")
    return ready["pid"]


@dataclass(slots=True)
class DetachedStateHandoff(ProcessStateUse):
    """Overlap managed-state use across an existing detached process launch.

    The caller still creates its process. This owner protects interpreter
    startup until that process acknowledges its own continuous shared use.
    Failed observation keeps the owner alive through actual child closure.
    """

    ready_path: Path
    use_token: str = field(default_factory=lambda: uuid.uuid4().hex)
    worker_pid: int | None = field(default=None, init=False)

    def wait_ready(self, *, timeout: float = PROCESS_STATE_HANDOFF_TIMEOUT_S) -> None:
        if self.process is None:
            raise RuntimeError("detached process has not been launched")
        deadline = time.monotonic() + timeout
        # The one-use launch capability binds this acknowledgement. A selected
        # interpreter may delegate (notably a Windows venv redirector), so the
        # actual worker PID is distinct from the Popen launcher we still own.
        while True:
            worker_pid = detached_worker_pid(self.ready_path, self.use_token)
            if worker_pid is not None:
                self.worker_pid = worker_pid
                self._release()
                return
            returncode = self.process.poll()
            if returncode is not None:
                raise RuntimeError(
                    f"worker exited {returncode} before use handoff at {self.ready_path}"
                )
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError(
                    f"worker use handoff unresolved at {self.ready_path}"
                )
            time.sleep(min(0.1, remaining))


@contextmanager
def detached_command_use(
    source_root: Path,
    environment: Mapping[str, str],
    *,
    cwd: Path,
    ready_path: Path,
    use_token: str,
) -> Generator[None]:
    """Acknowledge only after acquiring the detached worker's actual use."""
    from molt.checkout_state import checkout_state_environment_roots, checkout_state_use

    if not ready_path.is_absolute() or re.fullmatch(r"[0-9a-f]{32}", use_token) is None:
        raise ValueError("invalid detached use handoff descriptor")
    with checkout_state_use(
        checkout_state_environment_roots(source_root, environment, cwd=cwd)
    ):
        temporary = ready_path.with_name(f".{ready_path.name}.{use_token}.tmp")
        try:
            temporary.write_text(
                json.dumps(
                    {"schema_version": 1, "pid": os.getpid(), "use_token": use_token}
                ),
                encoding="utf-8",
            )
            os.replace(temporary, ready_path)
        finally:
            temporary.unlink(missing_ok=True)
        yield


@dataclass(slots=True, weakref_slot=True)
class GuardedCommand:
    """Launch handle and admitted guard identity, with cancellation custody.

    Only the guard may terminate its sampled/Job-owned child tree. The caller
    requests cancellation and waits; a stalled owner keeps its evidence and
    handle instead of being killed while a child might still be alive.
    """

    process: subprocess.Popen[Any] | None
    cancellation_path: Path
    summary_path: Path
    evidence_path: Path
    launch_id: str
    startup_path: Path
    command: tuple[str, ...]
    terminal: bool = False
    guard_pid: int | None = None
    child_identity: dict[str, object] | None = None
    progress_path: Path | None = None
    state_use: ExitStack | None = field(default=None, repr=False)

    def __post_init__(self) -> None:
        # Publish a durable cleanup owner before child creation. An exception
        # retaining this object alone is insufficient: GC can finalize its
        # generator-backed use context while the guard still owns a live tree.
        if self.state_use is not None:
            atexit.register(self._close_at_exit)

    def _close_at_exit(self) -> None:
        if self.process is None:
            self._release_state_use()
            return
        if not self.terminal:
            try:
                self.cancel_and_wait()
            except Exception as error:
                print(
                    f"guard closure unresolved at {self.evidence_path}: {error}",
                    file=sys.stderr,
                )

    def _release_state_use(self) -> None:
        if self.state_use is not None:
            self.state_use.close()
            self.state_use = None
            atexit.unregister(self._close_at_exit)

    def _require_process(self) -> subprocess.Popen[Any]:
        if self.process is None:
            raise RuntimeError("guard process has not been launched")
        return self.process

    @property
    def stdin(self):
        return self._require_process().stdin

    @property
    def stdout(self):
        return self._require_process().stdout

    @property
    def stderr(self):
        return self._require_process().stderr

    @property
    def pid(self) -> int:
        """The owned launcher PID; admitted guard_pid can be different."""
        return self._require_process().pid

    @property
    def args(self):
        return self._require_process().args

    @property
    def returncode(self):
        return self._require_process().returncode

    def poll(self):
        return self._require_process().poll()

    def request_cancel(self) -> None:
        # The exclusive launch directory is the capability. No PID lookup,
        # signal, process-group kill, or authority transfer occurs here.
        try:
            with self.cancellation_path.open("x", encoding="utf-8") as handle:
                handle.write("cancel\n")
        except FileExistsError:
            pass

    def _progress_mark(self) -> tuple[int, float]:
        """Size and wall time of the guard's last closure progress notice."""
        if self.progress_path is None:
            return 0, 0.0
        try:
            status = self.progress_path.stat()
        except FileNotFoundError:
            return 0, 0.0
        return status.st_size, status.st_mtime

    def cancel_and_wait(
        self, *, timeout: float = GUARD_CANCELLATION_OBSERVATION_SECONDS
    ) -> int:
        """Observe requested closure until the guard stops progressing.

        The observation ends ``timeout`` seconds after the guard's last closure
        progress notice, as systemd renews a stop timeout on
        ``EXTEND_TIMEOUT_USEC``. A guard slowed by CPU load keeps its owner
        observing; only a guard that reports nothing for a whole window is a
        stalled observation. Expiry never retires the owner.
        """
        try:
            self.request_cancel()
            mark = self._progress_mark()
            window = timeout
            while True:
                try:
                    return self.wait(timeout=window)
                except subprocess.TimeoutExpired:
                    renewed = self._progress_mark()
                    if renewed[0] == mark[0]:
                        raise
                    mark = renewed
                    window = max(0.0, renewed[1] + timeout - time.time())
        except Exception as exc:
            setattr(exc, "guard_command", self)
            exc.add_note(f"guard custody retained at {self.evidence_path}")
            raise

    def wait(self, timeout: float | None = None) -> int:
        self.terminal = False
        result = int(self._require_process().wait(timeout=timeout))
        try:
            startup = json.loads(self.startup_path.read_text(encoding="utf-8"))
            payload = json.loads(self.summary_path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            raise RuntimeError(
                f"guard exited with status {result} without startup/terminal "
                f"child custody; inspect {self.evidence_path}"
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
        if valid_startup:
            self.guard_pid = startup["guard_pid"]
            self.child_identity = startup["child_process"]
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
        self._release_state_use()
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
        if env is not None:
            from molt.process_environment import (
                ProcessEnvironment,
                ProcessEnvironmentBuilder,
            )

            # The isolated guard owns its types. Carry the closed rule value
            # with the mapping instead of inferring it again from this host.
            rule = (
                process_guard.NameRule(env.rule.value)
                if isinstance(env, (ProcessEnvironment, ProcessEnvironmentBuilder))
                else None
            )
            env = process_guard.ProcessEnvironment(env, rule=rule)
        return process_guard.run_completed_command(
            command,
            guard_source_root=self.repo_root,
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
            # A read that writes evidence files is a recorded execution, not a
            # bounded probe: `git cat-file --batch` spools every source blob.
            memory_guard_prefix=(
                None
                if _is_bounded_metadata_probe(command)
                and stdout_capture_path is None
                and stderr_capture_path is None
                else self.prefix
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
        """Start one explicitly caller-owned typed-argv process.

        The caller retains its command-aware ProcessStateUse (or equivalent
        guard/exec/detached owner) through observed child closure. This low-level
        method does not add a background waiter or take that lifetime ownership.
        """

        if isinstance(args, (str, bytes)):
            raise TypeError("command must be typed argv, not shell text")
        command = [str(part) for part in args]
        if not command:
            raise ValueError("command argv must not be empty")
        harness_memory_guard = _harness_memory_guard()
        env, _cargo_policies = harness_memory_guard.cargo_subprocess_environment(
            command,
            env,
            cwd=cwd,
        )
        process_guard = _process_guard_authority(str(self.repo_root))
        process = subprocess.Popen(
            process_guard.native_popen_command(command),
            cwd=cwd,
            env=None if env is None else env.child_block(),
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
        # Native shell serialization is an OS-boundary detail, not a change to
        # the caller-owned child's typed argv contract.
        process.args = command
        return process

    def wait_owned(
        self,
        process: subprocess.Popen[Any] | GuardedCommand,
        *,
        timeout: float,
        terminate_timeout: float = GUARD_CANCELLATION_OBSERVATION_SECONDS,
    ) -> int:
        """Wait finitely and clean only the exact process this caller owns."""

        if timeout <= 0 or terminate_timeout <= 0:
            raise ValueError("owned process timeouts must be positive")
        try:
            return int(process.wait(timeout=timeout))
        except subprocess.TimeoutExpired as timeout_error:
            if isinstance(process, GuardedCommand):
                try:
                    process.cancel_and_wait(timeout=terminate_timeout)
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
                    setattr(timeout_error, "guard_command", process)
                    setattr(timeout_error, "cleanup_error", cleanup_error)
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
        harness: bool = False,
    ) -> GuardedCommand:
        """Start an interactive command with admitted actual-worker custody."""

        if timeout is not None and timeout <= 0:
            raise ValueError("timeout must be positive")
        if isinstance(args, (str, bytes)):
            raise TypeError("command must be typed argv, not shell text")
        command = [str(part) for part in args]
        if not command:
            raise ValueError("command argv must not be empty")
        harness_memory_guard = _harness_memory_guard()
        if harness:
            command = harness_memory_guard.memory_guard._resolve_relative_executable(
                command, parent_cwd=self.repo_root
            )
        env, _cargo_policies = harness_memory_guard.cargo_subprocess_environment(
            command,
            env,
            cwd=cwd,
        )
        if harness:
            launch_environment = harness_memory_guard.canonical_harness_env(
                env, repo_root=self.repo_root
            )
        else:
            context = harness_memory_guard.HarnessExecutionContext.from_env(
                self.prefix, env, repo_root=self.repo_root
            )
            launch_environment = context.env
            limits = context.limits
        from molt.memory_guard_paths import memory_guard_state_root

        launch_id = uuid.uuid4().hex
        custody = (
            memory_guard_state_root(self.repo_root, launch_environment)
            / "commands"
            / launch_id
        )
        custody.mkdir(parents=True, exist_ok=False)
        # Persistent command custody outlives failed closure and dropped handles.
        # Its empty startup namespace must survive for the same lifetime.
        verifier_bytecode_prefix = custody / "guardian-bytecode"
        verifier_bytecode_prefix.mkdir()
        cancellation_path = custody / "cancel"
        progress_path = custody / "progress"
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
        if harness:
            # The harness worker runs the same memory_guard owner in this process;
            # preserve its preflight, profiling and sentinel without a second guard.
            guarded_argv = guardian_argv(
                sys.executable,
                self.repo_root / "tools" / "guarded_exec.py",
                verifier_bytecode_prefix=verifier_bytecode_prefix,
            ) + [
                "--prefix",
                self.prefix,
                "--cwd",
                str(Path.cwd() if cwd is None else Path(cwd).resolve()),
                "--metrics-json",
                str(summary_path),
                "--cancel-file",
                str(cancellation_path),
                "--progress-file",
                str(progress_path),
            ]
            if timeout is not None:
                guarded_argv.extend(("--timeout", str(timeout)))
        else:
            guarded_argv = guardian_argv(
                sys.executable,
                self.repo_root / "tools" / "memory_guard.py",
                verifier_bytecode_prefix=verifier_bytecode_prefix,
            ) + [
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
                guarded_argv.extend(("--timeout", str(timeout)))
            guarded_argv.extend(
                (
                    "--summary-json",
                    str(summary_path),
                    "--cancel-file",
                    str(cancellation_path),
                    "--progress-file",
                    str(progress_path),
                )
            )
        # Use the existing hidden-command worker contract directly. On Windows
        # memory_guard's command-line facade otherwise adds a detached wrapper
        # whose Popen is not the worker that owns the Job or active marker.
        worker_environment = harness_memory_guard.memory_guard._worker_env(
            launch_environment,
            command,
            launch_id=launch_id,
            startup_json=str(startup_path),
        )
        from molt.checkout_state import checkout_state_command_roots, checkout_state_use

        state_use = ExitStack()
        state_use.enter_context(
            checkout_state_use(
                checkout_state_command_roots(
                    self.repo_root,
                    launch_environment,
                    command,
                    cwd=cwd,
                )
            )
        )
        try:
            owned = GuardedCommand(
                None,
                cancellation_path,
                summary_path,
                evidence_path,
                launch_id,
                startup_path,
                tuple(command),
                progress_path=progress_path,
                state_use=state_use,
            )
        except BaseException:
            state_use.close()
            raise
        try:
            process = self.start_owned(
                guarded_argv,
                cwd=self.repo_root if harness else cwd,
                env=worker_environment,
                stdin=stdin,
                stdout=stdout,
                stderr=stderr,
                text=text,
                encoding=encoding,
                errors=errors,
                bufsize=bufsize,
            )
            owned.process = process
        except BaseException:
            # No child was returned by the launch owner. Release the admission
            # and remove its pre-registered cleanup callback.
            owned._release_state_use()
            raise
        evidence.update(launch_pid=process.pid, status="launched")
        try:
            evidence_path.write_text(
                json.dumps(evidence, sort_keys=True) + "\n", encoding="utf-8"
            )
        except OSError as exc:
            setattr(exc, "guard_command", owned)
            try:
                owned.request_cancel()
                self.wait_owned(owned, timeout=5.0)
            except (OSError, RuntimeError, subprocess.SubprocessError) as cleanup_error:
                setattr(exc, "cleanup_error", cleanup_error)
                exc.add_note(f"guard cancellation unresolved: {cleanup_error}")
            exc.add_note(f"guard custody retained at {evidence_path}")
            raise
        return owned
