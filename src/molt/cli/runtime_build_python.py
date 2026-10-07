"""One build operation's selected, live isolated build-Python admission."""

from __future__ import annotations

from contextlib import contextmanager
import copy
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
from typing import Generator, Mapping, TYPE_CHECKING

from molt import process_guard
from molt.cli.runtime_identity_schema import _BUILD_PYTHON_SCHEMA, _digest
from molt.exact_json import ExactJsonError, loads_exact
from molt.python_environment_identity import python_identity_probe_arguments
from molt.python_runtime_identity import validate_python_runtime_identity
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    resolve_executable,
    stable_executable_probe,
    _stat_identity,
)

if TYPE_CHECKING:
    from molt.cli.models import _RuntimeArtifactState


_RESPONSE_TIMEOUT = 30.0
# The admission response content-hashes the whole interpreter installation, so
# its cost scales with the install, not the protocol: 3.5k files / 63 MB take
# 5 s on a Windows workstation, while a hosted Linux CPython (test suite
# included) is several times larger and shares its runner with compiler builds.
_ADMISSION_TIMEOUT = 300.0
_RESPONSE_LIMIT = 64 * 1024 * 1024


def _loader_environment(env: Mapping[str, str]) -> tuple[tuple[str, str], ...]:
    """Isolation ignores Python startup options; native loader policy still applies."""
    return tuple(
        sorted(
            (key.upper() if os.name == "nt" else key, value)
            for key, value in env.items()
            if key.upper()
            in {
                "PATH",
                "SYSTEMROOT",
                "WINDIR",
                "__PYVENV_LAUNCHER__",
                "PYTHONEXECUTABLE",
            }
            or key.startswith(("LD_", "DYLD_"))
        )
    )


class BuildPythonCleanupError(ValueError):
    """Cleanup failure retaining the exact owner for inspection and retry."""

    def __init__(self, admission, cause: BaseException):
        self.admission = admission
        self.cleanup_error = cause
        evidence = admission.custody_path
        self.evidence_path = None if evidence is None else str(evidence)
        detail = f"; custody: {evidence}" if evidence is not None else ""
        super().__init__(f"runtime build Python cleanup failed: {cause}{detail}")


