from __future__ import annotations

import functools
import os
import re
import uuid
import json
import subprocess
import sys
import textwrap
import threading
from enum import Enum
from contextlib import contextmanager
from collections.abc import Mapping, Sequence
from pathlib import Path
from collections.abc import Callable, Iterator
from typing import Any, TYPE_CHECKING

if TYPE_CHECKING:
    from multiprocessing.process import BaseProcess

from tools import harness_memory_guard
from molt.temporary_artifacts import new_guarded_directory
from molt.cargo_execution_policy import (
    default_nested_process_timeout_seconds,
    enforce_owning_proof_timeout,
)


class ModuleView:
    """One test's view of a shared module, installed only in modules under test.

    Process-custody tests replace ``os.getpid``, ``os.kill``, ``os.name`` and
    ``subprocess.Popen``, and remove ``os.killpg``. Those modules are
    process-wide: pytest and pathlib read ``os.name`` while a test runs (a
    spoofed name makes ``Path()`` raise and aborts the session), and the
    session sentinel's thread launches ``ps`` and records file-lock ownership
    with ``os.getpid()``. A patch or deletion on this view reaches only the
    modules it is installed in; every other reader sees the real module.
    """

    def __init__(self, real: object, **overrides: Any) -> None:
        object.__setattr__(self, "_real", real)
        object.__setattr__(self, "_hidden", set())
        for attr, value in overrides.items():
            object.__setattr__(self, attr, value)

    def __getattr__(self, attr: str) -> Any:
        if attr in self._hidden:
            raise AttributeError(attr)
        return getattr(self._real, attr)

    def __setattr__(self, attr: str, value: Any) -> None:
        self._hidden.discard(attr)
        object.__setattr__(self, attr, value)

    def __delattr__(self, attr: str) -> None:
        if attr in self.__dict__:
            object.__delattr__(self, attr)
        elif attr in self._hidden or not hasattr(self._real, attr):
            raise AttributeError(attr)
        self._hidden.add(attr)


# Python source for a child that stands in for a hung command: it ends only
# when its owner kills it. A finite sleep races the deadline under test. When
# the host stalls the owner past the sleep, the child exits 0 first and the
# owner reports success; a regrtest shard test with a 5 s deadline and a 60 s
# sleep failed so on 2026-10-21, while its guard waited out a 38-minute stall.
HUNG_CHILD_SOURCE = "import threading\nthreading.Event().wait()\n"


_HUNG_CHILD_MARKER = re.compile(r"molt-hung-child-[0-9a-f]{32}")


def hung_child_marker() -> str:
    """A token for one test's hung children to carry as an argv element.

    It is a fresh uuid, so no other process carries it: not a child of another
    session that runs the same test, and not a shell whose command line merely
    mentions the text. Carrying it is the cleanup's proof that the test
    started the process.
    """

    return f"molt-hung-child-{uuid.uuid4().hex}"


def processes_carrying(marker: str) -> list[int]:
    """Live processes that carry ``marker`` as a whole argv element.

    Only processes born after this test process are read: a hung child the
    code under test left behind may have been reparented, so it is no longer
    a descendant, but it is never older than the test that started it. A test
    that learns a hung descendant's pid only after the call under test
    returns cannot clean it up when that call raises; the marker finds it.
    """

    if not _HUNG_CHILD_MARKER.fullmatch(marker):
        raise ValueError(f"cleanup markers come from hung_child_marker(): {marker!r}")
    from molt import process_model
    from tools import memory_guard

    samples = memory_guard.sample_processes()
    own = samples.get(os.getpid())
    own_birth = None if own is None else own.started_at_ns
    found = []
    for pid, sample in samples.items():
        if pid == os.getpid():
            continue
        born = sample.started_at_ns
        if own_birth is not None and born is not None and born < own_birth:
            continue
        argv = process_model.process_command_argv(pid)
        if argv is not None and marker in argv:
            found.append(pid)
    return sorted(found)


