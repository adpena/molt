"""Landing-gate execution for ``tools/molt_dev.py integrate``.

Explicit pytest nodes collect first under the same environment without executing
test bodies or replacing mandatory gates. The first observed gate failure cancels
remaining work through the existing guards. Outcomes report in declared order.
A gate's nonzero exit raises ``GateFailure``, a ``CandidateFailure``
the landing queue bisects on; a stop of the runner itself stays a plain
``DriverError``. Every gate executes through the one proof executor
(``tools/proof_executor.py``) in its plan resource class: gates that may build
with Cargo share one target, where Cargo's build lock serializes their builds
and each rule keeps its declared order; pytest sessions run beside them in the
memory slice the executor exports, and read-only checks fill the rest, all
bounded by the plan's executor ceiling and ``molt.host_capacity``
(docs/design/host_capacity.md, "One scheduler"). The block leaves a receipt
with each gate's duration and peak RSS. Every gate writes its streams to files
while it runs (`prepare_gate_streams`); the terminal shows a failure's bounded
tail.
"""

from __future__ import annotations

from tools.molt_dev_policy import Deferred, GateEnvironment, LandingMode
from tools.proof_plan import CargoTargetAccess

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field, replace
from enum import StrEnum
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import sys
import threading
import time
from typing import TYPE_CHECKING, Any, Literal, Protocol, assert_never

from tools import harness_memory_guard, proof_executor, proof_receipts
from tools.driver_errors import CandidateFailure, DriverError, GateFailure  # noqa: E402
from tools.molt_dev_common import _say, _warn  # noqa: E402

if TYPE_CHECKING:
    from molt.disk_capacity import DiskCapacityError, DiskCapacityProbe
    from molt.host_capacity import HostWorkDemand, HostWorkGrant
    from tools.proof_plan import ToolchainPolicy

# Local always gates share the repository-policy resource class with their CI
# commands; a policy without that class admits up to the executor ceiling.
GATE_RESOURCE_CLASS = "repository-policy"
# Every gate that may build with Cargo shares one Cargo target. Cargo's build
# lock serializes builds there and releases it before tests run, so local Cargo
# gates overlap each other's test runs and other-profile builds; memory bounds
# how many run at once. pytest sessions size their workers within the memory
# slice the executor exports to each.
CARGO_GATE_RESOURCE_CLASS = "compiler-build-resource"
# A Cargo gate's process tree peaks near 5.4 GiB (changed Clippy, changed crate
# tests on the 2026-11-15 batches); the plan's single CI slot stays for CI.
LOCAL_CARGO_GATE_BYTES = 8 << 30
LOCAL_CARGO_GATES = 3
PYTEST_GATE_RESOURCE_CLASS = "python-tests"
LOCAL_GATE_FAMILY = "local_integration"
# The largest always gate's process tree peaks below 1 GiB (structural_audit
# 0.69 GB, generators 0.50 GB on the 2026-10 tree): the host-capacity unit.
CONCURRENT_GATE_BYTES = 1 << 30


class CancelReason(StrEnum):
    """Why a runtime's owner stops its gates; each owner names its own."""

    # A higher-priority landing request preempts a closed postsubmit run.
    PENDING_LANDING = "postsubmit cancelled for a pending landing request"
    # The lane precheck's try run reached its budget (PRECHECK_BUDGET).
    PRECHECK_BUDGET = "the precheck try run reached its budget"

    @property
    def stops_on_unresolved_closure(self) -> bool:
        """Whether a cancelled gate whose guard closure stays indeterminate
        stops the runner. A postsubmit verdict needs proven custody (HF-655).
        A budget refusal names the gate and its unresolved closure instead:
        the precheck proves nothing for it, and the batch reruns it in its own
        sandbox."""
        return self is CancelReason.PENDING_LANDING


class GateCancellation(DriverError):
    """The runtime's owner stopped its gates for ``reason``.

    ``unresolved`` names each cancelled gate whose guard closure stayed
    indeterminate, with the guard's diagnostic.
    """

    def __init__(self, reason: CancelReason, unresolved: tuple[str, ...] = ()) -> None:
        super().__init__(reason.value)
        self.reason = reason
        self.unresolved = unresolved


class GateMemoryPressure(DriverError):
    """The guard stopped a gate at its memory share, and closed its tree.

    The share is the free memory divided among the gates running at once, so
    the same gate can pass alone. The landing reruns the stopped gates alone
    once before it fails (2026-11-06: one `molt-wasm-host` needed 9.8-11.0 GB,
    above a 7.3-8.3 GB share, on main and on the batch). ``shown`` names them.
    """

    def __init__(self, message: str, shown: tuple[str, ...]) -> None:
        super().__init__(message)
        self.shown = shown


class Gate(Protocol):
    @property
    def command(self) -> str | tuple[str, ...]: ...

    @property
    def toolchains(self) -> tuple[str, ...]: ...

    @property
    def concurrent(self) -> bool: ...

    @property
    def cargo_target_access(self) -> CargoTargetAccess | None: ...

    @property
    def collection_command(self) -> tuple[str, ...] | None: ...

    @property
    def pytest(self) -> bool: ...

    @property
    def rules(self) -> tuple[str, ...]: ...


class Progress(Protocol):
    def emit(self, event: str, **details: object) -> None: ...


@dataclass(frozen=True)
class GateOutcome:
    guard_result: harness_memory_guard.GuardedCompletedProcess[str]

    @property
    def returncode(self) -> int:
        return self.guard_result.returncode

    @property
    def stdout(self) -> str:
        return self.guard_result.stdout or ""

    @property
    def stderr(self) -> str:
        return self.guard_result.stderr or ""

    @property
    def peak_rss_bytes(self) -> int | None:
        peak = self.guard_result.peak_total or self.guard_result.peak
        return None if peak is None else peak.rss_kb * 1024


def _classify_gate(
    outcome: GateOutcome,
    policies: tuple[ToolchainPolicy, ...],
    *,
    cancel_requested: bool,
    cargo_target_access: CargoTargetAccess,
) -> dict[str, Any]:
    """Project the actual guard result through the shared failure authority.

    ``cancel_requested`` is whether the executor had asked to stop the gate
    when its outcome arrived. The guard alone knows whether that request
    reached the running gate, so the request counts only with the guard's
    cancellation; a guard cancellation without a request stays inconsistent.
    A gate without Cargo target access makes no claim on the shared target:
    the guard's Cargo recovery there is not this gate's verdict (HF-605).
    """
    from tools.proof_plan import (
        _admitted_linker_helpers,
        _guard_closure_failure,
        _guarded_failure_scope,
    )

    if not isinstance(outcome, GateOutcome) or not isinstance(
        outcome.guard_result, harness_memory_guard.GuardedCompletedProcess
    ):
        raise DriverError("guard outcome unavailable or inconsistent")
    guard = outcome.guard_result
    memory = harness_memory_guard.memory_guard
    metrics = {
        "schema": "molt.local-gate-guard-outcome.v1",
        "command": list(guard.args),
        "returncode": guard.returncode,
        "child_returncode": guard.child_returncode,
        "cancelled": guard.cancelled,
        "descendants_closed": guard.descendants_closed,
        "timed_out": guard.timed_out,
        "memory_violation": harness_memory_guard._rss_record_payload(guard.violation),
        "guard_signal": guard.guard_signal,
        "exit_signal": (
            None
            if guard.cancelled
            or guard.timed_out
            or guard.violation
            or guard.guard_signal
            else memory.exit_signal_payload(guard.returncode)
        ),
        "infrastructure_failure": memory.infrastructure_failure_payload(
            guard.infrastructure_failure
        ),
        "termination_reports": memory.termination_reports_payload(
            guard.termination_reports
        ),
        "windows_job_cleanup": memory.windows_job_cleanup_payload(
            guard.windows_job_cleanup
        ),
        "cargo_incremental_quarantine": memory._cargo_incremental_quarantine_payload(
            guard.cargo_incremental_quarantine
        ),
        "temporary_artifacts": guard.temporary_artifacts,
    }
    verdict = (
        {**metrics, "cargo_incremental_quarantine": None}
        if cargo_target_access is CargoTargetAccess.NONE
        else metrics
    )
    helpers = _admitted_linker_helpers(policies)
    scope, reason = _guarded_failure_scope(
        verdict,
        metrics_valid=True,
        returncode=guard.returncode,
        cancel_requested=cancel_requested and guard.cancelled,
        admitted_linker_helpers=helpers,
    )
    if reason == "command failed":
        reason = f"exit {guard.returncode}"
    cancelled = reason == "executor cancellation"
    return {
        "status": (
            "cancelled"
            if cancelled
            else "failure"
            if scope == "global" or guard.returncode != 0
            else "success"
        ),
        "failure_scope": scope,
        "failure_reason": reason,
        "cancellation_cleanup": (
            _guard_closure_failure(verdict, helpers) if cancelled else None
        ),
        "guard_outcome": metrics,
    }


