"""Guard-marker identity, publication, reconciliation and retirement.

``active/`` holds the markers of unresolved custody: a guard that may still
run, a child or process group that may still live, or a scratch lease whose
payload a process may still use. A marker leaves ``active/`` only after its
whole custody is resolved. It then moves to ``retired/``, which keeps a
bounded history. So every reader of ``active/`` costs O(live custody), not
O(history).

This module observes custody evidence; it has no process actuation
capability. Process identity is (pid, birth). A pid that a complete native
snapshot does not hold is dead, whatever its recorded birth. A pid that now
names another birth was reused, and its recorded process group closed before
the reuse: a kernel never gives a new process a pid that still names a live
group. A pid that is present without two comparable births is ambiguous.
Neither a path, a timestamp, nor a parent's watched-PID list establishes
process identity. Unreadable and ambiguous records stay protective, and the
reconciliation report names each one with the command that resolves it.
"""

from __future__ import annotations

from collections.abc import Callable, Collection, Generator, Mapping
from contextlib import contextmanager
from dataclasses import asdict, dataclass, replace
import os
from pathlib import Path
import re
import stat
from typing import Protocol

from molt import temporary_artifacts as _scratch
from molt.exact_json import encode_exact, loads_exact, read_exact, write_exact
from molt.file_locks import (
    _acquire_file_lock,
    _file_lock_owned_operation,
    _release_file_lock,
    _try_acquire_file_lock,
)
from molt.file_publication import (
    atomic_write_bytes,
    canonical_file_leaf,
    namespace_move_exclusive,
    metadata_is_link_like,
)
from molt.memory_guard_paths import retired_guard_marker_dir
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    capture_stable_regular_file,
)
from tools.memory_guard_core.common import utc_timestamp


ACTIVE_GUARD_MARKER_SCHEMA_VERSION = 2
TERMINAL_GUARD_STATUSES = frozenset(
    {"completed", "finalizer_completed", "custody_reconciled"}
)
_STATUSES = TERMINAL_GUARD_STATUSES | {
    "guard_starting",
    "launch_prepared",
    "spawn_pending",
    "spawn_failed",
    "child_running",
    "child_running_telemetry_degraded",
    "guard_exception",
    "cancellation_terminating",
    "timeout_terminating",
    "rss_limit_terminating",
    "guard_signal_terminating",
    "windows_job_draining",
    "finalizer_cleanup",
}
# "failed" records that the child launch raised: no child process exists.
_LAUNCH_STATES = ("not_started", "pending", "recorded", "failed")
_MARKER_NAME = re.compile(r"guard-([1-9][0-9]*)-([0-9a-f]{32})\.json")
_LOCK_NAME = re.compile(r"guard-[1-9][0-9]*-[0-9a-f]{32}\.lock")
_MAX_MARKER_BYTES = 16 * 1024 * 1024
# The retired history keeps this many resolved markers. Pruning starts only
# at the slack above it, so one retirement costs O(1) amortized.
RETIRED_GUARD_MARKER_KEEP = 256
_RETIRED_PRUNE_SLACK = 64
# An exiting guard sweeps active/ when it holds this many more markers than
# twice what the previous sweep left there. The gate grows with the records
# that stay (live guards), so a session's sweeps cost O(records) in total.
AUTO_SWEEP_GROWTH = 16
# Above this many markers, active/ still holds history from before markers
# retired. A guard never sweeps it; the operator migrates it once.
AUTO_SWEEP_LIMIT = 1024
_SWEEP_RECEIPT = "sweep.json"
_SWEEP_LOCK = "sweep.lock"
RECONCILED_REASON = "recorded_custody_absent_or_reused"
OPERATOR_REASON = "operator_attested"
# No live process is observed, but the evidence cannot prove closure.
OPERATOR_RESOLVABLE_REASONS = frozenset(
    {
        "child_launch_identity_unpublished",
        "guard_process_identity_unavailable",
        "child_process_identity_unavailable",
    }
)
CUSTODY_COMMAND = "python tools/memory_guard_custody.py"


class ActiveCustodyError(ValueError):
    """Custody cannot be established; no reconciliation may proceed."""


class ProcessSampleView(Protocol):
    """The read-only identity fields of one native process-table row."""

    @property
    def pid(self) -> int: ...

    @property
    def pgid(self) -> int | None: ...

    @property
    def started_at_ns(self) -> int | None: ...


@dataclass(frozen=True, slots=True)
class MarkerProcessIdentity:
    pid: int
    started_at_ns: int | None
    pgid: int | None = None