def stop_test_processes_carrying(marker: str) -> None:
    """Stop every live process that carries ``marker`` (``processes_carrying``).

    A ``HUNG_CHILD_SOURCE`` descendant that the test does not own as a
    ``Popen`` never exits by itself; when the code under test leaves it
    running, the test stops it here. The marker and the birth time gate each
    signal, so a reused PID never receives it.
    """

    from molt import process_model
    from tools import memory_guard

    for pid in processes_carrying(marker):
        born = process_model.process_started_at_ns(pid)
        argv = process_model.process_command_argv(pid)
        if born is not None and argv is not None and marker in argv:
            memory_guard.terminate_verified_pid(
                pid, memory_guard.ProcessIdentity(born), grace=1.0
            )


def guard_custody_env() -> dict[str, str]:
    """Guard custody that a deliberately minimal child environment must carry.

    An explicit environment is the guard's complete authority, so a test that
    hands a guard a minimal environment would put its marker and scratch in
    the host's guard state root. The session fixture in ``tests/conftest.py``
    sets a session-scoped root; spread this into every minimal environment.
    """

    from molt.memory_guard_paths import STATE_ROOT_ENV

    return {STATE_ROOT_ENV: os.environ[STATE_ROOT_ENV]}


def keep_probe_commands_as_written(monkeypatch: Any) -> None:
    """For a test that fakes the toolchain probe launch itself.

    ``toolchain_probe.run_toolchain_probe`` finds a bare command in the probe's
    own environment before launch (that behavior has its own test). A test
    that fakes the launch compares commands as the policy spells them, so the
    lookup keeps each command as written there.
    """
    from pathlib import Path

    from molt import toolchain_identity

    monkeypatch.setattr(
        toolchain_identity,
        "find_executable",
        lambda command, *, environment, cwd=None: Path(command),
    )


def install_module_view(
    monkeypatch: Any, attribute: str, real: object, *modules: object, **overrides: Any
) -> ModuleView:
    """Give ``modules`` one shared private view of ``real`` for the current test."""

    view = ModuleView(real, **overrides)
    for module in modules:
        monkeypatch.setattr(module, attribute, view)
    return view


def install_module_os_view(
    monkeypatch: Any, *modules: object, name: str | None = None
) -> ModuleView:
    """Give ``modules`` one shared private ``os`` view for the current test."""

    overrides = {} if name is None else {"name": name}
    return install_module_view(monkeypatch, "os", os, *modules, **overrides)


def current_thread_only(
    replacement: Callable[..., Any], original: Callable[..., Any]
) -> Callable[..., Any]:
    """Apply a test double or probe only on the installing thread.

    Shared primitives such as file-lock acquire/release are also used by the
    in-process session sentinel thread. A double that records or injects
    failures must not count, fail or release that thread's handles. A probe
    observes only work on the installing thread: an operation that dispatches
    workers must own and instrument those workers itself.
    """

    owner = threading.get_ident()

    @functools.wraps(original)
    def dispatch(*args: Any, **kwargs: Any) -> Any:
        if threading.get_ident() == owner:
            return replacement(*args, **kwargs)
        return original(*args, **kwargs)

    return dispatch


def install_thread_module_view(
    monkeypatch: Any, attribute: str, real: object, *modules: object, **fakes: Any
) -> ModuleView:
    """Give ``modules`` one private view whose function fakes answer this thread.

    Use it when other threads of the pytest process also call the modules
    under test, as they call the memory-guard custody modules. A function
    fake answers only the installing thread; other threads reach the real
    function. A value fake (``sys.platform``), a class, or a fake for a
    function the host lacks (``os.killpg`` on Windows) cannot be scoped and
    answers every reader of the view.
    """

    scoped = {
        name: current_thread_only(fake, original)
        if callable(fake)
        and not isinstance(fake, type)
        and callable(original := getattr(real, name, None))
        else fake
        for name, fake in fakes.items()
    }
    return install_module_view(monkeypatch, attribute, real, *modules, **scoped)


class GuardedProcessRole(str, Enum):
    """Orthogonal operation role for a suite-family process guard."""

    BUILD = "build"
    EXECUTION = "execution"


