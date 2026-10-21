from __future__ import annotations

# tools/command_execution.py loads this file under a per-checkout neutral
# package (tools/import_file.py require_repository_identity, HF-223).
__molt_neutral_authority__ = True

import os
import shlex
import subprocess
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any, Protocol

from .cargo_execution_policy import cargo_subprocess_environment
from .process_environment import NameRule as NameRule
from .process_environment import ProcessEnvironment, ProcessEnvironmentBuilder
from .source_root import compiler_source_root
from .process_spawn import ProcessStateUse, native_popen_command


CLI_MEMORY_GUARD_PREFIX = "MOLT_CLI"
DEFAULT_UNGUARDED_PROBE_TIMEOUT_SECONDS = 30.0

_MEMORY_GUARD_ENV_SUFFIXES = (
    "MEMORY_GUARD",
    "MEMORY_GUARD_POLL_SEC",
    "MAX_PROCESS_RSS_GB",
    "MAX_RSS_GB",
    "MAX_TOTAL_RSS_GB",
    "MAX_TREE_RSS_GB",
    "GLOBAL_RSS_LIMIT_GB",
    "MAX_GLOBAL_RSS_GB",
    "CHILD_RLIMIT_GB",
    "MAX_CHILD_RLIMIT_GB",
    "TOTAL_MEMORY_GB",
    "MEMORY_TOTAL_GB",
    "MEM_AVAILABLE_GB",
    "MEMORY_AVAILABLE_GB",
    "MEMORY_RESERVE_GB",
    "MEM_RESERVE_GB",
)


class GuardLoader(Protocol):
    def __call__(
        self, cwd: Path | None, *, guard_source_root: Path | None = None
    ) -> Any: ...


class GuardInfrastructureError(subprocess.SubprocessError):
    """The memory guard failed around a child it ran.

    This is not the child's exit status. The guard reports its own failure (for
    example incomplete temporary-artifact custody) with exit code 125, so a
    caller that reads ``returncode`` would blame the child. This error carries
    no ``returncode``: ``child_returncode`` is what the child itself returned,
    or None when the guard did not observe it.
    """

    def __init__(self, command: Sequence[str], result: Any) -> None:
        failure = result.infrastructure_failure
        self.command = tuple(str(part) for part in command)
        self.phase: str = failure.phase
        self.details: tuple[str, ...] = tuple(failure.details)
        self.child_returncode: int | None = result.child_returncode
        # Telemetry consumers keep the guard's complete terminal record.
        self.guarded_result = result
        child = (
            "the child's exit status is unknown"
            if self.child_returncode is None
            else f"the child exited with {self.child_returncode}"
        )
        super().__init__(
            f"memory guard infrastructure failure ({self.phase}) while running "
            f"{shlex.join(self.command)}: {'; '.join(self.details)}; "
            f"{child}"
        )


def guard_infrastructure_exit_code() -> int:
    """The guard's own exit code for its infrastructure failures."""
    return int(load_harness_memory_guard(None).memory_guard.INFRASTRUCTURE_RETURN_CODE)


def raise_for_guard_outcome(
    command: Sequence[str], result: Any, *, timeout: float | None
) -> None:
    """Raise for outcomes the guard owns, so returncode stays the child's.

    The guard reports a timeout as 124 and its own failure as 125. A caller of
    a subprocess-compatible runner must never read either as the child's exit
    status, so both become exceptions here: ``subprocess.TimeoutExpired`` and
    ``GuardInfrastructureError``. Each carries the guard's terminal record as
    ``guarded_result`` (Cargo test receipts and benchmarks read its RSS samples).
    """
    command = [str(part) for part in command]
    if bool(getattr(result, "timed_out", False)):
        if timeout is None:
            raise RuntimeError(
                "guarded subprocess reported a timeout without a requested "
                "timeout; timeout custody is inconsistent"
            )
        error = subprocess.TimeoutExpired(
            command,
            timeout,
            output=result.stdout,
            stderr=result.stderr,
        )
        setattr(error, "guarded_result", result)
        raise error
    if getattr(result, "infrastructure_failure", None) is not None:
        raise GuardInfrastructureError(command, result)


def _command_source_root(guard_source_root: Path | None = None) -> Path:
    if guard_source_root is not None and not guard_source_root.is_absolute():
        raise ValueError("guard source owner must be an absolute path")
    return (
        compiler_source_root() if guard_source_root is None else guard_source_root
    ).resolve()


def _molt_repo_root(guard_source_root: Path | None = None) -> Path:
    root = _command_source_root(guard_source_root)
    if (
        not (root / "pyproject.toml").is_file()
        or not (root / "tools" / "harness_memory_guard.py").is_file()
    ):
        raise RuntimeError(
            "guarded execution requires the selected Molt compiler sources; "
            f"guard tools are unavailable under {root}"
        )
    return root