#: A failed gate's terminal excerpt: the last lines of each stream, each cut
#: to a width, so one oversized line cannot flood the caller's context.
FAILURE_TAIL_LINES = 20
FAILURE_TAIL_WIDTH = 400


def gate_output_dir(receipt_path: Path) -> Path:
    """This run's gate logs, beside the receipt, named by UTC time and pid so
    a run never reuses another's."""
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    return prerequisite_receipt_path(receipt_path).parent / f"{stamp}-{os.getpid()}"


def prerequisite_receipt_path(receipt_path: Path) -> Path:
    """Selection evidence follows the executor receipt, independently of logs."""
    return receipt_path.with_suffix(".d") / "prerequisites.json"


def prepare_gate_streams(
    directory: Path, ordinal: int
) -> tuple[tuple[Path, Path] | None, str | None]:
    """The files a gate writes its stdout and stderr to while it runs, or
    ``(None, error)`` when they cannot be prepared; the gate then runs with
    its streams in memory, and a failure shows them whole."""
    streams = (
        directory / f"gate-{ordinal:03d}.stdout.log",
        directory / f"gate-{ordinal:03d}.stderr.log",
    )
    try:
        directory.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        return None, f"{type(exc).__name__}: {exc}"
    return streams, None


def stream_record(
    streams: tuple[Path, Path] | None, error: str | None
) -> dict[str, str]:
    """Where a gate's streams are, for its receipt record and progress event."""
    if streams is None:
        return {"output_error": str(error)}
    return {"stdout_path": str(streams[0]), "stderr_path": str(streams[1])}


def gate_failure_report(outcome: GateOutcome, record: Mapping[str, str]) -> str:
    """What the terminal shows of a failed gate: where its full streams are
    and the bounded end of each; when they could not go to files, both
    streams whole, so the reason is never lost."""
    if "stdout_path" not in record:
        return (
            f"    gate output not kept ({record.get('output_error')}); "
            "its streams follow whole\n"
            f"--- stdout ---\n{outcome.stdout}\n--- stderr ---\n{outcome.stderr}\n"
        )

    def tail(text: str) -> list[str]:
        lines = text.rstrip("\n").splitlines()[-FAILURE_TAIL_LINES:]
        return [
            line
            if len(line) <= FAILURE_TAIL_WIDTH
            else f"{line[:FAILURE_TAIL_WIDTH]}... ({len(line)} chars)"
            for line in lines
        ]

    parts = [
        f"    full stdout: {record['stdout_path']}\n",
        f"    full stderr: {record['stderr_path']}\n",
    ]
    for name, text in (("stdout", outcome.stdout), ("stderr", outcome.stderr)):
        lines = tail(text)
        if lines:
            parts.append(f"    --- {name}, last {len(lines)} lines ---\n")
            parts.extend(f"{line}\n" for line in lines)
    return "".join(parts)


@dataclass(frozen=True)
class GateRuntime:
    """What one landing's gates run against; integrate builds it once."""

    repo: Path
    environments: Mapping[tuple[str, ...], GateEnvironment]
    cargo_outputs: Mapping[tuple[str, ...], tuple[Path, ...]]
    capacity_requirements: Mapping[tuple[str, ...], int]
    progress: Progress
    check_source: Callable[[], None]
    source_changes: Callable[[], tuple[str, ...]]
    source_identity: Callable[[], dict[str, str]]
    policy_text: str
    executor_max_workers: int | None
    resource_limits: Mapping[str, int]
    toolchains: Mapping[str, dict[str, Any]]
    receipt_path: Path
    # This run's gate logs (`gate_output_dir`); the receipt names each file.
    output_dir: Path
    run_captured_gate: Callable[..., GateOutcome]
    observe_capacity: Callable[..., Any]
    say: Callable[[str], None]
    ok: Callable[[str], None]
    toolchain_policies: tuple[ToolchainPolicy, ...] = ()
    mode: LandingMode = LandingMode.PRESUBMIT
    cancelled: Callable[[], bool] = lambda: False
    cancel_reason: CancelReason = CancelReason.PENDING_LANDING
    deferred: tuple[Deferred, ...] = ()
    host_grant: HostWorkGrant | None = None
    # Every gate that passed, in completion order: the typed outcome a caller
    # reads instead of matching rendered commands.
    passed: list[Gate] = field(default_factory=list)
    # The stdout log of the run that passed, by shown command: after a rerun,
    # the rerun's log, never the failed first attempt's (landing receipt counts).
    passed_logs: dict[str, Path] = field(default_factory=dict)


def gate_runs_pytest(gate: Gate, *, repo: Path) -> bool:
    """Structured cohorts and literal rule commands share the argv authority."""
    import shlex
    from tools.proof_command_argv import pytest_arguments

    argv = (
        tuple(shlex.split(gate.command))
        if isinstance(gate.command, str)
        else gate.command
    )
    return gate.pytest or pytest_arguments(argv, root=repo) is not None


def _pytest_report_command(
    command: str | tuple[str, ...],
    gate: Gate,
    runtime: GateRuntime,
    ordinal: int,
    log_dir: Path,
) -> str | tuple[str, ...]:
    """JUnit is pytest's own test-level failure evidence, kept with the gate's
    other logs in ``log_dir``: postsubmit names failing tests from it, and
    either mode reruns a failed test alone by its node ID
    (``_run_heavy_gates``). Postsubmit also runs the flaky-marked tests."""
    if not gate_runs_pytest(gate, repo=runtime.repo):
        return command
    match runtime.mode:
        case LandingMode.PRESUBMIT:
            extra: tuple[str, ...] = ()
        case LandingMode.POSTSUBMIT:
            extra = ("--run-flaky",)
        case _:
            assert_never(runtime.mode)
    import shlex

    argv = tuple(shlex.split(command)) if isinstance(command, str) else command
    return _with_pytest_options(
        argv,
        *extra,
        "-o",
        "junit_family=xunit1",
        "--junitxml=" + str(junit_path(log_dir, ordinal)),
        repo=runtime.repo,
    )


def junit_path(log_dir: Path, ordinal: int) -> Path:
    return log_dir / f"gate-{ordinal:03d}.junit.xml"


def _numbered_attempt_directories(
    directory: Path,
    prefix: Literal[
        "postsubmit-rest-", "heavy-rest-", "isolate-", "continue-", "attempt-"
    ],
) -> tuple[Path, ...]:
    """Existing directories with the exact numeric suffix of an owned phase."""
    return tuple(
        sorted(
            (
                path
                for path in directory.glob(prefix + "[0-9]*")
                if path.name.removeprefix(prefix).isdecimal() and path.is_dir()
            ),
            key=lambda path: int(path.name.removeprefix(prefix)),
        )
    )


def last_gate_attempt_directory(log_dir: Path, ordinal: int) -> Path:
    """The latest executor attempt that created this gate's stream evidence."""
    attempts = _numbered_attempt_directories(log_dir, "attempt-")
    return next(
        (
            path
            for path in reversed(attempts)
            if (path / f"gate-{ordinal:03d}.stdout.log").is_file()
        ),
        log_dir,
    )


def _with_pytest_options(
    argv: Sequence[str], *options: str, repo: Path
) -> tuple[str, ...]:
    """``argv`` with pytest options added before any ``--``, after which every
    word is a target."""
    from tools.proof_command_argv import pytest_arguments

    arguments = pytest_arguments(argv, root=repo)
    argv = tuple(argv)
    if arguments is None or "--" not in arguments:
        return (*argv, *options)
    boundary = len(argv) - len(arguments) + arguments.index("--")
    return (*argv[:boundary], *options, *argv[boundary:])


def _check_cancellation(runtime: GateRuntime) -> None:
    if runtime.cancelled():
        raise GateCancellation(runtime.cancel_reason)


def display(command: str | tuple[str, ...]) -> str:
    import shlex

    return command if isinstance(command, str) else shlex.join(command)


def gate_time_summary(gate_seconds: list[tuple[float, str]], *, top: int = 8) -> str:
    """The gates' total time and the slowest ones, so the costliest gates
    are visible on every landing."""
    total = sum(seconds for seconds, _ in gate_seconds)
    lines = [f"    gates: {len(gate_seconds)} in {total:.1f} s; slowest:"]
    ranked = sorted(gate_seconds, key=lambda item: item[0], reverse=True)[:top]
    for seconds, cmd in ranked:
        lines.append(f"      {seconds:7.1f} s  {cmd}")
    return "\n".join(lines)