_ALWAYS_BUILD_DRIVERS = frozenset(
    {
        "c++",
        "cc",
        "cl",
        "clang",
        "clang++",
        "gcc",
        "g++",
        "ld",
        "ld.lld",
        "link",
        "lld",
        "lld-link",
        "make",
        "gmake",
        "msbuild",
        "ninja",
        "rustc",
        "wasm-ld",
        "xcodebuild",
    }
)
_BUILD_SUBCOMMAND_DRIVERS = frozenset(
    {"cargo", "dotnet", "go", "molt", "npm", "pnpm", "swift", "yarn", "zig"}
)
_BUILD_FLAG_DRIVERS = frozenset({"cmake"})
_METADATA_PROBE_FLAGS = frozenset(
    {
        "--help",
        "--version",
        "-V",
        "-dumpmachine",
        "-dumpversion",
        "-h",
        "-v",
        "-vV",
        "/?",
        "/Bv",
    }
)


def _compiler_command_is_metadata_probe(
    executable: str, arguments: tuple[str, ...]
) -> bool:
    if not arguments:
        return True
    first = arguments[0]
    if first in _METADATA_PROBE_FLAGS or first.startswith("-print-"):
        return True
    return executable == "rustc" and first == "--print" and len(arguments) <= 2


def guarded_process_role(args: Sequence[str]) -> GuardedProcessRole:
    """Classify build/compiler work independently of its test-suite family.

    Classification happens on the realized command at the launch boundary, so
    import aliases and commands assembled in variables cannot bypass it.
    """

    command = tuple(str(arg) for arg in args)
    if not command:
        return GuardedProcessRole.EXECUTION
    executable = Path(command[0]).name.lower()
    if executable.endswith(".exe"):
        executable = executable[:-4]
    arguments = command[1:]
    if executable in _ALWAYS_BUILD_DRIVERS and not _compiler_command_is_metadata_probe(
        executable, arguments
    ):
        return GuardedProcessRole.BUILD
    if (
        executable in _BUILD_SUBCOMMAND_DRIVERS
        and len(command) >= 2
        and command[1] == "build"
    ):
        return GuardedProcessRole.BUILD
    if (
        executable in _BUILD_FLAG_DRIVERS
        and len(command) >= 2
        and command[1] == "--build"
    ):
        return GuardedProcessRole.BUILD
    if any(
        current in {"molt", "molt.cli"} and following == "build"
        for current, following in zip(command, command[1:], strict=False)
    ):
        return GuardedProcessRole.BUILD
    return GuardedProcessRole.EXECUTION


# The registered scope whose timeout bounds a role: building a test fixture is
# a `molt build`, so a build takes `MOLT_BUILD_TIMEOUT_SEC`. Execution has no
# scope beyond the test's own stem; its default comes from the role.
_ROLE_SCOPE: Mapping[GuardedProcessRole, str | None] = {
    GuardedProcessRole.BUILD: "MOLT_BUILD",
    GuardedProcessRole.EXECUTION: None,
}


def resolve_test_process_timeout(
    prefix: str,
    role: GuardedProcessRole,
    env: Mapping[str, str],
    *,
    explicit: float | None = None,
    default: float | None = None,
) -> float | None:
    """One budget for a fixture's build or first execution.

    Precedence: the explicit timeout, the test stem's `<STEM>_TIMEOUT_SEC`, the
    role scope's (`_ROLE_SCOPE`), then the root `MOLT_TEST_PROCESS_TIMEOUT_SEC`
    or the role default. Every name is a registered stem's; no stem is composed.
    """
    if explicit is not None:
        return explicit
    role_default = (
        default
        if default is not None
        else default_nested_process_timeout_seconds(role.value)
    )
    # The generic root fallback inside timeout_from_env waits until both
    # scopes have had precedence.
    for stem in (prefix, _ROLE_SCOPE[role]):
        if stem is None:
            continue
        raw = env.get(f"{stem}_TIMEOUT_SEC")
        if raw is None or not raw.strip():
            continue
        selected = harness_memory_guard.timeout_from_env(
            stem, {f"{stem}_TIMEOUT_SEC": raw}, default=role_default
        )
        return enforce_owning_proof_timeout(env, selected)
    selected = harness_memory_guard.timeout_from_env(prefix, env, default=role_default)
    return enforce_owning_proof_timeout(env, selected)


def _diagnostic_value(result: object, name: str) -> object:
    value = getattr(result, name, None)
    if hasattr(value, "to_dict"):
        return value.to_dict()
    return value