@dataclass(frozen=True, slots=True)
class MarkerRecord:
    path: Path
    identity: StableRegularFileIdentity | None = None
    payload: dict[str, object] | None = None
    guard: MarkerProcessIdentity | None = None
    child: MarkerProcessIdentity | None = None
    error: str | None = None
    # Process groups the producer found outliving its child.
    orphaned_groups: tuple[int, ...] = ()
    # The producer's closure verdict; None in records written before it.
    descendants_closed: bool | None = None

    @property
    def status(self) -> str | None:
        value = None if self.payload is None else self.payload.get("status")
        return value if isinstance(value, str) else None

    @property
    def closure_proven(self) -> bool:
        """The producer proved that no descendant outlives its run.

        A record written before ``descendants_closed`` existed proves it only
        when it lists no orphaned process group.
        """
        if self.descendants_closed is not None:
            return self.descendants_closed
        return not self.orphaned_groups

    @property
    def terminal(self) -> bool:
        """The producer or a reconciliation receipt closed this record.

        A producer publishes ``completed`` or ``finalizer_completed`` only
        after it reaped its child, so births do not decide its own claim; a
        fast child often exits before its birth can be read. The status closes
        the record only with proven descendant closure: otherwise orphaned
        processes may still run beside its artifacts, and the record stays
        unresolved until ``observe_custody`` sees every recorded group empty.
        A launch that never published its outcome is never closed by status.
        """
        if self.error is not None or self.status not in TERMINAL_GUARD_STATUSES:
            return False
        assert self.payload is not None
        if self.status == "custody_reconciled":
            return True
        return (
            self.payload.get("child_launch_state") != "pending" and self.closure_proven
        )

    @property
    def accepts_inherited_guard(self) -> bool:
        """A child may start before its producer publishes the child birth.

        This admits the launch token during that window; it is distinct from
        the stronger evidence required to release artifact protection.
        """
        return self.error is None and self.status not in TERMINAL_GUARD_STATUSES | {
            "finalizer_cleanup",
            "guard_exception",
            "spawn_failed",
        }


@dataclass(frozen=True, slots=True)
class ProcessCustodyEvidence:
    role: str
    pid: int
    expected_started_at_ns: int | None
    observed_started_at_ns: int | None
    state: str


@dataclass(frozen=True, slots=True)
class CustodyObservation:
    """One snapshot's verdict on a record's processes.

    ``state`` is "closed" (every recorded process and the child's group are
    gone), "live" (one is observed), or "ambiguous" (none is observed live,
    but the evidence cannot prove closure).
    """

    state: str
    reason: str
    evidence: tuple[ProcessCustodyEvidence, ...]
    # Recorded process groups that hold no member in the snapshot.
    empty_groups: tuple[int, ...] = ()


@dataclass(frozen=True, slots=True)
class ReconciliationDecision:
    marker: str
    disposition: str
    reason: str
    previous_status: str | None
    evidence: tuple[ProcessCustodyEvidence, ...] = ()
    empty_process_groups: tuple[int, ...] = ()
    applied: bool = False
    scratch: Mapping[str, object] | None = None
    retired_to: str | None = None
    # Why a resolved record stays in active/, when it does.
    retirement: str | None = None

    @property
    def operator_resolvable(self) -> bool:
        return self.disposition == "preserve" and (
            self.reason in OPERATOR_RESOLVABLE_REASONS
            or self.reason.startswith("marker_invalid: ")
        )


@dataclass(frozen=True, slots=True)
class ReconciliationReport:
    active_dir: str
    snapshot_processes: int
    apply: bool
    decisions: tuple[ReconciliationDecision, ...]
    removed_locks: int = 0
    scratch_retention: tuple[Mapping[str, object], ...] = ()

    @property
    def terminalized(self) -> int:
        return sum(
            item.applied and item.disposition == "terminalize"
            for item in self.decisions
        )

    @property
    def retired(self) -> int:
        return sum(item.retired_to is not None for item in self.decisions)

    @property
    def preserved(self) -> int:
        return sum(item.disposition == "preserve" for item in self.decisions)

    @property
    def remaining(self) -> int:
        """Records this pass left in active/."""
        return sum(item.retired_to is None for item in self.decisions)

    def next_steps(self) -> list[str]:
        """Name each record only an operator can resolve, with the command."""
        steps = []
        for item in self.decisions:
            if not item.operator_resolvable:
                continue
            steps.append(
                f"{item.marker}: {item.reason}. No live process of this run is "
                "observed, but the evidence cannot prove closure. After you "
                "confirm that no process of the run is alive, run: "
                f"{CUSTODY_COMMAND} --active-dir {self.active_dir} "
                f"--release {item.marker} --apply"
            )
        return steps

    def to_dict(self) -> dict[str, object]:
        return {
            **asdict(self),
            "terminalized": self.terminalized,
            "retired": self.retired,
            "preserved": self.preserved,
            "remaining": self.remaining,
            "next_steps": self.next_steps(),
        }


def _positive_int(value: object) -> bool:
    return type(value) is int and value > 0


def _process_identity(value: object) -> MarkerProcessIdentity:
    if not isinstance(value, dict) or not _positive_int(value.get("pid")):
        raise ActiveCustodyError("process_pid_invalid")
    birth, pgid = value.get("started_at_ns"), value.get("pgid")
    if birth is not None and not _positive_int(birth):
        raise ActiveCustodyError("process_birth_invalid")
    if pgid is not None and not _positive_int(pgid):
        raise ActiveCustodyError("process_group_invalid")
    return MarkerProcessIdentity(value["pid"], birth, pgid)