def gate_details(ordinal: int, gate: Gate) -> dict[str, object]:
    cmd = gate.command
    return {
        "gate_ordinal": ordinal,
        "command": (
            {"kind": "shell", "source": cmd}
            if isinstance(cmd, str)
            else {"kind": "argv", "argv": list(cmd)}
        ),
        "toolchains": list(gate.toolchains),
    }


def gate_failure(
    rc: int | None,
    elapsed: float,
    shown: str,
    gate_seconds: list[tuple[float, str]],
    record: Mapping[str, str] | None = None,
) -> GateFailure:
    """A selected gate exited nonzero: the candidate failed (the landing queue
    bisects on this type). A fault of the runner itself stays a DriverError."""
    kept = (
        f"    full stderr: {record['stderr_path']}\n"
        if record and "stderr_path" in record
        else ""
    )
    failure = GateFailure(
        f"gate failed (exit {rc}) after {elapsed:.1f} s: {shown}\n"
        + kept
        + gate_time_summary(gate_seconds)
        + "\n    Integration halts before push; fix the gate and re-run."
    )
    failure.returncode = rc
    return failure


def gate_work_demand(
    gates: Sequence[Gate], *, repo: Path, requested_workers: int
) -> HostWorkDemand:
    """One gate slot must accommodate any selected gate's workspace.

    A Cargo-backed gate may build while running pytest, so it retains its
    charged Cargo envelope even when the child also has a pytest worker unit.
    """
    import shlex

    from molt.host_capacity import HostWorkDemand
    from tools.proof_command_argv import pytest_allocation_request

    bytes_per_worker = CONCURRENT_GATE_BYTES
    for gate in gates:
        if gate.cargo_target_access is not CargoTargetAccess.NONE:
            bytes_per_worker = max(bytes_per_worker, LOCAL_CARGO_GATE_BYTES)
        if gate_runs_pytest(gate, repo=repo):
            argv = (
                shlex.split(gate.command)
                if isinstance(gate.command, str)
                else gate.command
            )
            _arguments, bytes_per_pytest_worker, _requested = pytest_allocation_request(
                argv, root=repo
            )
            # The executor may allocate several pytest workers to a single gate.
            # Its actual worker count is bounded by the grant's memory slice.
            bytes_per_worker = max(bytes_per_worker, bytes_per_pytest_worker)
    return HostWorkDemand(
        workers=max(1, min(len(gates), requested_workers)),
        bytes_per_worker=bytes_per_worker,
        minimum_workers=1,
    )


def concurrent_gate_limit() -> int:
    """Gates this host runs at once, from the host capacity authority."""
    from molt import host_capacity

    return host_capacity.worker_count(
        bytes_per_worker=CONCURRENT_GATE_BYTES,
        headroom_bytes=host_capacity.headroom_bytes(host_capacity.memory_capacity()),
    )


def reclaim_hot_output(
    error: DiskCapacityError,
    environment: Mapping[str, str],
    attempted: set[Path],
) -> bool:
    """Try one bounded cleanup for a measured shortfall in the bound hot root."""
    root = error.hot_root
    if root is None or root in attempted or not error.is_hot_capacity_shortfall():
        return False
    try:
        device = root.stat().st_dev
        for probe in error.rejected:
            measured = probe.measured_path
            if (
                probe.error is not None
                or probe.free_bytes is None
                or measured is None
                or not probe.requested_path.is_relative_to(root)
                or not measured.is_relative_to(root)
                or measured.stat().st_dev != device
            ):
                return False
    except OSError:
        return False

    from tools import disk_guard

    attempted.add(root)
    required = max(probe.required_bytes for probe in error.rejected)
    result = disk_guard.ensure_free(
        required / (1 << 30), volume_root=root, env=environment
    )
    _say(
        f"    hot output cleanup: {len(result.reclaimed)} retired, "
        f"{result.free_before} -> {result.free_after} bytes free on {root}"
    )
    return True


def admit_selected_output_capacity(
    outputs: dict[tuple[str, ...], tuple[Path, ...]],
    environments: dict[tuple[str, ...], GateEnvironment],
) -> dict[tuple[str, ...], int]:
    """Observe output capacity, reclaim once on a hot shortfall, then recheck."""
    from molt.disk_capacity import (
        DiskCapacityError,
        minimum_headroom_bytes,
        require_build_capacity,
    )

    requirements = {}
    observed: set[tuple[tuple[Path, ...], int]] = set()
    attempted: set[Path] = set()
    for tools, paths in outputs.items():
        environment = environments[tools].environment
        required = minimum_headroom_bytes(environment)
        requirements[tools] = required
        key = (paths, required)
        if key in observed:
            continue
        try:
            receipt = require_build_capacity(paths, env=environment)
        except DiskCapacityError as error:
            if not reclaim_hot_output(error, environment, attempted):
                raise
            receipt = require_build_capacity(paths, env=environment)
        observed.add(key)
        _say(
            "    selected output capacity: "
            + json.dumps(
                {
                    **receipt.as_dict(),
                    "scope": "current Cargo-capable selected outputs",
                    "remaining_campaign_growth_bytes": None,
                    "reserved_bytes": 0,
                },
                sort_keys=True,
            )
        )
    if outputs:
        _say("    remaining campaign growth unknown; capacity is not reserved")
    return requirements


def observe_gate_capacity(
    paths: tuple[Path, ...],
    *,
    required_bytes: int,
    phase: str,
    gate: str,
    before: tuple[DiskCapacityProbe, ...] = (),
) -> tuple[DiskCapacityProbe, ...]:
    """Record interval observations; telemetry cannot change a gate verdict."""
    from molt.disk_capacity import probe_capacity

    try:
        probes = tuple(
            probe_capacity(path, required_bytes=required_bytes) for path in paths
        )
        previous = {probe.requested_path: probe.free_bytes for probe in before}
        changes = {
            str(probe.requested_path): (
                probe.free_bytes - prior
                if isinstance(prior := previous.get(probe.requested_path), int)
                and isinstance(probe.free_bytes, int)
                else None
            )
            for probe in probes
        }
        _say(
            "    gate capacity: "
            + json.dumps(
                {
                    "scope": "Cargo-capable gate interval; not exclusive allocation",
                    "phase": phase,
                    "gate": gate,
                    "probes": [probe.as_dict() for probe in probes],
                    "free_bytes_delta": changes if phase == "after" else {},
                },
                sort_keys=True,
            )
        )
        return probes
    except Exception as exc:
        _warn(
            f"gate capacity observation unavailable ({phase}): {type(exc).__name__}: {exc}"
        )
        return ()


def gate_resource_class(gate: Gate) -> str:
    """The plan resource class one landing gate is admitted in. A pytest
    session counts against the session limit even when its rules declare
    Cargo; `_shared_target_order` orders only shared-target work."""
    if gate.pytest:
        return PYTEST_GATE_RESOURCE_CLASS
    if "cargo" in gate.toolchains:
        return CARGO_GATE_RESOURCE_CLASS
    return GATE_RESOURCE_CLASS


def _shared_target_order(commands: list[_LocalGateCommand]) -> list[_LocalGateCommand]:
    """Order shared-target work by its producers instead of one global chain.

    Cargo's build lock serializes builds on the shared target and releases it
    before tests run, so independent rules' Cargo gates overlap. Each rule
    keeps its declared order: a later gate may consume an earlier one's build
    (``runtime-wasm-signatures`` compares the runtime its first gate builds).
    A pytest session merges rules, so it waits for every shared-target build
    declared before it. Isolated targets retain their child-owned admission;
    failure cancellation still covers every gate of the block.
    """
    ordered: list[_LocalGateCommand] = []
    builds: list[str] = []
    last: dict[str, str] = {}
    for command in commands:
        if command.gate.cargo_target_access is CargoTargetAccess.SHARED:
            if command.gate.pytest:
                after = tuple(builds)
            else:
                after = tuple(
                    dict.fromkeys(
                        last[rule] for rule in command.gate.rules if rule in last
                    )
                )
                builds.append(command.id)
                for rule in command.gate.rules:
                    last[rule] = command.id
            if after:
                command = replace(command, dependencies=after)
        ordered.append(command)
    return ordered


