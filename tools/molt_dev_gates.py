"""Landing-gate execution for ``tools/molt_dev.py integrate``.

Explicit pytest nodes collect first under the same environment without executing
test bodies or replacing mandatory gates. The first observed gate failure cancels
remaining work through the existing guards. Outcomes report in declared order.
A gate's nonzero exit raises ``GateFailure``, a ``CandidateFailure``
the landing queue bisects on; a stop of the runner itself stays a plain
``DriverError``. Every gate executes through the one proof executor
(``tools/proof_executor.py``) in its plan resource class: gates that may build
with Cargo share one target and run one at a time, pytest sessions run beside
them in the memory slice the executor exports, and read-only checks fill the
rest, all bounded by the plan's executor ceiling and ``molt.host_capacity``
(docs/design/host_capacity.md, "One scheduler"). The block leaves a receipt
with each gate's duration and peak RSS. Every gate writes its streams to files
while it runs (`prepare_gate_streams`); the terminal shows a failure's bounded
tail.
"""

from __future__ import annotations

from tools.molt_dev_policy import GateEnvironment
from tools.proof_plan import CargoTargetAccess

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, replace
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import threading
import time
from typing import TYPE_CHECKING, Any, Protocol

from tools import harness_memory_guard, proof_executor, proof_receipts
from tools.driver_errors import DriverError, GateFailure  # noqa: E402
from tools.molt_dev_common import _say, _warn  # noqa: E402
from tools.memory_guard_core.reporting import NAMED_RECORD_MIN_AGE_S

if TYPE_CHECKING:
    from molt.disk_capacity import DiskCapacityProbe
    from tools.proof_plan import ToolchainPolicy