def _parse_payload(path: Path, payload: object) -> MarkerRecord:
    if not isinstance(payload, dict):
        raise ActiveCustodyError("marker_payload_not_object")
    match = _MARKER_NAME.fullmatch(path.name)
    if (
        match is None
        or not _positive_int(payload.get("pid"))
        or payload["pid"] != int(match[1])
        or payload.get("token") != match[2]
    ):
        raise ActiveCustodyError("marker_identity_invalid")
    if (
        type(payload.get("schema_version")) is not int
        or payload["schema_version"] != ACTIVE_GUARD_MARKER_SCHEMA_VERSION
    ):
        raise ActiveCustodyError("marker_schema_unsupported")
    status = payload.get("status")
    if not isinstance(status, str) or status not in _STATUSES:
        raise ActiveCustodyError("marker_status_invalid")
    guard = _process_identity(payload.get("guard_process"))
    if guard.pid != payload["pid"]:
        raise ActiveCustodyError("guard_identity_disagrees_with_marker")
    child_value = payload.get("child_process")
    child = None if child_value is None else _process_identity(child_value)
    launch = payload.get("child_launch_state")
    if launch not in _LAUNCH_STATES:
        raise ActiveCustodyError("child_launch_state_invalid")
    if (child is None) != (launch != "recorded"):
        raise ActiveCustodyError("child_launch_identity_inconsistent")
    if child is not None and child.pid == guard.pid:
        raise ActiveCustodyError("guard_and_child_identity_overlap")
    if launch == "not_started" and status not in {
        "guard_starting",
        "launch_prepared",
        "guard_exception",
        "completed",
        "finalizer_completed",
        "custody_reconciled",
    }:
        raise ActiveCustodyError("child_launch_status_inconsistent")
    if launch == "failed" and status not in {
        "spawn_failed",
        "guard_exception",
        "custody_reconciled",
    }:
        raise ActiveCustodyError("child_launch_status_inconsistent")
    closed = payload.get("descendants_closed")
    if closed is not None and type(closed) is not bool:
        raise ActiveCustodyError("descendants_closed_invalid")
    groups = payload.get("orphaned_process_groups")
    groups = [] if groups is None else groups
    if not isinstance(groups, list) or not all(_positive_int(g) for g in groups):
        raise ActiveCustodyError("orphaned_process_groups_invalid")
    record = MarkerRecord(
        path,
        payload=payload,
        guard=guard,
        child=child,
        orphaned_groups=tuple(sorted(set(groups))),
        descendants_closed=closed,
    )
    if status == "custody_reconciled":
        _validate_reconciliation(record)
    return record


def _validate_reconciliation(record: MarkerRecord) -> None:
    assert record.payload is not None and record.guard is not None
    receipt = record.payload.get("reconciliation")
    if not isinstance(receipt, dict):
        raise ActiveCustodyError("reconciliation_receipt_invalid")
    previous, reason = receipt.get("previous_status"), receipt.get("reason")
    if (
        type(receipt.get("schema_version")) is not int
        or receipt["schema_version"] != 1
        or not isinstance(previous, str)
        or previous not in _STATUSES
        or previous == "custody_reconciled"
        or reason not in (RECONCILED_REASON, OPERATOR_REASON)
        or not _positive_int(receipt.get("snapshot_processes"))
        or not isinstance(receipt.get("source_sha256"), str)
        or re.fullmatch(r"[0-9a-f]{64}", receipt["source_sha256"]) is None
    ):
        raise ActiveCustodyError("reconciliation_receipt_invalid")
    # A receipt written before group evidence existed lists none; it is valid
    # only for a record without orphaned groups.
    empty_groups = receipt.get("empty_process_groups", [])
    if (
        not isinstance(empty_groups, list)
        or not all(_positive_int(group) for group in empty_groups)
        or not set(record.orphaned_groups) <= set(empty_groups)
    ):
        raise ActiveCustodyError("reconciliation_group_evidence_invalid")
    identities = [("guard", record.guard)]
    if record.child is not None:
        identities.append(("child", record.child))
    evidence = receipt.get("evidence")
    if not isinstance(evidence, list) or len(evidence) != len(identities):
        raise ActiveCustodyError("reconciliation_evidence_invalid")
    for item, (role, identity) in zip(evidence, identities):
        if not isinstance(item, dict):
            raise ActiveCustodyError("reconciliation_evidence_invalid")
        expected = item.get("expected_started_at_ns")
        if (
            item.get("role") != role
            or not _positive_int(item.get("pid"))
            or item["pid"] != identity.pid
            or (expected is not None and not _positive_int(expected))
            or expected != identity.started_at_ns
        ):
            raise ActiveCustodyError("reconciliation_evidence_invalid")
        state, observed = item.get("state"), item.get("observed_started_at_ns")
        if state == "absent":
            valid = observed is None
        elif state == "identity_mismatch":
            valid = (
                expected is not None
                and _positive_int(observed)
                and observed != expected
            )
        elif state == "identity_unavailable":
            # Only an operator attestation may release inconclusive evidence.
            valid = (
                reason == OPERATOR_REASON
                and (observed is None or _positive_int(observed))
                and (expected is None or observed is None)
            )
        else:
            valid = False
        if not valid:
            raise ActiveCustodyError("reconciliation_evidence_invalid")