class BuildPythonAdmission:
    """Lazy, revocable session; no receipt or process survives its operation.

    Each capture independently selects the caller's interpreter and checks its
    exact entrypoint/generation and loader configuration. The producing Python
    retains PythonFileCaptureContext and verifies every live fence on reuse.
    Caller-owned copies keep the admitted semantic projection immutable.
    """

    def __init__(self) -> None:
        self._closed = False
        self._process = None
        self._executor = None
        self._stderr = None
        self._reader = None
        self._responses: queue.Queue[str | BaseException] = queue.Queue(maxsize=2)
        self._entrypoint_generation: tuple[int, ...] | None = None
        self._entrypoint: Path | None = None
        self._executable: StableRegularFileIdentity | None = None
        self._environment: tuple[tuple[str, str], ...] | None = None
        self._identity: dict[str, object] | None = None
        self._startup_selection: dict[str, object] | None = None
        self._primary_failure: object | None = None
        self.cleanup_failure: BuildPythonCleanupError | None = None
        self.custody_path: str | None = None
        self._failure_detail = ""
        self._protocol_failure: BaseException | None = None
        self._lock = threading.RLock()

    def __enter__(self) -> BuildPythonAdmission:
        if self._closed:
            raise ValueError("runtime build Python admission is revoked")
        return self

    @property
    def terminal(self) -> bool:
        return self._process is None

    def record_failure(self, primary: object = None) -> None:
        """Keep a caller's explicit False/nonzero outcome across nested scopes."""
        if self._primary_failure is None:
            self._primary_failure = (
                primary if primary is not None else "operation failed"
            )

    def _report_cleanup_failure(self, error: BuildPythonCleanupError, primary) -> None:
        if isinstance(primary, BaseException):
            primary.add_note(str(error))
        print(
            json.dumps(
                {
                    "kind": "build-python-cleanup-failure",
                    "primary_failure": str(primary),
                    "error": str(error),
                    "cleanup_notes": getattr(error.cleanup_error, "__notes__", []),
                    "custody": error.evidence_path,
                    "terminal": self.terminal,
                },
                sort_keys=True,
            ),
            file=sys.stderr,
        )

    def __exit__(self, exc_type, exc, traceback) -> None:
        if self.cleanup_failure is not None and self._primary_failure is not None:
            # capture already attempted and reported cleanup. A caller may retry
            # close explicitly; scope unwinding must not start a retry loop.
            return
        try:
            self.close()
        except BuildPythonCleanupError as cleanup:
            primary = exc if exc is not None else self._primary_failure
            if primary is None:
                raise
            self._report_cleanup_failure(cleanup, primary)

    def _read_response(self, timeout: float = _RESPONSE_TIMEOUT) -> str:
        try:
            response = self._responses.get(timeout=timeout)
        except queue.Empty as exc:
            raise ValueError(
                f"runtime build Python session response timed out after {timeout:g} s"
            ) from exc
        if self._protocol_failure is not None:
            raise ValueError(
                "runtime build Python session emitted unsolicited output"
            ) from self._protocol_failure
        if isinstance(response, BaseException):
            raise ValueError("runtime build Python identity probe failed") from response
        if self._process is None or self._process.poll() is not None:
            raise ValueError("runtime build Python session exited before admission")
        return response

    def _start(self, entrypoint: Path, env: Mapping[str, str]) -> dict[str, object]:
        self._executor = process_guard.source_command_executor("MOLT_BUILD_PYTHON")
        self._stderr = tempfile.TemporaryFile(mode="w+b")
        try:
            self._process = self._executor.start_guarded(
                [
                    str(entrypoint),
                    *python_identity_probe_arguments(
                        (
                            "--capture-runtime",
                            "--runtime-session",
                            "--hash-workers",
                            "4",
                        ),
                        no_site=True,
                    ),
                ],
                env=dict(env),
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=self._stderr,
                text=True,
                encoding="utf-8",
                errors="strict",
                bufsize=1,
            )
        except BaseException as exc:
            self._process = getattr(exc, "guard_command", None)
            evidence = getattr(self._process, "evidence_path", None)
            self.custody_path = None if evidence is None else str(evidence)
            raise
        process = self._process
        evidence = getattr(process, "evidence_path", None)
        self.custody_path = None if evidence is None else str(evidence)

        def read_responses() -> None:
            try:
                assert process.stdout is not None
                while True:
                    line = process.stdout.readline(_RESPONSE_LIMIT + 1)
                    if (
                        not line
                        or not line.endswith("\n")
                        or len(line) > _RESPONSE_LIMIT
                    ):
                        raise ValueError("runtime build Python session lost framing")
                    self._responses.put_nowait(line)
            except queue.Full as exc:
                # Unsolicited output cannot grow memory or leave a blocked
                # response thread after the operation revokes this session.
                self._protocol_failure = exc
                return
            except (OSError, ValueError, UnicodeError) as exc:
                try:
                    self._responses.put_nowait(exc)
                except queue.Full:
                    self._protocol_failure = exc

        self._reader = threading.Thread(
            target=read_responses, name="build-python-admission-response", daemon=True
        )
        self._reader.start()
        try:
            payload = loads_exact(self._read_response(_ADMISSION_TIMEOUT))
            if not isinstance(payload, dict) or set(payload) != {
                "runtime",
                "startup_selection",
            }:
                raise ValueError("runtime build Python session envelope is invalid")
            selection = payload["startup_selection"]
            if (
                not isinstance(selection, dict)
                or selection.get("schema") != "molt-python-startup-selection-v1"
            ):
                raise ValueError("runtime build Python startup selection is invalid")
            self._startup_selection = selection
            return validate_python_runtime_identity(payload["runtime"])
        except (json.JSONDecodeError, ExactJsonError) as exc:
            raise ValueError(
                "runtime build Python identity probe emitted invalid JSON"
            ) from exc

    def _verify_fresh_selection(self, entrypoint: Path, env: Mapping[str, str]) -> None:
        assert self._executor is not None
        # A launcher can consult any environment key, and unchanged search
        # directories can acquire higher-precedence loader providers. Only a
        # fresh launch observes those selections. This mode does no tree scan.
        completed = self._executor.run(
            [
                str(entrypoint),
                *python_identity_probe_arguments(
                    ("--runtime-selection",), no_site=True
                ),
            ],
            env=dict(env),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="strict",
            timeout=_RESPONSE_TIMEOUT,
            check=True,
        )
        if len(completed.stdout) > _RESPONSE_LIMIT:
            raise ValueError(
                "runtime build Python startup selection exceeds framing limit"
            )
        try:
            selected = loads_exact(completed.stdout)
        except (json.JSONDecodeError, ExactJsonError) as exc:
            raise ValueError(
                "runtime build Python fresh selection emitted invalid JSON"
            ) from exc
        if selected != self._startup_selection:
            raise ValueError(
                "runtime build Python fresh startup selection changed after admission"
            )

    def capture(self, env: Mapping[str, str]) -> dict[str, object]:
        with self._lock:
            if self._closed:
                raise ValueError("runtime build Python admission is revoked")
            try:
                command = (
                    env.get("MOLT_BUILD_PYTHON", "").strip()
                    or env.get("PYTHON", "").strip()
                    or ("python" if os.name == "nt" else "python3")
                )
                path = resolve_executable(
                    command, environment=env, label="runtime build Python"
                )
                policy = _loader_environment(env)
                with stable_executable_probe(
                    path, label="runtime build Python", identity=self._executable
                ) as (entrypoint, executable):
                    entrypoint_generation = _stat_identity(entrypoint.lstat())
                    if self._identity is None:
                        runtime = self._start(entrypoint, env)
                        self._entrypoint_generation = entrypoint_generation
                        self._entrypoint = entrypoint
                        self._executable = executable
                        self._environment = policy
                        material = {
                            "schema": _BUILD_PYTHON_SCHEMA,
                            "logical_name": "build_python",
                            "selected_executable": {
                                "entrypoint": entrypoint.name.casefold()
                                if os.name == "nt"
                                else entrypoint.name,
                                "content_filename": executable.path.name.casefold()
                                if os.name == "nt"
                                else executable.path.name,
                                "size": executable.size,
                                "sha256": executable.sha256,
                            },
                            "runtime": runtime,
                        }
                        self._identity = {
                            **material,
                            "identity_sha256": _digest(material),
                        }
                    else:
                        if (
                            entrypoint != self._entrypoint
                            or entrypoint_generation != self._entrypoint_generation
                            or policy != self._environment
                        ):
                            raise ValueError(
                                "runtime build Python selection/configuration changed after admission"
                            )
                        process = self._process
                        if (
                            process is None
                            or process.poll() is not None
                            or process.stdin is None
                        ):
                            raise ValueError("runtime build Python session is not live")
                        self._verify_fresh_selection(entrypoint, env)
                        # Content and topology fences run after fresh selection,
                        # before returning the admitted receipt to its consumer.
                        process.stdin.write("verify\n")
                        process.stdin.flush()
                        runtime = self._identity["runtime"]
                        assert isinstance(runtime, dict)
                        if (
                            self._read_response().strip()
                            != runtime["runtime_closure_sha256"]
                        ):
                            raise ValueError(
                                "runtime build Python verification response differs from admission"
                            )
                return copy.deepcopy(self._identity)
            except BaseException as exc:
                self.record_failure(exc)
                try:
                    self.close()
                except BuildPythonCleanupError as cleanup:
                    self._report_cleanup_failure(cleanup, exc)
                if self._failure_detail:
                    exc.add_note(self._failure_detail)
                raise

    def close(self) -> None:
        with self._lock:
            # Revocation is immediate. Cleanup is retriable until exact child
            # closure AND response-reader completion are both proven.
            self._closed = True
            self._identity = None
            process = self._process
            if process is None:
                if self._stderr is not None:
                    self._stderr.close()
                    self._stderr = None
                return
            failure: BaseException | None = None
            result: int | None = None
            try:
                if process.stdin is not None and not process.stdin.closed:
                    try:
                        process.stdin.close()
                    except BrokenPipeError:
                        pass
                assert self._executor is not None
                result = self._executor.wait_owned(process, timeout=10.0)
            except BaseException as exc:
                failure = exc
            # A failed wait can still have completed guardian cancellation.
            # Inspect the owner's terminal proof; wrapper exit alone is not it.
            terminal = bool(getattr(process, "terminal", False))
            if terminal and self._reader is not None:
                self._reader.join(timeout=1.0)
                if self._reader.is_alive():
                    terminal = False
                    failure = failure or RuntimeError(
                        "response reader remains live after guardian closure"
                    )
            if terminal:
                try:
                    if process.stdout is not None:
                        process.stdout.close()
                    if self._stderr is not None:
                        self._stderr.seek(0, os.SEEK_END)
                        self._stderr.seek(max(0, self._stderr.tell() - 16384))
                        self._failure_detail = (
                            self._stderr.read()
                            .decode("utf-8", errors="replace")
                            .strip()
                        )
                        self._stderr.close()
                        self._stderr = None
                    self._process = None
                except BaseException as exc:
                    if failure is not None:
                        failure.add_note(f"stream cleanup also failed: {exc}")
                    else:
                        failure = exc
            else:
                failure = failure or RuntimeError(
                    "guardian child custody remains unresolved"
                )
            if result is not None and result != 0:
                failure = failure or ValueError(
                    f"identity probe failed (exit {result})"
                )
            if failure is not None:
                cleanup = BuildPythonCleanupError(self, failure)
                self.cleanup_failure = cleanup
                raise cleanup from failure


@contextmanager
def build_python_scope(
    state: _RuntimeArtifactState | None,
) -> Generator[BuildPythonAdmission]:
    """Borrow the enclosing build owner, or close one standalone producer scope."""
    if state is not None and state.build_python_admission is not None:
        yield state.build_python_admission
        return
    admission = BuildPythonAdmission()
    if state is not None:
        state.build_python_admission = admission
    try:
        with admission:
            yield admission
    finally:
        if state is not None and admission.terminal:
            state.build_python_admission = None