def _timeout_receipt(result: object) -> str:
    """Return bounded custody evidence without ever masking the timeout."""

    try:
        stderr = getattr(result, "stderr", "") or ""
        if isinstance(stderr, bytes):
            stderr = stderr.decode("utf-8", errors="replace")
        payload = {
            "schema": "molt.test-process-timeout.v1",
            "timed_out": _diagnostic_value(result, "timed_out"),
            "child_process": _diagnostic_value(result, "child_process"),
            "termination_reports": _diagnostic_value(result, "termination_reports"),
            "orphaned_process_groups": _diagnostic_value(
                result, "orphaned_process_groups"
            ),
            "peak": _diagnostic_value(result, "peak"),
            "peak_total": _diagnostic_value(result, "peak_total"),
            "stderr_tail": stderr[-4000:],
        }
        return json.dumps(payload, default=str, sort_keys=True)
    except BaseException as error:
        return json.dumps(
            {
                "schema": "molt.test-process-timeout.v1",
                "diagnostic_error": f"{type(error).__name__}: {error}",
            },
            sort_keys=True,
        )


# A bare interpreter name resolves through PATH, so its version is whatever the
# host put there (Apple's /usr/bin/python3 is 3.9 and exports xcrun state). A
# test runs Python as CPython's own tests do (Lib/test/support/script_helper.py
# always uses sys.executable): through sys.executable, or a target interpreter
# that molt.python_interpreter resolved and verified (HF-587).
_BARE_INTERPRETER = re.compile(r"python(?:\d+(?:\.\d+)?)?(?:\.exe)?", re.IGNORECASE)


def refuse_bare_interpreter(command: Sequence[str]) -> None:
    """Refuse an argv whose program is an interpreter named without a path."""
    if command and Path(command[0]).name == command[0]:
        if _BARE_INTERPRETER.fullmatch(command[0]) is not None:
            raise ValueError(
                f"test process names the interpreter {command[0]!r} without a path; "
                "use sys.executable or a resolved target interpreter (HF-587)"
            )


@contextmanager
def guarded_temporary_directory(*, prefix: str) -> Iterator[Path]:
    """Allocate helper scratch whose queue or guard owns terminal cleanup."""
    # Child-tree closure, not the Python context exit, owns reclamation.
    yield new_guarded_directory(os.environ, prefix=prefix)


def run_guarded_test_process(
    args: Sequence[str],
    *,
    prefix: str = "MOLT_PYTEST",
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    default_timeout: float | None = None,
    capture_output: bool = True,
    text: bool = True,
    check: bool = False,
    input: str | bytes | None = None,
    stdout: int | None = None,
    stderr: int | None = None,
    encoding: str = "utf-8",
    errors: str = "replace",
) -> harness_memory_guard.GuardedCompletedProcess:
    supported_streams = {None, subprocess.PIPE, subprocess.DEVNULL}
    if stdout not in supported_streams:
        raise ValueError("guarded test stdout must inherit, PIPE, or DEVNULL")
    if stderr not in supported_streams | {subprocess.STDOUT}:
        raise ValueError("guarded test stderr must inherit, PIPE, DEVNULL, or STDOUT")
    if capture_output and (stdout is not None or stderr is not None):
        raise ValueError("capture_output cannot be combined with stdout or stderr")
    if stderr == subprocess.STDOUT:
        raise ValueError(
            "guarded test commands preserve stdout/stderr separately; use an "
            "explicitly owned streaming process when interleaving is required"
        )
    command = list(args)
    refuse_bare_interpreter(command)
    process_env = os.environ if env is None else env
    role = guarded_process_role(command)
    resolved_timeout = resolve_test_process_timeout(
        prefix,
        role,
        process_env,
        explicit=timeout,
        default=default_timeout,
    )
    capture_streams = capture_output or stdout is not None or stderr is not None
    result = harness_memory_guard.guarded_completed_process(
        command,
        prefix=prefix,
        operation_role=role.value,
        cwd=cwd,
        env=process_env,
        input=input,
        capture_output=capture_streams,
        text=text,
        timeout=resolved_timeout,
        encoding=encoding,
        errors=errors,
    )
    if stderr == subprocess.DEVNULL:
        result.stderr = None
    if stdout == subprocess.DEVNULL:
        result.stdout = None
    if resolved_timeout is not None and bool(getattr(result, "timed_out", False)):
        error = subprocess.TimeoutExpired(
            command,
            resolved_timeout,
            output=result.stdout,
            stderr=result.stderr,
        )
        error.add_note(_timeout_receipt(result))
        setattr(error, "guarded_result", result)
        raise error
    if check and result.returncode != 0:
        error = subprocess.CalledProcessError(
            result.returncode,
            command,
            output=result.stdout,
            stderr=result.stderr,
        )
        setattr(error, "guarded_result", result)
        raise error
    return result