def load_harness_memory_guard(
    cwd: Path | None, *, guard_source_root: Path | None = None
) -> Any:
    del cwd  # A command directory cannot authorize a different guard module.
    root = _molt_repo_root(guard_source_root)
    # Tools import as `tools.<module>` (HF-F217); `tools/` itself never joins
    # sys.path, where tools/profile.py would shadow the stdlib `profile`.
    if str(root) not in sys.path:
        sys.path.insert(0, str(root))
    try:
        from tools import harness_memory_guard
    except ModuleNotFoundError as exc:
        raise RuntimeError(
            f"memory guard helper is required for guarded subprocesses: {exc}"
        ) from exc
    # Never replace a live foreign guard: it may own processes and suppression
    # state. Reject the mixed source authority before any child is launched.
    for module, relative in (
        (harness_memory_guard, "harness_memory_guard.py"),
        (getattr(harness_memory_guard, "memory_guard", None), "memory_guard.py"),
        (
            getattr(harness_memory_guard, "process_sentinel", None),
            "process_sentinel.py",
        ),
    ):
        actual = getattr(module, "__file__", None)
        expected = root / "tools" / relative
        if actual is None or Path(actual).resolve() != expected.resolve():
            raise RuntimeError(
                f"guard source authority mismatch: expected {expected}, loaded {actual!r}; "
                "use a fresh process bound to one Molt checkout"
            )
    return harness_memory_guard


def with_memory_guard_env(
    env: ProcessEnvironment | None,
    memory_guard_prefix: str,
) -> ProcessEnvironment | None:
    if env is None:
        return None
    merged = ProcessEnvironmentBuilder(env)
    inherited = ProcessEnvironment(os.environ, rule=env.rule)
    normalized = memory_guard_prefix.strip().upper().rstrip("_")
    names: list[str] = []
    if normalized:
        names.extend(f"{normalized}_{suffix}" for suffix in _MEMORY_GUARD_ENV_SUFFIXES)
    names.extend(f"MOLT_{suffix}" for suffix in _MEMORY_GUARD_ENV_SUFFIXES)
    for name in dict.fromkeys(names):
        if name not in merged and name in inherited:
            merged[name] = inherited[name]
    return ProcessEnvironment(merged)


def timeout_from_env(
    memory_guard_prefix: str,
    env: Mapping[str, str] | None,
    *,
    explicit: float | None = None,
    default: float | None = None,
    guard_loader: GuardLoader = load_harness_memory_guard,
    cwd: Path | None = None,
) -> float | None:
    harness_memory_guard = guard_loader(cwd)
    return harness_memory_guard.timeout_from_env(
        memory_guard_prefix,
        env,
        explicit=explicit,
        default=default,
    )