def run_selected_gates(
    gates: Sequence[Gate],
    runtime: GateRuntime,
    gate_seconds: list[tuple[float, str]],
    *,
    host_limit: Callable[[], int] = concurrent_gate_limit,
    keep_going: bool = False,
) -> None:
    """Read-only checks first; then the heavy gates overlap in one block.

    The read-only checks run as before: the leading always gates as one
    executor block, then each rule check in declared order. A failing check
    therefore stops every build before it starts. The heavy gates (Cargo and
    pytest) then share one executor block: shared-target writers run one at a
    time while independent pytest sessions run beside them, each sized
    within the exported memory slice. The first observed failure within a
    phase halts the landing and cancels its remaining gates through their guards.

    ``keep_going`` (postsubmit) runs every gate and raises one CandidateFailure
    naming each failed gate at the end: postsubmit reports main's state, and a
    first-failure stop hid every later gate (HF-937: no run since 2026-11-01
    reached a Cargo gate past its first red static check). Read-only checks
    then run one at a time, and the heavy block reruns the gates a failure
    cancelled, without the failed ones. Runner, custody and cancellation
    faults still stop at once.
    """
    for item in runtime.deferred:
        runtime.say(item.diagnostic())
    from molt.file_publication import atomic_write_bytes

    prerequisites = (
        json.dumps(
            {
                "schema": "molt.landing-prerequisites.v1",
                "mode": runtime.mode.value,
                "runnable": [gate.command for gate in gates],
                "deferred": [item.record() for item in runtime.deferred],
            },
            indent=2,
        )
        + "\n"
    ).encode("utf-8")
    try:
        atomic_write_bytes(
            prerequisite_receipt_path(runtime.receipt_path), prerequisites
        )
    except OSError as exc:
        raise DriverError(f"cannot retain prerequisite evidence: {exc}") from exc
    ordered = list(enumerate(gates, start=1))
    failures: list[CandidateFailure] = []
    for ordinal, gate in tuple(ordered):
        _check_cancellation(runtime)
        if gate.collection_command is not None:
            try:
                _run_serial_gate(ordinal, gate, runtime, gate_seconds, collection=True)
            except GateFailure as failure:
                if not keep_going:
                    raise
                # A gate whose tests do not collect cannot run them.
                failures.append(failure)
                ordered.remove((ordinal, gate))
    light = [
        (ordinal, gate)
        for ordinal, gate in ordered
        if gate_resource_class(gate) == GATE_RESOURCE_CLASS
    ]
    heavy = [
        (ordinal, gate)
        for ordinal, gate in ordered
        if gate_resource_class(gate) != GATE_RESOURCE_CLASS
    ]
    block = 0
    while not keep_going and block < len(light) and light[block][1].concurrent:
        block += 1
    if block >= 2:
        _run_executor_block(light[:block], runtime, gate_seconds, host_limit())
    else:
        block = 0
    for ordinal, gate in light[block:]:
        _check_cancellation(runtime)
        try:
            _run_serial_gate(ordinal, gate, runtime, gate_seconds)
        except GateFailure as failure:
            if not keep_going:
                raise
            failures.append(failure)
    _check_cancellation(runtime)
    if heavy:
        from molt import host_capacity

        # Every gate run (a try run, a batch, postsubmit, a receipt rerun)
        # takes its class's heavy-work slot for its heavy block (HF-623): a
        # batch the integration slot, the rest the development slot. A caller
        # that already holds a slot (the try run, whose budget starts at
        # admission) is admitted at once through the inherited mark.
        with host_capacity.host_work_slot(
            host_capacity.host_work_dir(runtime.repo),
            label=f"{runtime.mode.value} gates ({runtime.repo.name})",
            work=(
                host_capacity.HostWorkClass.INTEGRATION
                if runtime.mode is LandingMode.PRESUBMIT
                else host_capacity.HostWorkClass.DEVELOPMENT
            ),
            say=runtime.say,
            demand=(
                gate_work_demand(
                    [gate for _ordinal, gate in heavy],
                    repo=runtime.repo,
                    requested_workers=max(1, host_capacity.cpu_count() // 2),
                )
                if runtime.mode is LandingMode.POSTSUBMIT
                else None
            ),
        ) as admission:
            grant = admission.grant or runtime.host_grant
            runtime = replace(runtime, host_grant=grant)
            remaining = heavy
            continuation = 0
            while remaining:
                attempt = runtime
                if continuation:
                    output = runtime.output_dir / f"postsubmit-rest-{continuation}"
                    attempt = replace(
                        runtime,
                        output_dir=output,
                        receipt_path=output / runtime.receipt_path.name,
                    )
                try:
                    _run_heavy_gates(remaining, attempt, gate_seconds, host_limit)
                    break
                except GateFailure as failure:
                    if (
                        not keep_going
                        or runtime.cancelled()
                        or not failure.clean_closure
                    ):
                        raise
                    failed = {
                        display(gate.command)
                        for _ordinal, gate in remaining
                        if display(gate.command) in failure.shown
                    }
                    if not failed:
                        raise
                    failures.append(failure)
                    remaining = [
                        (ordinal, gate)
                        for ordinal, gate in remaining
                        if gate not in runtime.passed
                        and display(gate.command) not in failed
                    ]
                    continuation += 1
    _check_cancellation(runtime)
    if failures:
        raise CandidateFailure(
            f"{len(failures)} gate(s) failed:\n"
            + "\n".join(str(failure).splitlines()[0] for failure in failures)
        )


def _run_heavy_gates(
    heavy: list[tuple[int, Gate]],
    runtime: GateRuntime,
    gate_seconds: list[tuple[float, str]],
    host_limit: Callable[[], int],
) -> None:
    """Run the heavy block; on a failure, retry what failed, then run the
    gates the stop cancelled, until every gate passed or one fails for good.

    A test that passes alone failed on the load beside it or on a flake, not
    on the candidate (Chromium's retry rule; Bazel's flaky-test attempts); a
    second failure is the candidate's and raises at once. A failed pytest gate
    whose JUnit names its failed tests reruns only those tests, alone
    (``_isolate_tests``), in any round: on 2026-11-14 a whole-gate retry spent
    449.8 s on one flaky test, and a flake in the gates a stop had cancelled
    failed a lane outright after 905 s. A failed changed-crate-tests gate
    whose last line names its failed tests reruns only those, alone
    (``_isolate_cargo_tests``): on 2026-11-15 a whole rerun spent 808.2 s to
    fail the same two tests again. Any other failed gate, and a memory
    stop, reruns whole and alone, in the first round only. Custody, closure,
    cancellation and runner faults stop at once: a retry must never hide
    them, so a failure retries only with a clean closure. The one exception
    is the cleanup of a gate this block's own stop cancelled: it permits a
    test-level rerun, never a whole one, and is kept as the failure's note.
    """
    block = list(heavy)
    rounds = 0
    while True:
        kind = ("heavy", "heavy-rest")[min(rounds, 1)] + (
            f"-{rounds}" if rounds > 1 else ""
        )
        log_dir = runtime.output_dir if rounds == 0 else runtime.output_dir / kind
        try:
            _run_executor_block(
                block,
                runtime,
                gate_seconds,
                host_limit(),
                receipt_kind=kind,
                log_dir=log_dir,
            )
            return
        except (GateFailure, GateMemoryPressure) as failure:
            # A gate this block's stop cancelled mid-compile leaves an
            # unresolved Cargo cleanup (batch 04f8f19ff9 lost a flake's rerun
            # this way); that permits only a test-level rerun of the failed
            # gate, and the cancelled gates rerun under their own guards.
            tests_only = (
                isinstance(failure, GateFailure)
                and not failure.clean_closure
                and failure.stop_cleanup_only
            )
            if runtime.cancelled() or (
                isinstance(failure, GateFailure)
                and not failure.clean_closure
                and not tests_only
            ):
                raise
            failed = [
                (ordinal, gate)
                for ordinal, gate in block
                if display(gate.command) in failure.shown
            ]
            if not failed:
                raise
            whole: list[tuple[int, Gate]] = []
            for ordinal, gate in failed:
                tests = (
                    _failed_tests(gate, ordinal, log_dir, runtime)
                    if isinstance(failure, GateFailure)
                    else None
                )
                rerun = (
                    _failed_cargo_tests(ordinal, log_dir)
                    if tests is None and isinstance(failure, GateFailure)
                    else None
                )
                if tests is not None:
                    _isolate_tests(
                        ordinal, gate, tests, failure, runtime, gate_seconds, host_limit
                    )
                elif rerun is not None:
                    _isolate_cargo_tests(
                        ordinal, gate, rerun, failure, runtime, gate_seconds
                    )
                elif rounds == 0 and not tests_only:
                    whole.append((ordinal, gate))
                else:
                    raise
            if whole:
                _retry_whole_gates(whole, failure, runtime, gate_seconds)
        rounds += 1
        # Each round passes or raises on every gate it names as failed.
        block = [
            (ordinal, gate) for ordinal, gate in heavy if gate not in runtime.passed
        ]
        if not block:
            return


def _retry_whole_gates(
    failed: list[tuple[int, Gate]],
    first: GateFailure | GateMemoryPressure,
    runtime: GateRuntime,
    gate_seconds: list[tuple[float, str]],
) -> None:
    runtime.say(
        f"    retry: {len(failed)} failed heavy gate(s) alone after: "
        f"{str(first).splitlines()[0]}"
    )
    try:
        _run_executor_block(
            failed,
            runtime,
            gate_seconds,
            1,
            receipt_kind="heavy-retry",
            log_dir=runtime.output_dir / "heavy-retry",
        )
    except BaseException as second:
        second.add_note(f"first attempt, beside other gates: {first}")
        raise
    runtime.say(
        "    retry passed: the first failure was load or a flake, not the "
        f"candidate: {str(first).splitlines()[0]}"
    )
    log_dir = runtime.output_dir / "heavy-retry"
    for ordinal, gate in failed:
        path = junit_path(
            last_gate_attempt_directory(runtime.output_dir, ordinal), ordinal
        )
        nodes = _failed_nodes(path)
        _record_flake(runtime, gate, nodes or (), evidence=log_dir)


@dataclass(frozen=True)
class FailedTests:
    """What one failed pytest gate's JUnit proves."""

    failed: tuple[str, ...]
    # Every test the run reported, the failed ones included.
    reported: frozenset[str]
    # Whether tests may remain unrun: pytest exit 2 (xdist's maxfail stop),
    # or exit 1 under --maxfail/-x.
    interrupted: bool


#: Isolation rounds per gate: a continuation that keeps failing new tests is
#: a host or a candidate problem, not a flake.
ISOLATION_ROUNDS = 3


def _failed_nodes(path: Path) -> tuple[str, ...] | None:
    from molt.pytest_junit import PytestOutcome, junit_outcomes

    try:
        outcomes = junit_outcomes(path)
    except ValueError:
        return None
    return tuple(
        sorted(
            node
            for node, outcome in outcomes.items()
            if outcome is PytestOutcome.FAILED
        )
    )


def _failed_tests(
    gate: Gate, ordinal: int, log_dir: Path, runtime: GateRuntime
) -> FailedTests | None:
    """The failed tests of a pytest gate's run, or None when its JUnit cannot
    name them: no report, a collection error, or a report without the node
    IDs that prove the checkout's plugin can select tests
    (``molt.pytest_junit``)."""
    from molt.pytest_junit import PytestOutcome, junit_outcomes

    if not gate_runs_pytest(gate, repo=runtime.repo):
        return None
    path = junit_path(last_gate_attempt_directory(log_dir, ordinal), ordinal)
    if not path.is_file():
        return None
    try:
        outcomes = junit_outcomes(path)
    except ValueError as exc:
        runtime.say(f"    no test-level retry: {exc}")
        return None
    failed = tuple(
        sorted(
            node
            for node, outcome in outcomes.items()
            if outcome is PytestOutcome.FAILED
        )
    )
    if not failed:
        return None
    return FailedTests(failed, frozenset(outcomes), _stops_early(gate, runtime.repo))


def _stops_early(gate: Gate, repo: Path) -> bool:
    import shlex

    from molt.pytest_memory_guard_bootstrap import _pytest_argument_parser
    from tools.proof_command_argv import pytest_arguments

    argv = shlex.split(gate.command) if isinstance(gate.command, str) else gate.command
    arguments = pytest_arguments(argv, root=repo)
    if arguments is None:
        return True
    namespace, _unknown = _pytest_argument_parser(True).parse_known_and_unknown_args(
        list(arguments)
    )
    return bool(getattr(namespace, "maxfail", 0))


def _isolate_tests(
    ordinal: int,
    gate: Gate,
    tests: FailedTests,
    first: GateFailure,
    runtime: GateRuntime,
    gate_seconds: list[tuple[float, str]],
    host_limit: Callable[[], int],
) -> None:
    """Rerun ``tests.failed`` alone, then what an early stop left unrun.

    The failed tests rerun in one serial process, selected by node ID; a
    second failure is the candidate's. The continuation reruns the gate's own
    command minus every reported test. Both keep the gate's argv, so its
    options, markers and cohort collect as before. On success the gate counts
    as passed, its log is the last run's, and the flake is recorded.
    """
    import shlex

    from molt.file_publication import atomic_write_bytes

    shown = display(gate.command)
    base = (
        tuple(shlex.split(gate.command))
        if isinstance(gate.command, str)
        else tuple(gate.command)
    )
    state = runtime.output_dir / f"gate-{ordinal:03d}.isolation"
    reported = set(tests.reported)
    interrupted = tests.interrupted or first.returncode == 2
    failed = tests.failed
    log: Path | None = None

    def write(name: str, nodes: Sequence[str]) -> str:
        path = state / name
        atomic_write_bytes(path, (json.dumps(sorted(nodes)) + "\n").encode("utf-8"))
        return str(path)

    def run(kind: str, argv: tuple[str, ...], host: int) -> tuple[GateRuntime, Path]:
        log_dir = state / kind
        sub = replace(runtime, passed=[], passed_logs={})
        _run_executor_block(
            [(ordinal, replace(gate, command=argv, collection_command=None))],
            sub,
            gate_seconds,
            host,
            receipt_kind=f"gate-{ordinal:03d}.{kind}",
            log_dir=log_dir,
        )
        return sub, log_dir

    for attempt in range(1, ISOLATION_ROUNDS + 1):
        runtime.say(
            f"    retry alone: {len(failed)} failed test(s) of {shown}: "
            + ", ".join(failed[:5])
            + (" ..." if len(failed) > 5 else "")
        )
        isolated = _with_pytest_options(
            base,
            "-n",
            "0",
            "--molt-run-nodes=" + write(f"run-{attempt}.json", failed),
            repo=runtime.repo,
        )
        try:
            sub, _log_dir = run(f"isolate-{attempt}", isolated, 1)
        except BaseException as second:
            if isinstance(second, (GateFailure, GateMemoryPressure)):
                second.shown = (shown,)
            second.add_note(f"first attempt, beside other gates: {first}")
            raise
        log = next(iter(sub.passed_logs.values()), log)
        runtime.say(
            "    retry passed: the failure was load or a flake, not the "
            f"candidate: {', '.join(failed)}"
        )
        _record_flake(runtime, gate, failed, evidence=state)
        if not interrupted:
            break
        reported.update(failed)
        rest = _with_pytest_options(
            base,
            "--molt-skip-nodes=" + write(f"skip-{attempt}.json", sorted(reported)),
            repo=runtime.repo,
        )
        runtime.say(
            f"    continue: {shown} without its {len(reported)} reported test(s)"
        )
        try:
            sub, _log_dir = run(f"continue-{attempt}", rest, host_limit())
            log = next(iter(sub.passed_logs.values()), log)
            break
        except (GateFailure, GateMemoryPressure) as again:
            again.shown = (shown,)
            if isinstance(again, GateMemoryPressure) or not again.clean_closure:
                raise
            more = _failed_tests(
                replace(gate, command=rest),
                ordinal,
                state / f"continue-{attempt}",
                runtime,
            )
            if more is None or attempt == ISOLATION_ROUNDS:
                again.add_note(f"first attempt, beside other gates: {first}")
                raise
            failed = more.failed
            reported.update(more.reported)
            interrupted = more.interrupted or again.returncode == 2
    runtime.passed.append(gate)
    if log is not None:
        runtime.passed_logs[shown] = log
    runtime.ok(f"gate passed after a test-level retry: {shown}")


def _failed_cargo_tests(ordinal: int, log_dir: Path) -> Path | None:
    """The stdout log of a failed changed-crate-tests run whose last line
    names its failed tests (``failed_tests``), or None."""
    from tools.check_changed_crate_tests import SCHEMA

    log = (
        last_gate_attempt_directory(log_dir, ordinal) / f"gate-{ordinal:03d}.stdout.log"
    )
    try:
        lines = log.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return None
    last = next((line for line in reversed(lines) if line.startswith("{")), None)
    try:
        record = json.loads(last) if last is not None else None
    except ValueError:
        return None
    if (
        isinstance(record, dict)
        and record.get("schema") == SCHEMA
        and record.get("failed_tests")
    ):
        return log
    return None


def _isolate_cargo_tests(
    ordinal: int,
    gate: Gate,
    log: Path,
    first: GateFailure,
    runtime: GateRuntime,
    gate_seconds: list[tuple[float, str]],
) -> None:
    """Rerun only the failed tests ``log`` names, alone and one at a time; a
    second failure is the candidate's. On success the gate counts as passed
    and the flake is recorded."""
    import shlex

    from tools.check_changed_crate_tests import RERUN_OPTION

    shown = display(gate.command)
    command: str | tuple[str, ...] = (
        f"{gate.command} {RERUN_OPTION} {shlex.quote(str(log))}"
        if isinstance(gate.command, str)
        else (*gate.command, RERUN_OPTION, str(log))
    )
    record = json.loads(
        next(
            line
            for line in reversed(log.read_text(encoding="utf-8").splitlines())
            if line.startswith("{")
        )
    )
    tests = [name for rerun in record["failed_tests"] for name in rerun["tests"]]
    runtime.say(
        f"    retry alone: {len(tests)} failed Rust test(s) of {shown}: "
        + ", ".join(tests[:5])
        + (" ..." if len(tests) > 5 else "")
    )
    state = runtime.output_dir / f"gate-{ordinal:03d}.isolation"
    sub = replace(runtime, passed=[], passed_logs={})
    try:
        _run_executor_block(
            [(ordinal, replace(gate, command=command, collection_command=None))],
            sub,
            gate_seconds,
            1,
            receipt_kind=f"gate-{ordinal:03d}.rerun",
            log_dir=state,
        )
    except BaseException as second:
        if isinstance(second, (GateFailure, GateMemoryPressure)):
            second.shown = (shown,)
        second.add_note(f"first attempt, beside other gates: {first}")
        raise
    runtime.say(
        "    retry passed: the failure was load or a flake, not the "
        f"candidate: {', '.join(tests)}"
    )
    _record_flake(runtime, gate, tests, evidence=state)
    runtime.passed.append(gate)
    log_path = next(iter(sub.passed_logs.values()), None)
    if log_path is not None:
        runtime.passed_logs[shown] = log_path
    runtime.ok(f"gate passed after a test-level retry: {shown}")


def _record_flake(
    runtime: GateRuntime, gate: Gate, nodes: Sequence[str], *, evidence: Path
) -> None:
    """Append one passed retry to the run's ``flakes.json``: postsubmit reads
    it so a test that passed alone never reads as main's failure, and the
    nodes are what a flake owner routes."""
    from molt.file_publication import atomic_write_bytes

    path = flakes_path(runtime.output_dir)
    try:
        records = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        records = []
    records.append(
        {"gate": display(gate.command), "tests": list(nodes), "evidence": str(evidence)}
    )
    atomic_write_bytes(path, (json.dumps(records, indent=2) + "\n").encode("utf-8"))


def flakes_path(output_dir: Path) -> Path:
    return output_dir / "flakes.json"


def gate_evidence_directories(output_dir: Path) -> tuple[Path, ...]:
    """The known attempt namespaces of one run, never arbitrary descendants.

    Outer postsubmit continuations retain their own heavy rounds and test
    isolation. Every reader uses this inventory so a later failed test and a
    passed retry's flake record retain the same evidence scope.
    """
    roots = (output_dir, *_numbered_attempt_directories(output_dir, "postsubmit-rest-"))
    directories: list[Path] = []
    for root in roots:
        directories.extend((root, root / "heavy-retry", root / "heavy-rest"))
        directories.extend(_numbered_attempt_directories(root, "heavy-rest-"))
        for state in sorted(root.glob("gate-*.isolation")):
            if (
                not state.name.removeprefix("gate-")
                .removesuffix(".isolation")
                .isdecimal()
            ):
                continue
            directories.append(state)
            directories.extend(_numbered_attempt_directories(state, "isolate-"))
            directories.extend(_numbered_attempt_directories(state, "continue-"))
    for directory in tuple(directories):
        directories.extend(_numbered_attempt_directories(directory, "attempt-"))
    return tuple(path for path in directories if path.is_dir())


def recorded_flaky_tests(
    output_dir: Path, *, directories: tuple[Path, ...] | None = None
) -> frozenset[str]:
    """The tests a retry of this run passed (``_record_flake``)."""
    nodes: set[str] = set()
    for directory in (
        gate_evidence_directories(output_dir) if directories is None else directories
    ):
        path = flakes_path(directory)
        if path.is_file():
            records = json.loads(path.read_text(encoding="utf-8"))
            nodes.update(node for record in records for node in record["tests"])
    return frozenset(nodes)


def _run_serial_gate(
    ordinal: int,
    gate: Gate,
    runtime: GateRuntime,
    gate_seconds: list[tuple[float, str]],
    *,
    collection: bool = False,
) -> None:
    cmd = gate.collection_command if collection else gate.command
    assert cmd is not None
    if not collection:
        cmd = _pytest_report_command(cmd, gate, runtime, ordinal, runtime.output_dir)
    assert isinstance(gate.cargo_target_access, CargoTargetAccess)
    phase = "collection" if collection else "gate"
    shown = display(cmd)
    runtime.check_source()
    paths = (
        ()
        if collection or gate.cargo_target_access is not CargoTargetAccess.SHARED
        else runtime.cargo_outputs.get(gate.toolchains, ())
    )
    required = runtime.capacity_requirements.get(gate.toolchains, 0)
    before = (
        runtime.observe_capacity(
            paths, required_bytes=required, phase="before", gate=shown
        )
        if paths
        else ()
    )
    started = time.monotonic()
    details = gate_details(ordinal, gate)
    if collection:
        details["command"] = {"kind": "argv", "argv": list(cmd)}
    runtime.progress.emit(f"{phase}_started", **details)
    rc = None
    outcome: GateOutcome | None = None
    classified: dict[str, Any] = {}
    streams, stream_error = prepare_gate_streams(
        runtime.output_dir / "collection" if collection else runtime.output_dir,
        ordinal,
    )
    record = stream_record(streams, stream_error)
    try:
        outcome = runtime.run_captured_gate(
            cmd,
            runtime.repo,
            runtime.environments[gate.toolchains],
            runtime.cancelled,
            streams,
        )
        classified = _classify_gate(
            outcome,
            runtime.toolchain_policies,
            cancel_requested=runtime.cancelled(),
            cargo_target_access=gate.cargo_target_access,
        )
        rc = outcome.returncode
    finally:
        elapsed = time.monotonic() - started
        runtime.progress.emit(
            f"{phase}_finished",
            **details,
            **record,
            **classified,
            returncode=rc,
            duration_seconds=elapsed,
        )
        if paths:
            runtime.observe_capacity(
                paths,
                required_bytes=required,
                phase="after",
                gate=shown,
                before=before,
            )
    # A cancelled gate reached no outcome; as in an executor block, its time
    # is no gate's result.
    if not collection and classified["status"] != "cancelled":
        gate_seconds.append((elapsed, shown))
    runtime.check_source()
    if classified["cancellation_cleanup"] is not None:
        if (
            runtime.cancelled()
            and not runtime.cancel_reason.stops_on_unresolved_closure
        ):
            raise GateCancellation(
                runtime.cancel_reason,
                unresolved=(f"{shown}: {classified['cancellation_cleanup']}",),
            )
        raise DriverError(
            f"guard cancellation cleanup failed: {classified['cancellation_cleanup']}"
        )
    if (
        classified["status"] in {"success", "cancelled"}
        and classified["cancellation_cleanup"] is None
    ):
        _check_cancellation(runtime)
    if classified["failure_scope"] == "global":
        raise DriverError(f"gate runner stopped: {classified['failure_reason']}")
    if rc != 0:
        runtime.say(f"    gate: {shown}")
        sys.stderr.write(gate_failure_report(outcome, record))
        sys.stderr.flush()
        timings = [*gate_seconds, (elapsed, shown)] if collection else gate_seconds
        raise gate_failure(rc, elapsed, shown, timings, record)
    if not collection:
        runtime.passed.append(gate)
        if streams is not None:
            runtime.passed_logs[display(gate.command)] = streams[0]
    runtime.ok(
        f"pytest collection passed in {elapsed:.1f} s (execution still required): {shown}"
        if collection
        else f"gate passed in {elapsed:.1f} s: {shown}"
    )


@dataclass(frozen=True)
class _ResourcePolicy:
    name: str
    max_parallel: int


@dataclass(frozen=True)
class _LocalGatePlan:
    receipt_schema: str
    executor_max_workers: int
    resource_policies: tuple[_ResourcePolicy, ...]
    toolchain_policies: tuple[Any, ...] = ()


@dataclass(frozen=True)
class _LocalGateCommand:
    id: str
    family: str
    data: dict[str, Any]
    argv: tuple[str, ...]
    gate: Any
    ordinal: int
    dependencies: tuple[str, ...] = ()
    evidence_outputs: tuple[str, ...] = ()


def _local_plan(
    runtime: GateRuntime, classes: dict[str, int], host: int
) -> _LocalGatePlan:
    ceiling = min(host, sum(classes.values()), runtime.executor_max_workers or host)

    def limit(name: str) -> int:
        if name == CARGO_GATE_RESOURCE_CLASS:
            return min(ceiling, _local_cargo_gates())
        return min(ceiling, runtime.resource_limits.get(name, ceiling))

    return _LocalGatePlan(
        receipt_schema=proof_receipts.RECEIPT_SCHEMA,
        executor_max_workers=ceiling,
        resource_policies=tuple(
            _ResourcePolicy(name, limit(name)) for name in sorted(classes)
        ),
    )


def _local_cargo_gates() -> int:
    """Cargo gates this host runs at once (``LOCAL_CARGO_GATE_BYTES`` each)."""
    from molt import host_capacity

    return min(
        LOCAL_CARGO_GATES,
        host_capacity.worker_count(
            bytes_per_worker=LOCAL_CARGO_GATE_BYTES,
            headroom_bytes=host_capacity.headroom_bytes(
                host_capacity.memory_capacity()
            ),
        ),
    )


def _memory_slice(
    plan: _LocalGatePlan, classes: dict[str, int], grant: HostWorkGrant | None
) -> int | None:
    """The memory one gate may claim while the executor runs others beside it.

    The proof executor's own rule (``proof_plan._command_memory_share``): the
    memory beyond the reserve, divided by the most gates that can run at once.
    """
    from molt import host_capacity

    capacity = host_capacity.memory_capacity(os.environ)
    _total, available = capacity.bounded_bytes
    if available is None and grant is None:
        return None
    slots = sum(
        min(classes[policy.name], policy.max_parallel)
        for policy in plan.resource_policies
    )
    concurrency = max(1, min(plan.executor_max_workers, slots))
    usable = (
        None
        if available is None
        else max(0, available - host_capacity.headroom_bytes(capacity))
    )
    if grant is not None:
        usable = (
            grant.memory_bytes if usable is None else min(usable, grant.memory_bytes)
        )
    assert usable is not None
    return max(1, usable // concurrency)


def _local_command(
    ordinal: int, gate: Gate, cell: str, *, repo: Path
) -> _LocalGateCommand:
    cmd = gate.command
    argv = ("/bin/sh", "-c", cmd) if isinstance(cmd, str) else tuple(cmd)
    pytest_gate = gate_runs_pytest(gate, repo=repo)
    if pytest_gate and isinstance(cmd, str):
        import shlex

        lexer = shlex.shlex(cmd, posix=True, punctuation_chars=";&|<>()")
        lexer.whitespace_split = True
        words = tuple(lexer)
        if any(word and set(word) <= set(";&|<>()") for word in words) or any(
            character in cmd for character in "$`\n"
        ):
            raise DriverError(
                "pytest gates require a simple argv; shell programs bypass worker allocation"
            )
        argv = words
    assert isinstance(gate.cargo_target_access, CargoTargetAccess)
    return _LocalGateCommand(
        id=f"local.{'always' if gate.concurrent else 'gate'}.{ordinal}",
        family=LOCAL_GATE_FAMILY,
        data={
            "cell": cell,
            "tiers": ["local-integration"],
            "resource_class": gate_resource_class(gate),
            "toolchains": list(gate.toolchains),
            "pytest": pytest_gate,
            "cargo_target_access": gate.cargo_target_access.value,
            # Local always gates keep their unbounded deadline.
            "timeout_budget": "none",
            "timeout_seconds": 0,
        },
        argv=argv,
        gate=gate,
        ordinal=ordinal,
    )


def _run_executor_block(
    gates: list[tuple[int, Gate]],
    runtime: GateRuntime,
    gate_seconds: list[tuple[float, str]],
    host: int,
    *,
    receipt_kind: str | None = None,
    log_dir: Path | None = None,
) -> None:
    """Run the block through the proof executor; report it in declared order.

    ``log_dir`` holds the block's gate logs; it defaults to the run's
    ``output_dir``. A block that reruns gates passes its own directory: gate
    streams are created exclusively, and a rerun keeps the first run's logs.
    """
    log_dir = runtime.output_dir if log_dir is None else log_dir
    from tools.proof_plan import _normalized_arch, _normalized_os

    classes: dict[str, int] = {}
    for _ordinal, gate in gates:
        name = gate_resource_class(gate)
        classes[name] = classes.get(name, 0) + 1
    grant = runtime.host_grant
    if grant is not None:
        host = min(host, grant.workers)
    plan = _local_plan(runtime, classes, host)
    cell = f"{_normalized_os()}-{_normalized_arch()}-local"
    commands = _shared_target_order(
        [
            _local_command(ordinal, gate, cell, repo=runtime.repo)
            for ordinal, gate in gates
        ]
    )
    outcomes: dict[str, GateOutcome | BaseException] = {}
    receipt_path = (
        runtime.receipt_path
        if receipt_kind is None
        else runtime.receipt_path.with_name(
            f"{runtime.receipt_path.stem}.{receipt_kind}{runtime.receipt_path.suffix}"
        )
    )
    from molt import host_capacity

    memory_slice = _memory_slice(plan, classes, grant)
    cargo_jobs = None
    pytest_cpu_budget = None
    if grant is not None:
        # Reserve a share for the non-pytest gates before dividing the remaining
        # CPUs among the concurrent pytest sessions.
        cargo_jobs = max(1, grant.workers // plan.executor_max_workers)
        occupied = min(
            sum(not command.data["pytest"] for command in commands),
            plan.executor_max_workers - 1,
        )
        pytest_cpu_budget = max(1, grant.workers - occupied * cargo_jobs)
    environments = {
        toolchains: (
            environment
            if memory_slice is None
            else environment.overlay(
                {host_capacity.MEMORY_SLICE_ENV: str(memory_slice)}
            )
        )
        for toolchains, environment in runtime.environments.items()
    }

    def run_command(
        _plan: Any,
        command: _LocalGateCommand,
        cancel: threading.Event,
        allocation: proof_executor.CommandAllocation,
    ) -> dict[str, Any]:
        record: dict[str, Any] = {
            **proof_receipts.command_record(command),
            "started_at": dt.datetime.now(dt.UTC).isoformat(),
            "duration_seconds": 0.0,
            "peak_rss_bytes": None,
            "cache_disposition": "none",
            "guard_metrics_schema": None,
            "returncode": None,
        }
        if cancel.is_set() or runtime.cancelled():
            return {
                **record,
                "status": "skipped",
                "skip_reason": "executor global stop before launch",
            }
        runtime.progress.emit(
            "gate_started", **gate_details(command.ordinal, command.gate)
        )
        shown = display(command.gate.command)
        paths = (
            runtime.cargo_outputs.get(command.gate.toolchains, ())
            if command.gate.cargo_target_access is CargoTargetAccess.SHARED
            else ()
        )
        required = runtime.capacity_requirements.get(command.gate.toolchains, 0)
        started = time.monotonic()
        attempt_dir = (
            log_dir
            if allocation.attempt == 1
            else log_dir / f"attempt-{allocation.attempt}"
        )
        streams, stream_error = prepare_gate_streams(attempt_dir, command.ordinal)
        record.update(stream_record(streams, stream_error))
        try:
            environment = environments[command.gate.toolchains].overlay(
                {
                    "MOLT_GUARD_PROFILE_LOG": str(
                        log_dir
                        / f"gate-{command.ordinal:03d}.attempt-{allocation.attempt}.profile.jsonl"
                    )
                }
            )
            if (
                cargo_jobs is not None
                and command.gate.cargo_target_access is not CargoTargetAccess.NONE
            ):
                requested_jobs = environment.environment.get("CARGO_BUILD_JOBS", "")
                if not requested_jobs or (
                    requested_jobs.isdecimal() and int(requested_jobs) > cargo_jobs
                ):
                    environment = environment.overlay(
                        {"CARGO_BUILD_JOBS": str(cargo_jobs)}
                    )
            argv = command.gate.command
            if allocation.pytest_cpus is not None:
                from tools.proof_command_argv import allocated_pytest_argv

                argv, workers = allocated_pytest_argv(
                    command.argv,
                    root=runtime.repo,
                    cpus=allocation.pytest_cpus,
                    environment=environment.environment,
                )
                record["pytest_workers"] = workers
            argv = _pytest_report_command(
                argv, command.gate, runtime, command.ordinal, attempt_dir
            )
            record["launch_argv"] = (
                list(argv) if not isinstance(argv, str) else ["/bin/sh", "-c", argv]
            )
            before = (
                runtime.observe_capacity(
                    paths, required_bytes=required, phase="before", gate=shown
                )
                if paths
                else ()
            )
            outcome = runtime.run_captured_gate(
                argv,
                runtime.repo,
                environment,
                lambda: cancel.is_set() or runtime.cancelled(),
                streams,
            )
            classified = _classify_gate(
                outcome,
                runtime.toolchain_policies,
                cancel_requested=cancel.is_set() or runtime.cancelled(),
                cargo_target_access=command.gate.cargo_target_access,
            )
            record["executed_argv"] = list(outcome.guard_result.args)
            record["child_cpu_seconds"] = outcome.guard_result.child_cpu_seconds
            if paths:
                runtime.observe_capacity(
                    paths,
                    required_bytes=required,
                    phase="after",
                    gate=shown,
                    before=before,
                )
        except Exception as exc:
            # The runner itself failed, not the gate: stop the block, and keep
            # this record (its streams and duration) for the receipt; the
            # declared-order report below raises the original error.
            outcomes[command.id] = exc
            from molt.process_spawn import ProcessLaunchError

            return {
                **record,
                "duration_seconds": round(time.monotonic() - started, 6),
                "status": "failure",
                "returncode": 2,
                "executor_error": f"{type(exc).__name__}: {exc}",
                "failure_scope": "global",
                "failure_reason": "the gate runner raised",
                **(
                    {"launch_failure": exc.json_payload()}
                    if isinstance(exc, ProcessLaunchError)
                    else {}
                ),
            }
        except BaseException as exc:  # an interrupt stops the block at once
            outcomes[command.id] = exc
            raise
        record["duration_seconds"] = round(time.monotonic() - started, 6)
        record["peak_rss_bytes"] = outcome.peak_rss_bytes
        record["returncode"] = outcome.returncode
        outcomes[command.id] = outcome
        return {**record, **classified}

    runtime.check_source()
    authority = hashlib.sha256(b"molt.local-integration-gates.v2\0")
    authority.update(runtime.policy_text.encode("utf-8"))
    block_started = time.monotonic()
    proof_executor.execute_commands(
        plan,
        commands,
        receipt_path,
        failure_policy=proof_executor.FailurePolicy.CANCEL_REMAINING,
        granted_cpus=pytest_cpu_budget,
        _source_tree_changes=runtime.source_changes,
        toolchain_fingerprints=lambda _policies, _names: dict(runtime.toolchains),
        _authority_sha256=lambda _plan: authority.hexdigest(),
        _source_identity=runtime.source_identity,
        _normalized_os=_normalized_os,
        _normalized_arch=_normalized_arch,
        _required_toolchains=lambda command: command.gate.toolchains,
        _run_command=run_command,
        _cache_disposition=lambda _command: "none",
    )
    wall = time.monotonic() - block_started
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    records = {record["id"]: record for record in receipt.get("commands", ())}

    def stopped(where: str) -> DriverError:
        # The runner, not the candidate, stopped the block.
        errors = "; ".join(receipt.get("errors", ()) or ("no reason recorded",))
        return DriverError(
            f"gate block stopped {where}: {errors}; receipt {receipt_path}"
        )

    runner_errors: list[BaseException] = []
    control_failures: list[str] = []
    # Control failures that are only the guard's memory stop with a closed tree.
    memory_stops: list[str] = []
    # Cancelled gates whose guard closure stayed indeterminate.
    unresolved: list[str] = []
    for command in commands:
        outcome = outcomes.get(command.id)
        record = records.get(command.id, {})
        status = record.get("status")
        shown = display(command.gate.command)
        if status in {"cancelled", "skipped"} and not isinstance(
            outcome, BaseException
        ):
            # Only a stop caused elsewhere cancels a gate; it never passed.
            # Its guard's closure verdict is evidence, never the block's.
            cleanup = record.get("cancellation_cleanup")
            runtime.say(
                f"    gate {status}: {shown}"
                + (f" (guard cleanup: {cleanup})" if cleanup else "")
            )
            if cleanup:
                unresolved.append(f"{shown}: {cleanup}")
            continue
        if outcome is None:
            control_failures.append(f"before {shown}")
            continue
        elapsed = float(record.get("duration_seconds") or 0.0)
        runtime.progress.emit(
            "gate_finished",
            **gate_details(command.ordinal, command.gate),
            **{
                key: record[key]
                for key in ("stdout_path", "stderr_path", "output_error")
                if key in record
            },
            returncode=record.get("returncode"),
            duration_seconds=elapsed,
        )
        if isinstance(outcome, BaseException) or outcome.returncode != 0:
            runtime.say(f"    gate: {shown}")
        if isinstance(outcome, GateOutcome) and outcome.returncode != 0:
            sys.stderr.write(gate_failure_report(outcome, record))
            sys.stderr.flush()
        gate_seconds.append((elapsed, shown))
        if isinstance(outcome, BaseException):
            runner_errors.append(outcome)
            continue
        # The executor observed the source after every completion; a gate
        # that changed it reports the canonical clean-source refusal.
        if record.get("source_changed") is True:
            runtime.check_source()
        if status == "failure" and outcome.returncode != 0:
            if record.get("failure_scope") == "global":
                control_failures.append(f"at {shown}: {record.get('failure_reason')}")
                guard = record.get("guard_outcome") or {}
                if (
                    guard.get("memory_violation") is not None
                    and outcome.guard_result.descendants_closed is True
                ):
                    memory_stops.append(shown)
            continue
        if status != "success":
            control_failures.append(
                f"at {shown}: {record.get('failure_reason') or status}"
            )
            continue
        runtime.passed.append(command.gate)
        if "stdout_path" in record:
            runtime.passed_logs[shown] = Path(record["stdout_path"])
        runtime.ok(f"gate passed in {elapsed:.1f} s: {shown}")
    if (
        runtime.cancelled()
        and not runner_errors
        and not control_failures
        and all(
            record.get("status") in {"success", "cancelled", "skipped"}
            and record.get("cancellation_cleanup") is None
            for record in records.values()
        )
        and all(
            isinstance(outcome, GateOutcome)
            and outcome.guard_result.descendants_closed is True
            for outcome in outcomes.values()
        )
    ):
        _check_cancellation(runtime)
    # The executor names the first gate that failed for its own reason. A
    # partition-scope trigger is the candidate's failure, which the landing
    # queue bisects; a runner or guard fault seen after it is evidence on
    # that failure, never a stop in its place (HF-605).
    trigger = receipt.get("execution", {}).get("failure_trigger")
    if trigger is not None and trigger.get("failure_scope") == "partition":
        command = next(
            command for command in commands if command.id == trigger["command_id"]
        )
        outcome = outcomes[command.id]
        record = records[command.id]
        assert isinstance(outcome, GateOutcome)
        failure = gate_failure(
            outcome.returncode,
            float(record.get("duration_seconds") or 0.0),
            display(command.gate.command),
            gate_seconds,
            record,
        )
        for evidence in (
            *(f"gate block also stopped at {item}" for item in unresolved),
            *(f"gate block also stopped {control}" for control in control_failures),
            *(f"gate runner also failed: {error!r}" for error in runner_errors),
        ):
            runtime.say(f"    {evidence}")
            failure.add_note(evidence)
        if runner_errors:
            failure.guard_errors = tuple(runner_errors)
        failure.clean_closure = not (unresolved or control_failures or runner_errors)
        failure.stop_cleanup_only = bool(unresolved) and not (
            control_failures or runner_errors
        )
        failure.shown = (display(command.gate.command),)
        raise failure
    if (
        unresolved
        and not runner_errors
        and not control_failures
        and runtime.cancelled()
        and not runtime.cancel_reason.stops_on_unresolved_closure
    ):
        raise GateCancellation(runtime.cancel_reason, unresolved=tuple(unresolved))
    control_failures.extend(f"at {item}" for item in unresolved)
    if runner_errors:
        error = runner_errors[0]
        if len(runner_errors) > 1:
            error.add_note(f"additional gate runner failures: {runner_errors[1:]!r}")
            error.guard_errors = tuple(runner_errors)
        raise error
    if control_failures:
        if len(memory_stops) == len(control_failures) and not unresolved:
            raise GateMemoryPressure(
                stopped("; ".join(control_failures)).args[0], tuple(memory_stops)
            )
        raise stopped("; ".join(control_failures))
    if receipt.get("status") != "success":
        raise stopped("without a classified gate failure")
    runtime.say(
        f"    gate block: {len(commands)} gates in {wall:.1f} s wall through the "
        f"proof executor, at most {plan.executor_max_workers} at once; "
        f"receipt {receipt_path}"
    )