def run_isolated_python_probe(
    source: str,
    *,
    args: Sequence[str | Path] = (),
    payload: object = None,
) -> Any:
    """Run test-owned Python with JSON transport through the existing guard.

    The child starts no pytest session or sentinel. Its source owns setup and
    measurement boundaries; startup and transport are outside those boundaries.
    """
    root = Path(__file__).resolve().parents[1]
    env = dict(os.environ)
    env["PYTHONPATH"] = os.pathsep.join((str(root / "src"), str(root)))
    result = run_guarded_test_process(
        [sys.executable, "-c", textwrap.dedent(source), *map(str, args)],
        cwd=root,
        env=env,
        input=json.dumps(payload),
        check=True,
    )
    assert isinstance(result.stdout, str)
    return json.loads(result.stdout)


class _FreshGuardObserver:
    """Child-local identity and proof that the selected case actually ran."""

    def __init__(self) -> None:
        self.passed_calls = 0

    def pytest_runtest_logreport(self, report: Any) -> None:
        if report.when == "call" and report.passed:
            self.passed_calls += 1


def run_in_fresh_guard_observer(request: Any) -> bool:
    """Isolate small-cap guard controls from a loaded runner's lifetime RSS.

    Linux preserves ru_maxrss across exec, including the inherited pre-exec
    image. Run the exact pytest node with its real fixtures in a fresh observer
    before it launches the small-cap subject. The existing guard still owns
    the observer, and the subject's limits and rusage enforcement stay intact.
    """
    if os.name != "posix" or any(
        isinstance(plugin, _FreshGuardObserver)
        for plugin in request.config.pluginmanager.get_plugins()
    ):
        return False
    root = Path(request.config.rootpath)
    env = dict(os.environ)
    for key in ("PYTEST_ADDOPTS", "PYTEST_XDIST_WORKER", "PYTEST_XDIST_WORKER_COUNT"):
        env.pop(key, None)
    env["PYTHONPATH"] = os.pathsep.join((str(root / "src"), str(root)))
    # The node runs under its project's own configuration: pytest custody
    # refuses an addopts override, and the addopts load the custody plugins.
    args = ["-q", request.node.nodeid]
    source = (
        "import pytest; "
        "from tests.process_guard_common import _FreshGuardObserver; "
        "observer = _FreshGuardObserver(); "
        f"status = pytest.main({args!r}, plugins=[observer]); "
        "raise SystemExit(int(status) if status else "
        "(0 if observer.passed_calls == 1 else 1))"
    )
    result = run_guarded_test_process(
        [sys.executable, "-B", "-c", source], cwd=root, env=env, timeout=60.0
    )
    assert result.returncode == 0, result.stdout + result.stderr
    return True


def check_output_guarded_test_process(
    args: Sequence[str],
    **kwargs: object,
) -> str | bytes:
    """Return stdout from the canonical guarded test execution boundary."""

    if "stdout" in kwargs:
        raise ValueError("stdout is owned by check_output_guarded_test_process")
    result = run_guarded_test_process(
        args,
        capture_output=False,
        stdout=subprocess.PIPE,
        check=True,
        **kwargs,
    )
    assert result.stdout is not None
    return result.stdout


def run_custody_subject_process(
    args: Sequence[str],
    *,
    timeout: float = 120.0,
    **kwargs: Any,
) -> subprocess.CompletedProcess[Any]:
    """Run the exact child whose raw process behavior is the test subject.

    Custody/bootstrap tests must sometimes observe an unwrapped interpreter,
    Git process, or native supervisor. Centralizing that exceptional boundary
    keeps ordinary test subprocesses guarded while enforcing typed argv and a
    finite deadline on every custody-subject launch.
    """

    if isinstance(args, (str, bytes)):
        raise TypeError("custody subject command must be typed argv")
    command = [str(arg) for arg in args]
    if not command:
        raise ValueError("custody subject command must not be empty")
    if timeout <= 0:
        raise ValueError("custody subject timeout must be positive")
    if "timeout" in kwargs:
        raise ValueError("custody subject timeout is owned by the shared boundary")
    return subprocess.run(command, timeout=timeout, **kwargs)