def read_marker_record(path: Path) -> MarkerRecord:
    identity = None
    payload = None
    try:
        identity, raw = capture_stable_regular_file(
            path, label="active guard marker", max_bytes=_MAX_MARKER_BYTES
        )
        payload = loads_exact(raw.decode("utf-8", errors="strict"))
        record = _parse_payload(path, payload)
        return replace(record, identity=identity)
    except (OSError, ValueError) as exc:
        return MarkerRecord(
            path,
            identity,
            payload if isinstance(payload, dict) else None,
            error=str(exc),
        )


def _direct_directory(directory: Path, *, role: str) -> bool:
    try:
        metadata = directory.lstat()
    except FileNotFoundError:
        return False
    if not stat.S_ISDIR(metadata.st_mode) or metadata_is_link_like(metadata):
        raise ActiveCustodyError(f"{role} directory is not direct: {directory}")
    return True


def read_marker_records(active_dir: Path) -> tuple[MarkerRecord, ...]:
    if not _direct_directory(active_dir, role="active marker"):
        return ()
    # Do not collapse enumeration/read failure into an empty custody set. Match
    # names before opening; a guard-shaped symlink, directory, or partial file
    # must produce a protective record rather than disappearing from the set.
    paths = sorted(
        path
        for path in active_dir.iterdir()
        if path.name.startswith("guard-") and path.suffix.lower() == ".json"
    )
    return tuple(read_marker_record(path) for path in paths)


def guard_marker_records(active_dir: Path, guard_pid: int) -> tuple[MarkerRecord, ...]:
    """Read one guard pid's records from ``active/`` and its retired history.

    Names are matched before any file is opened, so the cost is the two
    directory listings, not the records of other guards.
    """
    prefix = f"guard-{guard_pid}-"
    records: list[MarkerRecord] = []
    for directory, role in (
        (active_dir, "active marker"),
        (retired_guard_marker_dir(active_dir), "retired marker"),
    ):
        if not _direct_directory(directory, role=role):
            continue
        records.extend(
            read_marker_record(path)
            for path in sorted(directory.iterdir())
            if path.name.startswith(prefix) and path.suffix.lower() == ".json"
        )
    return tuple(records)


def active_guard_blockers(active_dir: Path) -> tuple[str, ...]:
    """Name every record that protects artifacts, filesystem-only."""
    try:
        records = read_marker_records(active_dir)
    except (OSError, ValueError) as exc:
        return (f"{active_dir}: {exc}",)
    return tuple(
        f"{record.path} ({record.status if record.error is None else record.error})"
        for record in records
        if not record.terminal
    )


def has_active_guard_marker(active_dir: Path) -> bool:
    """Filesystem-only protective projection shared with disk reclamation."""
    return bool(active_guard_blockers(active_dir))


@contextmanager
def _marker_lock(path: Path) -> Generator[Path]:
    canonical = canonical_file_leaf(path, create_parent=True, role="active marker")
    lock_path = canonical_file_leaf(
        canonical.with_suffix(".lock"), create_parent=True, role="active marker lock"
    )
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=2.0,
        timeout_message="active marker publication lock timed out",
    )
    try:
        with _file_lock_owned_operation(handle, expected_lock_path=lock_path):
            yield canonical
    finally:
        _release_file_lock(handle)


def write_active_guard_marker(path: Path, payload: Mapping[str, object]) -> None:
    """Exclusively publish one launch; leave every prior evidence record intact."""
    _parse_payload(path, dict(payload))
    with _marker_lock(path) as canonical:
        atomic_write_bytes(canonical, encode_exact(payload), exclusive=True)


def update_active_guard_marker(
    path: Path, token: str, *, status: str, **fields: object
) -> bool:
    """Serialize producer updates with reconciliation; never replace bad custody."""
    with _marker_lock(path) as canonical:
        record = read_marker_record(canonical)
        if (
            record.error is not None
            or record.payload is None
            or record.payload.get("token") != token
            or record.status == "custody_reconciled"
        ):
            return False
        immutable = {"schema_version", "pid", "token", "guard_process", "created_at"}
        if immutable.intersection(fields):
            raise ActiveCustodyError(
                "producer update changes immutable marker identity"
            )
        payload = {
            **record.payload,
            **fields,
            "status": status,
            "updated_at": utc_timestamp(),
        }
        if fields.get("child_process") is not None:
            payload["child_launch_state"] = "recorded"
        updated = _parse_payload(canonical, payload)
        if record.child is not None and (
            updated.child is None
            or updated.child.pid != record.child.pid
            or updated.child.pgid != record.child.pgid
            or (
                record.child.started_at_ns is not None
                and updated.child.started_at_ns != record.child.started_at_ns
            )
        ):
            raise ActiveCustodyError("producer update changes recorded child identity")
        previous_launch = record.payload.get("child_launch_state")
        launch = payload["child_launch_state"]
        if launch != previous_launch and (
            launch == "not_started"
            or previous_launch in {"recorded", "failed"}
            or (launch == "failed" and previous_launch != "pending")
        ):
            raise ActiveCustodyError("producer update reverses child launch boundary")
        atomic_write_bytes(canonical, encode_exact(payload))
    return True