def run_completed_command(
    cmd: Sequence[str],
    *,
    env: ProcessEnvironment | None = None,
    cwd: str | Path | None = None,
    capture_output: bool = False,
    stdout_capture_path: str | Path | None = None,
    stderr_capture_path: str | Path | None = None,
    capture_tail_bytes: int | None = None,
    memory_guard_prefix: str | None = CLI_MEMORY_GUARD_PREFIX,
    input: str | bytes | None = None,
    timeout: float | None = None,
    text: bool | None = True,
    check: bool = False,
    stdout: int | None = None,
    stderr: int | None = None,
    encoding: str | None = None,
    errors: str | None = None,
    progress_label: str | None = None,
    guard_loader: GuardLoader = load_harness_memory_guard,
    guard_source_root: Path | None = None,
) -> subprocess.CompletedProcess[Any]:
    if isinstance(cmd, (str, bytes)):
        raise TypeError("command must be typed argv, not shell text")
    command = [str(part) for part in cmd]
    if not command:
        raise ValueError("command argv must not be empty")
    env, _cargo_policies = cargo_subprocess_environment(command, env)
    if capture_output and (stdout is not None or stderr is not None):
        raise ValueError("capture_output cannot be combined with stdout or stderr")
    supported_streams = {None, subprocess.PIPE, subprocess.DEVNULL}
    if stdout not in supported_streams:
        raise ValueError("stdout must inherit, PIPE, or DEVNULL")
    if stderr not in supported_streams | {subprocess.STDOUT}:
        raise ValueError("stderr must inherit, PIPE, DEVNULL, or STDOUT")
    text_mode = bool(text or encoding is not None or errors is not None)
    if memory_guard_prefix is None:
        if stdout_capture_path is not None or stderr_capture_path is not None:
            raise ValueError("evidence capture paths require guarded execution")
        probe_timeout = (
            DEFAULT_UNGUARDED_PROBE_TIMEOUT_SECONDS if timeout is None else timeout
        )
        with ProcessStateUse(
            _command_source_root(guard_source_root),
            os.environ if env is None else env,
            cwd,
        ) as state_owner:
            process = subprocess.Popen(
                native_popen_command(command),
                env=env.child_block() if env is not None else None,
                cwd=cwd,
                stdin=subprocess.PIPE if input is not None else None,
                stdout=subprocess.PIPE if capture_output else stdout,
                stderr=subprocess.PIPE if capture_output else stderr,
                text=text_mode,
                encoding=encoding,
                errors=errors,
            )
            state_owner.process = process
            error: BaseException | None = None
            try:
                try:
                    output, diagnostic = process.communicate(
                        input, timeout=probe_timeout
                    )
                except subprocess.TimeoutExpired as failure:
                    try:
                        process.kill()
                        if os.name == "nt":
                            failure.stdout, failure.stderr = process.communicate()
                        else:
                            process.wait()
                    except BaseException as cleanup_error:
                        failure.add_note(
                            f"timed-out child cleanup failed: {cleanup_error!r}"
                        )
                    raise
                except BaseException as failure:
                    try:
                        process.kill()
                    except BaseException as cleanup_error:
                        failure.add_note(f"child kill failed: {cleanup_error!r}")
                    raise
                returncode = process.poll()
                if returncode is None:
                    raise RuntimeError("completed probe has no observed exit status")
                if check and returncode:
                    raise subprocess.CalledProcessError(
                        returncode, process.args, output=output, stderr=diagnostic
                    )
                return subprocess.CompletedProcess(
                    command, returncode, output, diagnostic
                )
            except BaseException as failure:
                error = failure
                raise
            finally:
                # Match Popen's native stream/interrupt/wait policy, while
                # keeping an earlier error if that physical cleanup fails.
                try:
                    process.__exit__(
                        type(error) if error is not None else None,
                        error,
                        error.__traceback__ if error is not None else None,
                    )
                except BaseException as cleanup_error:
                    if error is None:
                        raise
                    error.add_note(f"child context cleanup failed: {cleanup_error!r}")
    if stderr == subprocess.STDOUT:
        raise ValueError(
            "guarded completed commands preserve stdout/stderr separately; "
            "use an explicitly owned streaming process when interleaving is required"
        )
    guard_env = with_memory_guard_env(env, memory_guard_prefix)
    cwd_path = None if cwd is None else Path(cwd)
    # Repository tools carry the owner admitted by import_file. Compiler input
    # selection remains caller input, and cannot redirect that control plane.
    owner_root = _command_source_root(guard_source_root)
    harness_memory_guard = guard_loader(cwd_path, guard_source_root=owner_root)
    if guard_env is not None:
        guard_env = harness_memory_guard.ProcessEnvironment(
            guard_env,
            rule=harness_memory_guard.NameRule(guard_env.rule.value),
        )
    guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
        memory_guard_prefix,
        guard_env,
        # The command's working directory is not its Molt source authority:
        # standalone Cargo workspaces and staged packages execute below or
        # outside the checkout while retaining this loaded guard's owner.
        repo_root=owner_root,
    )
    capture_streams = capture_output or stdout is not None or stderr is not None
    result = guard_context.run(
        command,
        cwd=cwd,
        input=input,  # type: ignore[arg-type]
        capture_output=capture_streams,
        stdout_capture_path=stdout_capture_path,
        stderr_capture_path=stderr_capture_path,
        capture_tail_bytes=capture_tail_bytes,
        text=text_mode,
        timeout=timeout,
        encoding=encoding or "utf-8",
        errors=errors or "strict",
        progress_label=progress_label,
    )
    if stderr == subprocess.DEVNULL:
        result.stderr = None
    if stdout == subprocess.DEVNULL:
        result.stdout = None
    raise_for_guard_outcome(command, result, timeout=timeout)
    if check and result.returncode != 0:
        error = subprocess.CalledProcessError(
            result.returncode,
            command,
            output=result.stdout,
            stderr=result.stderr,
        )
        # As in raise_for_guard_outcome: subprocess compatibility remains
        # intact, while callers retain the guard's terminal record.
        setattr(error, "guarded_result", result)
        raise error
    return result


def source_command_executor(prefix: str) -> Any:
    """Use the existing interactive process owner from this compiler's sources."""
    root = _molt_repo_root()
    load_harness_memory_guard(root)
    from tools import command_execution

    expected = root / "tools" / "command_execution.py"
    if Path(command_execution.__file__).resolve() != expected.resolve():
        raise RuntimeError("interactive command execution source authority mismatch")
    return command_execution.CommandExecutor(prefix=prefix, repo_root=root)