def check_output_custody_subject_process(
    args: Sequence[str],
    *,
    timeout: float = 120.0,
    **kwargs: Any,
) -> str | bytes:
    if "stdout" in kwargs:
        raise ValueError("stdout is owned by check_output_custody_subject_process")
    result = run_custody_subject_process(
        args,
        timeout=timeout,
        stdout=subprocess.PIPE,
        check=True,
        **kwargs,
    )
    assert result.stdout is not None
    return result.stdout


def start_owned_test_process(
    args: Sequence[str],
    *,
    prefix: str = "MOLT_PYTEST",
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    stdin: int | None = None,
    stdout: int | None = None,
    stderr: int | None = None,
    text: bool = False,
    encoding: str | None = None,
    errors: str | None = None,
    creationflags: int = 0,
) -> subprocess.Popen[Any]:
    """Start one caller-owned direct child with platform process-group custody."""

    command = list(args)
    refuse_bare_interpreter(command)
    run_env = harness_memory_guard.canonical_harness_env(env)
    limits = harness_memory_guard.limits_from_env(prefix, run_env)
    group_kwargs = harness_memory_guard.batch_process_group_kwargs(
        limits,
        env=run_env,
    )
    if creationflags:
        group_kwargs["creationflags"] = int(group_kwargs.get("creationflags", 0)) | int(
            creationflags
        )
    return subprocess.Popen(
        command,
        cwd=cwd,
        env=run_env,
        stdin=stdin,
        stdout=stdout,
        stderr=stderr,
        text=text,
        encoding=encoding,
        errors=errors,
        **group_kwargs,
    )


def child_exited_unreaped(process: subprocess.Popen[Any]) -> bool:
    """Whether the kernel reports this POSIX child exited; never reaps it.

    Both oracles are independent of ``molt.process_model``, and the child
    stays a zombie for its owner. Linux asks the kernel's own wait
    (``waitid`` with ``WNOWAIT``), which reports a thread group only when every
    thread has exited, not when its leader has. Python 3.12 has no ``waitid``
    on macOS, so Darwin reads the state ``ps(1)`` reports.
    """

    if sys.platform == "darwin":
        state = check_output_custody_subject_process(
            ["ps", "-o", "stat=", "-p", str(process.pid)], text=True
        )
        return state.strip().startswith("Z")
    flags = os.WEXITED | os.WNOWAIT | os.WNOHANG
    return os.waitid(os.P_PID, process.pid, flags) is not None


def close_owned_test_process(process: subprocess.Popen[Any]) -> None:
    """Close exactly the process group created by start_owned_test_process."""

    if process.poll() is None:
        harness_memory_guard.force_close_process_group(process)
    process.wait()


def close_owned_multiprocessing_child(child: BaseProcess) -> None:
    """Reap only the caller's directly created multiprocessing observer.

    BaseProcess checks its creator PID before is_alive/join. The child remains
    unreaped until this owner joins it; cleanup never signals a process group.
    """
    if child.is_alive():
        child.terminate()
    child.join(10)
    if child.is_alive():
        child.kill()
        child.join(10)
    assert not child.is_alive(), "owned multiprocessing observer did not stop"
    child.close()


def guard_result_for_test(
    command: Sequence[str],
    returncode: int,
    stdout: str,
    stderr: str,
    *,
    descendants_closed: bool,
    elapsed_s: float | None = None,
) -> harness_memory_guard.GuardedCompletedProcess[str]:
    """Declare a synthetic terminal guard fact explicitly at a mock boundary."""
    return harness_memory_guard.GuardedCompletedProcess(
        command,
        returncode,
        stdout,
        stderr,
        elapsed_s=elapsed_s,
        child_stderr=stderr,
        descendants_closed=descendants_closed,
        child_returncode=returncode,
    )