def _process_evidence(
    role: str, identity: MarkerProcessIdentity, samples: Mapping[int, ProcessSampleView]
) -> ProcessCustodyEvidence:
    sample = samples.get(identity.pid)
    observed = None if sample is None else sample.started_at_ns
    expected = identity.started_at_ns
    state = (
        "absent"
        if sample is None
        else "identity_unavailable"
        if observed is None or expected is None
        else "identity_match"
        if observed == expected
        else "identity_mismatch"
    )
    return ProcessCustodyEvidence(role, identity.pid, expected, observed, state)


def observe_custody(
    record: MarkerRecord, samples: Mapping[int, ProcessSampleView]
) -> CustodyObservation:
    """Judge a parsed record's guard, child and child group in one snapshot."""
    assert record.error is None and record.guard is not None
    identities = [("guard", record.guard)]
    if record.child is not None:
        identities.append(("child", record.child))
    evidence = tuple(
        _process_evidence(role, identity, samples) for role, identity in identities
    )
    for item in evidence:
        if item.state == "identity_match":
            return CustodyObservation(
                "live", f"{item.role}_process_identity_match", evidence
            )
    present_groups = {sample.pgid for sample in samples.values()}
    child = record.child
    if child is not None and child.pgid is not None:
        # A reused leader pid proves its old group empty: the kernel does not
        # give a new process a pid that still names a live group.
        leader_reused = (
            evidence[-1].state == "identity_mismatch" and child.pgid == child.pid
        )
        if not leader_reused and child.pgid in present_groups:
            return CustodyObservation(
                "live", "child_process_group_still_present", evidence
            )
    # Groups the producer saw outlive its child, while its closure is open.
    # Their leaders are not recorded, so only an empty group proves closure.
    open_groups = () if record.terminal else record.orphaned_groups
    if present_groups.intersection(open_groups):
        return CustodyObservation(
            "live", "orphaned_process_group_still_present", evidence
        )
    recorded_groups = set(open_groups)
    if child is not None and child.pgid is not None:
        recorded_groups.add(child.pgid)
    empty_groups = tuple(sorted(recorded_groups - present_groups))
    if record.payload is not None and (
        record.payload.get("child_launch_state") == "pending"
    ):
        # The guard died between the launch boundary and the child record.
        return CustodyObservation(
            "ambiguous", "child_launch_identity_unpublished", evidence, empty_groups
        )
    for item in evidence:
        if item.state == "identity_unavailable":
            return CustodyObservation(
                "ambiguous",
                f"{item.role}_process_identity_unavailable",
                evidence,
                empty_groups,
            )
    return CustodyObservation("closed", RECONCILED_REASON, evidence, empty_groups)


def _scratch_closure(
    observation: CustodyObservation, *, authority: str, snapshot_processes: int
) -> dict[str, object]:
    return {
        "schema": "molt.guard-scratch-closure.v1",
        "authority": authority,
        "closed": True,
        "reason": observation.reason,
        "snapshot_processes": snapshot_processes,
        "process_evidence": [asdict(item) for item in observation.evidence],
        "empty_process_groups": list(observation.empty_groups),
    }


def _scratch_generation(record: MarkerRecord) -> Path | None:
    assert record.payload is not None
    outcome = record.payload.get("temporary_artifacts")
    if not isinstance(outcome, Mapping):
        return None
    return _scratch.scratch_generation(str(record.payload["token"]), outcome)


def _resolve_scratch(
    record: MarkerRecord, marker: Path, closure: Mapping[str, object] | None
) -> Mapping[str, object] | None:
    try:
        generation = _scratch_generation(record)
    except ValueError as exc:
        return {"state": "error", "resolved": False, "error": str(exc)}
    if generation is None:
        return None
    return _scratch.resolve_guard_scratch(
        generation, guard_marker=marker, closure=closure
    )


def _inspect_scratch(record: MarkerRecord) -> Mapping[str, object] | None:
    try:
        generation = _scratch_generation(record)
    except ValueError as exc:
        return {"state": "error", "error": str(exc)}
    if generation is None:
        return None
    return {
        "generation": str(generation),
        "state": _scratch.inspect_guard_scratch(generation),
    }