# Local always gates share the repository-policy resource class with their CI
# commands; a policy without that class admits up to the executor ceiling.
GATE_RESOURCE_CLASS = "repository-policy"
# Every gate that may build with Cargo shares one Cargo target, so those gates
# run one at a time (a correctness bound, not capacity); pytest sessions size
# their workers within the memory slice the executor exports to each.
CARGO_GATE_RESOURCE_CLASS = "compiler-build-resource"
PYTEST_GATE_RESOURCE_CLASS = "python-tests"
LOCAL_GATE_FAMILY = "local_integration"
# The largest always gate's process tree peaks below 1 GiB (structural_audit
# 0.69 GB, generators 0.50 GB on the 2026-10 tree): the host-capacity unit.
CONCURRENT_GATE_BYTES = 1 << 30


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
    outcome: GateOutcome, policies: tuple[ToolchainPolicy, ...]
) -> dict[str, Any]:
    """Project the actual guard result through the shared failure authority."""
    from tools.proof_plan import _admitted_linker_helpers, _guarded_failure_scope

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
    scope, reason = _guarded_failure_scope(
        metrics,
        metrics_valid=True,
        returncode=guard.returncode,
        cancelled=guard.cancelled,
        admitted_linker_helpers=_admitted_linker_helpers(policies),
    )
    if reason == "command failed":
        reason = f"exit {guard.returncode}"
    return {
        "status": (
            "cancelled"
            if reason == "executor cancellation"
            else "failure"
            if scope == "global" or guard.returncode != 0
            else "success"
        ),
        "failure_scope": scope,
        "failure_reason": reason,
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
    return receipt_path.with_suffix(".d") / f"{stamp}-{os.getpid()}"


def retire_gate_logs(receipts: Path, *, now: float) -> None:
    """Delete gate run directories idle past the named-record window, then
    the receipt directories they leave empty; a landing calls this once and
    warns when one cannot go. Retiring whole receipts belongs to HF-435."""
    failures: list[str] = []
    for run in receipts.glob("*.d/*"):
        try:
            if run.is_dir() and now - run.stat().st_mtime > NAMED_RECORD_MIN_AGE_S:
                shutil.rmtree(run)
                if not any(run.parent.iterdir()):
                    run.parent.rmdir()
        except OSError as exc:
            failures.append(f"{run}: {type(exc).__name__}: {exc}")
    if failures:
        _warn(
            f"{len(failures)} idle gate log run(s) could not be retired; "
            f"first: {failures[0]}"
        )


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
    return GateFailure(
        f"gate failed (exit {rc}) after {elapsed:.1f} s: {shown}\n"
        + kept
        + gate_time_summary(gate_seconds)
        + "\n    Integration halts before push; fix the gate and re-run."
    )


def concurrent_gate_limit() -> int:
    """Gates this host runs at once, from the host capacity authority."""
    from molt import host_capacity

    return host_capacity.worker_count(
        bytes_per_worker=CONCURRENT_GATE_BYTES,
        headroom_bytes=host_capacity.headroom_bytes(host_capacity.memory_capacity()),
    )


def admit_selected_output_capacity(
    outputs: dict[tuple[str, ...], tuple[Path, ...]],
    environments: dict[tuple[str, ...], GateEnvironment],
) -> dict[tuple[str, ...], int]:
    """Check current volumes before any gate, without forecasting growth."""
    from molt.disk_capacity import minimum_headroom_bytes, require_build_capacity

    requirements = {}
    observed: set[tuple[tuple[Path, ...], int]] = set()
    for tools, paths in outputs.items():
        environment = environments[tools].environment
        required = minimum_headroom_bytes(environment)
        requirements[tools] = required
        key = (paths, required)
        if key in observed:
            continue
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
    Cargo; `_cargo_chain` serializes only shared-target writers."""
    if gate.pytest:
        return PYTEST_GATE_RESOURCE_CLASS
    if "cargo" in gate.toolchains:
        return CARGO_GATE_RESOURCE_CLASS
    return GATE_RESOURCE_CLASS


def _cargo_chain(commands: list[_LocalGateCommand]) -> list[_LocalGateCommand]:
    """Serialize shared-target writers without chaining tool availability.

    Isolated targets retain their child-owned output admission. Every pytest
    cohort still consumes its session and memory capacity; failure cancellation
    covers independent cohorts as well as this shared-target chain.
    """
    chained: list[_LocalGateCommand] = []
    previous: str | None = None
    for command in commands:
        if command.gate.cargo_target_access is CargoTargetAccess.SHARED:
            if previous is not None:
                command = replace(command, dependencies=(previous,))
            previous = command.id
        chained.append(command)
    return chained


def run_selected_gates(
    gates: Sequence[Gate],
    runtime: GateRuntime,
    gate_seconds: list[tuple[float, str]],
    *,
    host_limit: Callable[[], int] = concurrent_gate_limit,
) -> None:
    """Read-only checks first; then the heavy gates overlap in one block.

    The read-only checks run as before: the leading always gates as one
    executor block, then each rule check in declared order. A failing check
    therefore stops every build before it starts. The heavy gates (Cargo and
    pytest) then share one executor block: shared-target writers run one at a
    time while independent pytest sessions run beside them, each sized
    within the exported memory slice. The first observed failure within a
    phase halts the landing and cancels its remaining gates through their guards.
    """
    ordered = list(enumerate(gates, start=1))
    for ordinal, gate in ordered:
        if gate.collection_command is not None:
            _run_serial_gate(ordinal, gate, runtime, gate_seconds, collection=True)
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
    while block < len(light) and light[block][1].concurrent:
        block += 1
    if block >= 2:
        _run_executor_block(light[:block], runtime, gate_seconds, host_limit())
    else:
        block = 0
    for ordinal, gate in light[block:]:
        _run_serial_gate(ordinal, gate, runtime, gate_seconds)
    if len(heavy) >= 2:
        _run_executor_block(
            heavy, runtime, gate_seconds, host_limit(), receipt_kind="heavy"
        )
    else:
        for ordinal, gate in heavy:
            _run_serial_gate(ordinal, gate, runtime, gate_seconds)


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
            lambda: False,
            streams,
        )
        classified = _classify_gate(outcome, runtime.toolchain_policies)
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
    if not collection:
        gate_seconds.append((elapsed, shown))
    runtime.check_source()
    if classified["failure_scope"] == "global":
        raise DriverError(f"gate runner stopped: {classified['failure_reason']}")
    if rc != 0:
        runtime.say(f"    gate: {shown}")
        sys.stderr.write(gate_failure_report(outcome, record))
        sys.stderr.flush()
        timings = [*gate_seconds, (elapsed, shown)] if collection else gate_seconds
        raise gate_failure(rc, elapsed, shown, timings, record)
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
    return _LocalGatePlan(
        receipt_schema=proof_receipts.RECEIPT_SCHEMA,
        executor_max_workers=ceiling,
        resource_policies=tuple(
            _ResourcePolicy(
                name, min(ceiling, runtime.resource_limits.get(name, ceiling))
            )
            for name in sorted(classes)
        ),
    )


def _memory_slice(plan: _LocalGatePlan, classes: dict[str, int]) -> int | None:
    """The memory one gate may claim while the executor runs others beside it.

    The proof executor's own rule (``proof_plan._command_memory_share``): the
    memory beyond the reserve, divided by the most gates that can run at once.
    """
    from molt import host_capacity

    capacity = host_capacity.memory_capacity(os.environ)
    _total, available = capacity.bounded_bytes
    if available is None:
        return None
    slots = sum(
        min(classes[policy.name], policy.max_parallel)
        for policy in plan.resource_policies
    )
    concurrency = max(1, min(plan.executor_max_workers, slots))
    usable = max(0, available - host_capacity.headroom_bytes(capacity))
    return max(1, usable // concurrency)


def _local_command(ordinal: int, gate: Gate, cell: str) -> _LocalGateCommand:
    cmd = gate.command
    argv = ("/bin/sh", "-c", cmd) if isinstance(cmd, str) else tuple(cmd)
    assert isinstance(gate.cargo_target_access, CargoTargetAccess)
    return _LocalGateCommand(
        id=f"local.{'always' if gate.concurrent else 'gate'}.{ordinal}",
        family=LOCAL_GATE_FAMILY,
        data={
            "cell": cell,
            "tiers": ["local-integration"],
            "resource_class": gate_resource_class(gate),
            "toolchains": list(gate.toolchains),
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
) -> None:
    """Run the block through the proof executor; report it in declared order."""
    from tools.proof_plan import _normalized_arch, _normalized_os

    classes: dict[str, int] = {}
    for _ordinal, gate in gates:
        name = gate_resource_class(gate)
        classes[name] = classes.get(name, 0) + 1
    plan = _local_plan(runtime, classes, host)
    cell = f"{_normalized_os()}-{_normalized_arch()}-local"
    commands = _cargo_chain(
        [_local_command(ordinal, gate, cell) for ordinal, gate in gates]
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

    memory_slice = _memory_slice(plan, classes)
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
        _plan: Any, command: _LocalGateCommand, cancel: threading.Event
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
        if cancel.is_set():
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
        streams, stream_error = prepare_gate_streams(
            runtime.output_dir, command.ordinal
        )
        record.update(stream_record(streams, stream_error))
        try:
            before = (
                runtime.observe_capacity(
                    paths, required_bytes=required, phase="before", gate=shown
                )
                if paths
                else ()
            )
            outcome = runtime.run_captured_gate(
                command.gate.command,
                runtime.repo,
                environments[command.gate.toolchains],
                cancel.is_set,
                streams,
            )
            classified = _classify_gate(outcome, runtime.toolchain_policies)
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
            return {
                **record,
                "duration_seconds": round(time.monotonic() - started, 6),
                "status": "failure",
                "returncode": 2,
                "executor_error": f"{type(exc).__name__}: {exc}",
                "failure_scope": "global",
                "failure_reason": "the gate runner raised",
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
    for command in commands:
        outcome = outcomes.get(command.id)
        record = records.get(command.id, {})
        status = record.get("status")
        shown = display(command.gate.command)
        if status in {"cancelled", "skipped"} and not isinstance(
            outcome, BaseException
        ):
            # Only a stop caused elsewhere cancels a gate; it never passed.
            runtime.say(f"    gate {status}: {shown}")
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
        if record.get("source_tree_state_after") == "changed":
            runtime.check_source()
        if status == "failure" and outcome.returncode != 0:
            if record.get("failure_scope") == "global":
                control_failures.append(f"at {shown}: {record.get('failure_reason')}")
            continue
        if status != "success":
            control_failures.append(
                f"at {shown}: {record.get('failure_reason') or status}"
            )
            continue
        runtime.ok(f"gate passed in {elapsed:.1f} s: {shown}")
    if runner_errors:
        error = runner_errors[0]
        if len(runner_errors) > 1:
            error.add_note(f"additional gate runner failures: {runner_errors[1:]!r}")
            error.guard_errors = tuple(runner_errors)
        raise error
    if control_failures:
        raise stopped("; ".join(control_failures))
    trigger = receipt.get("execution", {}).get("failure_trigger")
    if trigger is not None:
        command = next(
            command for command in commands if command.id == trigger["command_id"]
        )
        outcome = outcomes[command.id]
        record = records[command.id]
        assert isinstance(outcome, GateOutcome)
        raise gate_failure(
            outcome.returncode,
            float(record.get("duration_seconds") or 0.0),
            display(command.gate.command),
            gate_seconds,
            record,
        )
    if receipt.get("status") != "success":
        raise stopped("without a classified gate failure")
    runtime.say(
        f"    gate block: {len(commands)} gates in {wall:.1f} s wall through the "
        f"proof executor, at most {plan.executor_max_workers} at once; "
        f"receipt {receipt_path}"
    )