def _publish_retired(canonical: Path) -> Path:
    """Move a resolved record into history; no durability barrier.

    A crash that rolls the move back leaves a terminal record in active/,
    which blocks nothing and which the next sweep retires again.
    """
    retired = retired_guard_marker_dir(canonical.parent)
    retired.mkdir(exist_ok=True)
    destination = retired / canonical.name
    namespace_move_exclusive(canonical, destination)
    return destination


def _discard_marker_lock(lock: Path) -> bool:
    """Delete the lock of a marker that left active/.

    A later holder of this lock finds no marker and writes nothing: markers
    are published exclusively under fresh tokens. Windows refuses to delete
    a lock that a contender holds open; the next full sweep removes it.
    """
    try:
        lock.unlink(missing_ok=True)
    except PermissionError:
        return False
    return True


def _prune_retired(retired: Path) -> None:
    try:
        entries = [
            entry for entry in os.scandir(retired) if _MARKER_NAME.fullmatch(entry.name)
        ]
    except FileNotFoundError:
        return
    if len(entries) <= RETIRED_GUARD_MARKER_KEEP + _RETIRED_PRUNE_SLACK:
        return
    # A retired marker keeps its last-write time, which orders the history.
    entries.sort(
        key=lambda entry: (entry.stat(follow_symlinks=False).st_mtime_ns, entry.name)
    )
    for entry in entries[: len(entries) - RETIRED_GUARD_MARKER_KEEP]:
        Path(entry.path).unlink(missing_ok=True)


def _retire(
    path: Path,
    accepts: Callable[[MarkerRecord], bool],
    decision: ReconciliationDecision,
    closure: Mapping[str, object] | None,
) -> ReconciliationDecision:
    """Move one terminal record out of active/ once its scratch resolves.

    ``accepts`` binds the record read under the lock to the caller's
    evidence: the generation a reconciliation judged, or the producer token.
    """
    scratch: Mapping[str, object] | None = None
    try:
        with _marker_lock(path) as canonical:
            current = read_marker_record(canonical)
            if current.error is not None or not accepts(current):
                return replace(
                    decision, retirement="marker_changed_during_reconciliation"
                )
            if not current.terminal:
                return replace(decision, retirement="marker_not_terminal")
            scratch = _resolve_scratch(current, canonical, closure)
            if scratch is not None and not scratch["resolved"]:
                return replace(
                    decision, scratch=scratch, retirement=f"scratch_{scratch['state']}"
                )
            destination = _publish_retired(canonical)
    except (OSError, ValueError, RuntimeError) as exc:
        return replace(
            decision, scratch=scratch, retirement=f"marker_retirement_failed: {exc}"
        )
    _discard_marker_lock(canonical.with_suffix(".lock"))
    _prune_retired(destination.parent)
    return replace(decision, scratch=scratch, retired_to=str(destination))


def retire_active_guard_marker(path: Path, token: str) -> Path | None:
    """The producer retires its own resolved marker as its last act.

    The marker stays when the guard raised (a non-terminal status) or when
    its scratch is leased or indeterminate; a later sweep resolves both.
    """
    decision = _retire(
        path,
        lambda record: (
            record.payload is not None and record.payload.get("token") == token
        ),
        ReconciliationDecision(str(path), "retire", "terminal_status", None),
        None,
    )
    return None if decision.retired_to is None else Path(decision.retired_to)


def _publish_reconciliation(
    record: MarkerRecord,
    decision: ReconciliationDecision,
    *,
    timestamp: str,
    snapshot_processes: int,
) -> tuple[ReconciliationDecision, StableRegularFileIdentity | None]:
    try:
        with _marker_lock(record.path) as canonical:
            current = read_marker_record(canonical)
            if current.identity != record.identity or current.error is not None:
                return (
                    replace(
                        decision,
                        disposition="preserve",
                        reason="marker_changed_during_reconciliation",
                    ),
                    None,
                )
            assert current.payload is not None and record.identity is not None
            payload = {
                **current.payload,
                "status": "custody_reconciled",
                "updated_at": timestamp,
                "reconciliation": {
                    "schema_version": 1,
                    "reconciled_at": timestamp,
                    "snapshot_processes": snapshot_processes,
                    "previous_status": decision.previous_status,
                    "reason": decision.reason,
                    "source_sha256": record.identity.sha256,
                    "evidence": [asdict(item) for item in decision.evidence],
                    "empty_process_groups": list(decision.empty_process_groups),
                },
            }
            _parse_payload(canonical, payload)
            atomic_write_bytes(canonical, encode_exact(payload))
            published = read_marker_record(canonical)
    except (OSError, ValueError, RuntimeError) as exc:
        return (
            replace(
                decision,
                disposition="preserve",
                reason=f"marker_publication_failed: {exc}",
            ),
            None,
        )
    return replace(decision, applied=True), published.identity


def _release_invalid(
    record: MarkerRecord, decision: ReconciliationDecision
) -> ReconciliationDecision:
    """Move an operator-released unreadable record, byte for byte, to history."""
    try:
        with _marker_lock(record.path) as canonical:
            current = read_marker_record(canonical)
            if current.identity is None or current.identity != record.identity:
                return replace(
                    decision, retirement="marker_changed_during_reconciliation"
                )
            destination = _publish_retired(canonical)
    except (OSError, ValueError, RuntimeError) as exc:
        return replace(decision, retirement=f"marker_retirement_failed: {exc}")
    _discard_marker_lock(canonical.with_suffix(".lock"))
    _prune_retired(destination.parent)
    return replace(decision, applied=True, retired_to=str(destination))


def _reconcile_record(
    record: MarkerRecord,
    samples: Mapping[int, ProcessSampleView],
    *,
    apply: bool,
    operator: bool,
    timestamp: str,
) -> ReconciliationDecision:
    path = str(record.path)
    if record.error is not None:
        decision = ReconciliationDecision(
            path, "preserve", f"marker_invalid: {record.error}", record.status
        )
        if operator and apply:
            if record.identity is None:
                return replace(
                    decision, retirement="marker_is_not_a_regular_file; inspect it"
                )
            return _release_invalid(record, replace(decision, disposition="release"))
        return decision
    observation = observe_custody(record, samples)
    closure = None
    if observation.state == "closed" or (operator and observation.state == "ambiguous"):
        closure = _scratch_closure(
            observation,
            authority="operator-attested" if operator else "reconciled-custody",
            snapshot_processes=len(samples),
        )
    if record.terminal:
        guard = observation.evidence[0]
        if guard.state in {"identity_match", "identity_unavailable"}:
            # The producer is still finishing; it retires its own marker.
            return ReconciliationDecision(
                path,
                "already_terminal",
                f"guard_process_{guard.state}",
                record.status,
                observation.evidence,
            )
        if observation.state == "live":
            # A live child or child-group member is unresolved custody.
            return ReconciliationDecision(
                path,
                "already_terminal",
                observation.reason,
                record.status,
                observation.evidence,
            )
        decision = ReconciliationDecision(
            path, "retire", "terminal_status", record.status, observation.evidence
        )
    elif closure is not None:
        decision = ReconciliationDecision(
            path,
            "terminalize",
            OPERATOR_REASON if operator else observation.reason,
            record.status,
            observation.evidence,
            observation.empty_groups,
        )
    else:
        return ReconciliationDecision(
            path, "preserve", observation.reason, record.status, observation.evidence
        )
    if not apply:
        scratch = _inspect_scratch(record)
        if (
            scratch is not None
            and scratch.get("state") in {"leased", "indeterminate"}
            and closure is None
        ):
            decision = replace(decision, retirement=f"scratch_{scratch['state']}")
        return replace(decision, scratch=scratch)
    expected = record.identity
    if decision.disposition == "terminalize":
        decision, expected = _publish_reconciliation(
            record, decision, timestamp=timestamp, snapshot_processes=len(samples)
        )
        if not decision.applied:
            return decision
    decision = _retire(
        record.path, lambda current: current.identity == expected, decision, closure
    )
    if decision.disposition == "retire":
        if decision.retired_to is None:
            return replace(decision, disposition="already_terminal")
        return replace(decision, applied=True)
    return decision


def _validated_snapshot(
    snapshot: Callable[[], Mapping[int, ProcessSampleView]],
) -> dict[int, ProcessSampleView]:
    samples = dict(snapshot())
    if not samples:
        raise ActiveCustodyError("process snapshot contains no usable rows")
    for pid, sample in samples.items():
        if (
            not _positive_int(pid)
            or type(sample.pid) is not int
            or sample.pid != pid
            or (
                sample.started_at_ns is not None
                and not _positive_int(sample.started_at_ns)
            )
            or (
                sample.pgid is not None
                and (type(sample.pgid) is not int or sample.pgid < 0)
            )
        ):
            raise ActiveCustodyError("process snapshot contains invalid identity rows")
    return samples


def _release_records(
    active_dir: Path, release: Collection[Path]
) -> tuple[MarkerRecord, ...]:
    if not _direct_directory(active_dir, role="active marker"):
        raise ActiveCustodyError(f"active marker directory is absent: {active_dir}")
    root = active_dir.resolve(strict=True)
    records = []
    for raw in release:
        path = Path(raw).absolute()
        if (
            _MARKER_NAME.fullmatch(path.name) is None
            or path.parent.resolve(strict=False) != root
        ):
            raise ActiveCustodyError(
                f"release names no guard marker in {active_dir}: {raw}"
            )
        if not os.path.lexists(path):
            raise ActiveCustodyError(f"release names no existing marker: {raw}")
        records.append(read_marker_record(root / path.name))
    return tuple(records)


def _discard_orphan_locks(active_dir: Path) -> int:
    """Delete the locks of markers that already left active/."""
    removed = 0
    for lock in sorted(active_dir.iterdir()):
        if _LOCK_NAME.fullmatch(lock.name) is None:
            continue
        marker = lock.with_suffix(".json")
        if os.path.lexists(marker):
            continue
        handle = _try_acquire_file_lock(lock)
        if handle is None:
            continue
        try:
            # A producer creates the lock before its marker; it holds the
            # lock until the marker exists, so recheck under the lock.
            orphan = not os.path.lexists(marker)
        finally:
            _release_file_lock(handle)
        if orphan and _discard_marker_lock(lock):
            removed += 1
    return removed


def reconcile_active_guard_markers(
    active_dir: Path,
    snapshot: Callable[[], Mapping[int, ProcessSampleView]],
    *,
    apply: bool = False,
    release: Collection[Path] = (),
) -> ReconciliationReport:
    """Read generations, take one snapshot, then optionally resolve records.

    A record with proven closure becomes ``custody_reconciled``. A terminal
    record whose guard is gone, and whose scratch resolves, leaves active/.
    ``release`` names records an operator attests closed: inconclusive and
    unreadable evidence then resolves too, but live evidence never does.

    Capturing records before the snapshot prevents an older snapshot from
    classifying a newly launched guard as absent. Publication holds the same
    lock as the producer and compares the full physical/content generation.
    The injected snapshot must be the complete native process table, never a
    selected group, cache, failed query, or empty result.
    """
    records = (
        _release_records(active_dir, release)
        if release
        else read_marker_records(active_dir)
    )
    samples = _validated_snapshot(snapshot)
    timestamp = utc_timestamp()
    decisions = tuple(
        _reconcile_record(
            record, samples, apply=apply, operator=bool(release), timestamp=timestamp
        )
        for record in records
    )
    removed_locks = 0
    retention: list[Mapping[str, object]] = []
    if apply:
        # Adopted payloads are retained failures; apply the retention bound
        # once per scratch root, outside every marker lock.
        roots = sorted(
            {
                Path(str(item.scratch["generation"])).parent
                for item in decisions
                if item.scratch is not None and "generation" in item.scratch
            }
        )
        retention = [
            {"root": str(root), **_scratch.reclaim_terminal_scratch(root)}
            for root in roots
            if root.is_dir()
        ]
        if not release:
            removed_locks = _discard_orphan_locks(active_dir)
    return ReconciliationReport(
        str(active_dir.absolute()),
        len(samples),
        apply,
        decisions,
        removed_locks,
        tuple(retention),
    )


def _marker_count(active_dir: Path, *, stop_after: int) -> int:
    count = 0
    try:
        with os.scandir(active_dir) as entries:
            for entry in entries:
                if _MARKER_NAME.fullmatch(entry.name):
                    count += 1
                    if count >= stop_after:
                        break
    except FileNotFoundError:
        return 0
    return count


def _sweep_baseline(state_root: Path) -> int:
    """Return how many records the previous sweep left, or 0 if unknown."""
    try:
        receipt = read_exact(
            state_root / _SWEEP_RECEIPT, max_bytes=1024 * 1024, label="custody sweep"
        )
    except (OSError, ValueError):
        return 0
    remaining = receipt.get("remaining") if isinstance(receipt, dict) else None
    return remaining if type(remaining) is int and remaining >= 0 else 0


def record_custody_sweep(report: ReconciliationReport) -> None:
    """Publish what one applied full sweep left, for the next guard's gate."""
    state_root = Path(report.active_dir).parent
    write_exact(
        state_root / _SWEEP_RECEIPT,
        {
            "schema_version": 1,
            "swept_at": utc_timestamp(),
            "remaining": report.remaining,
            "next_steps": report.next_steps(),
        },
    )


def sweep_active_guard_markers(
    active_dir: Path, snapshot: Callable[[], Mapping[int, ProcessSampleView]]
) -> ReconciliationReport | None:
    """Reconcile active/ after a guard exits, when enough records gathered.

    The gate reads at most ``AUTO_SWEEP_LIMIT`` + 1 directory entries, so an
    exiting guard never pays for history. Returns None when no sweep ran.
    """
    state_root = active_dir.parent
    count = _marker_count(active_dir, stop_after=AUTO_SWEEP_LIMIT + 1)
    # Read the previous sweep's receipt only when a sweep can be due.
    if (
        count <= AUTO_SWEEP_GROWTH
        or count > AUTO_SWEEP_LIMIT
        or count <= 2 * _sweep_baseline(state_root) + AUTO_SWEEP_GROWTH
    ):
        return None
    handle = _try_acquire_file_lock(state_root / _SWEEP_LOCK)
    if handle is None:
        return None  # Another guard is sweeping the same records.
    try:
        samples = dict(snapshot())
        # A complete native table holds its reader. This refuses a selected,
        # cached or substituted table before it can retire live custody.
        if os.getpid() not in samples:
            raise ActiveCustodyError("process snapshot omits its own reader")
        report = reconcile_active_guard_markers(active_dir, lambda: samples, apply=True)
        record_custody_sweep(report)
    finally:
        _release_file_lock(handle)
    return report
