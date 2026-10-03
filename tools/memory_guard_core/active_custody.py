"""Shared active-marker identity, publication, and explicit reconciliation.

This module observes custody evidence; it has no process actuation capability.
Neither a PID, a path, a timestamp, nor a parent's watched-PID list establishes
process identity. Unreadable, indirect, legacy, or incomplete records protect
artifacts until their custody can be established. Evidence is never pruned.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator, Mapping
from contextlib import contextmanager
from dataclasses import asdict, dataclass, replace
from pathlib import Path
import re
import stat
from typing import Protocol

from molt.exact_json import encode_exact, loads_exact
from molt.file_locks import (
    _acquire_file_lock,
    _file_lock_owned_operation,
    _release_file_lock,
)
from molt.file_publication import (
    atomic_write_bytes,
    canonical_file_leaf,
    metadata_is_link_like,
)
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
_MARKER_NAME = re.compile(r"guard-([1-9][0-9]*)-([0-9a-f]{32})\.json")
_MAX_MARKER_BYTES = 16 * 1024 * 1024


class ActiveCustodyError(ValueError):
    """Custody cannot be established; no reconciliation may proceed."""


class ProcessSampleView(Protocol):
    pid: int
    pgid: int | None
    started_at_ns: int | None


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

    @property
    def status(self) -> str | None:
        value = None if self.payload is None else self.payload.get("status")
        return value if isinstance(value, str) else None

    @property
    def custody_error(self) -> str | None:
        if self.error is not None:
            return self.error
        if self.guard is None or self.guard.started_at_ns is None:
            return "guard_birth_identity_unavailable"
        if self.child is not None and self.child.started_at_ns is None:
            return "child_birth_identity_unavailable"
        if self.payload is None or self.payload.get("child_launch_state") == "pending":
            return "child_launch_identity_unpublished"
        return None

    @property
    def terminal(self) -> bool:
        return self.custody_error is None and self.status in TERMINAL_GUARD_STATUSES

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
    expected_started_at_ns: int
    observed_started_at_ns: int | None
    state: str


@dataclass(frozen=True, slots=True)
class ReconciliationDecision:
    marker: str
    disposition: str
    reason: str
    previous_status: str | None
    evidence: tuple[ProcessCustodyEvidence, ...] = ()
    applied: bool = False


@dataclass(frozen=True, slots=True)
class ReconciliationReport:
    active_dir: str
    snapshot_processes: int
    apply: bool
    decisions: tuple[ReconciliationDecision, ...]

    @property
    def terminalized(self) -> int:
        return sum(item.applied for item in self.decisions)

    @property
    def preserved(self) -> int:
        return sum(item.disposition == "preserve" for item in self.decisions)

    def to_dict(self) -> dict[str, object]:
        return {
            **asdict(self),
            "terminalized": self.terminalized,
            "preserved": self.preserved,
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
    if launch not in ("not_started", "pending", "recorded"):
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
    record = MarkerRecord(path, payload=payload, guard=guard, child=child)
    if status == "custody_reconciled":
        _validate_reconciliation(record)
    return record


def _validate_reconciliation(record: MarkerRecord) -> None:
    assert record.payload is not None and record.guard is not None
    receipt = record.payload.get("reconciliation")
    if not isinstance(receipt, dict):
        raise ActiveCustodyError("reconciliation_receipt_invalid")
    previous = receipt.get("previous_status")
    if (
        type(receipt.get("schema_version")) is not int
        or receipt["schema_version"] != 1
        or not isinstance(previous, str)
        or previous not in _STATUSES
        or previous in TERMINAL_GUARD_STATUSES
        or not _positive_int(receipt.get("snapshot_processes"))
        or not isinstance(receipt.get("source_sha256"), str)
        or re.fullmatch(r"[0-9a-f]{64}", receipt["source_sha256"]) is None
    ):
        raise ActiveCustodyError("reconciliation_receipt_invalid")
    identities = [("guard", record.guard)]
    if record.child is not None:
        identities.append(("child", record.child))
    evidence = receipt.get("evidence")
    if not isinstance(evidence, list) or len(evidence) != len(identities):
        raise ActiveCustodyError("reconciliation_evidence_invalid")
    for item, (role, identity) in zip(evidence, identities):
        if (
            not isinstance(item, dict)
            or item.get("role") != role
            or not _positive_int(item.get("pid"))
            or item["pid"] != identity.pid
            or not _positive_int(item.get("expected_started_at_ns"))
            or item["expected_started_at_ns"] != identity.started_at_ns
        ):
            raise ActiveCustodyError("reconciliation_evidence_invalid")
        state, observed = item.get("state"), item.get("observed_started_at_ns")
        if not (
            (state == "absent" and observed is None)
            or (
                state == "identity_mismatch"
                and _positive_int(observed)
                and observed != identity.started_at_ns
            )
        ):
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


def read_marker_records(active_dir: Path) -> tuple[MarkerRecord, ...]:
    try:
        metadata = active_dir.lstat()
    except FileNotFoundError:
        return ()
    if not stat.S_ISDIR(metadata.st_mode) or metadata_is_link_like(metadata):
        raise ActiveCustodyError(f"active marker directory is not direct: {active_dir}")
    # Do not collapse enumeration/read failure into an empty custody set. Match
    # names before opening; a guard-shaped symlink, directory, or partial file
    # must produce a protective record rather than disappearing from the set.
    paths = sorted(
        path
        for path in active_dir.iterdir()
        if path.name.startswith("guard-") and path.suffix.lower() == ".json"
    )
    return tuple(read_marker_record(path) for path in paths)


def has_active_guard_marker(active_dir: Path) -> bool:
    """Filesystem-only protective projection shared with disk reclamation."""
    try:
        return any(not record.terminal for record in read_marker_records(active_dir))
    except (OSError, ValueError):
        return True


@contextmanager
def _marker_lock(path: Path) -> Iterator[Path]:
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
        if (
            record.payload.get("child_launch_state") != "not_started"
            and payload["child_launch_state"] == "not_started"
        ):
            raise ActiveCustodyError("producer update reverses child launch boundary")
        atomic_write_bytes(canonical, encode_exact(payload))
    return True


def _process_evidence(
    role: str, identity: MarkerProcessIdentity, samples: Mapping[int, ProcessSampleView]
) -> ProcessCustodyEvidence:
    assert identity.started_at_ns is not None
    sample = samples.get(identity.pid)
    observed = None if sample is None else sample.started_at_ns
    state = (
        "absent"
        if sample is None
        else "identity_unavailable"
        if observed is None
        else "identity_match"
        if observed == identity.started_at_ns
        else "identity_mismatch"
    )
    return ProcessCustodyEvidence(
        role, identity.pid, identity.started_at_ns, observed, state
    )


def _classify(
    record: MarkerRecord, samples: Mapping[int, ProcessSampleView]
) -> ReconciliationDecision:
    def result(disposition: str, reason: str, evidence=()) -> ReconciliationDecision:
        return ReconciliationDecision(
            str(record.path), disposition, reason, record.status, evidence
        )

    if record.custody_error is not None:
        return result("preserve", record.custody_error)
    if record.terminal:
        return result("already_terminal", "terminal_status")
    assert record.guard is not None
    identities = [("guard", record.guard)]
    if record.child is not None:
        identities.append(("child", record.child))
    evidence: list[ProcessCustodyEvidence] = []
    for role, identity in identities:
        item = _process_evidence(role, identity, samples)
        evidence.append(item)
        if item.state in {"identity_match", "identity_unavailable"}:
            return result("preserve", f"{role}_process_{item.state}", tuple(evidence))
    if record.child is not None and record.child.pgid is not None:
        if any(sample.pgid == record.child.pgid for sample in samples.values()):
            return result(
                "preserve", "child_process_group_still_present", tuple(evidence)
            )
    return result("terminalize", "recorded_custody_absent_or_reused", tuple(evidence))


def _apply_decision(
    record: MarkerRecord,
    decision: ReconciliationDecision,
    *,
    timestamp: str,
    snapshot_processes: int,
) -> ReconciliationDecision:
    try:
        with _marker_lock(record.path) as canonical:
            current = read_marker_record(canonical)
            if current.identity != record.identity or current.error is not None:
                return replace(
                    decision,
                    disposition="preserve",
                    reason="marker_changed_during_reconciliation",
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
                },
            }
            _parse_payload(canonical, payload)
            atomic_write_bytes(canonical, encode_exact(payload))
    except (OSError, ValueError, RuntimeError) as exc:
        return replace(
            decision, disposition="preserve", reason=f"marker_publication_failed: {exc}"
        )
    return replace(decision, applied=True)


def reconcile_active_guard_markers(
    active_dir: Path,
    snapshot: Callable[[], Mapping[int, ProcessSampleView]],
    *,
    apply: bool = False,
) -> ReconciliationReport:
    """Read generations, take one snapshot, then optionally terminalize evidence.

    Capturing records before the snapshot prevents an older snapshot from
    classifying a newly launched guard as absent. Publication holds the same
    lock as the producer and compares the full physical/content generation.
    The injected snapshot must be the complete native process table, never a
    selected group, cache, failed query, or empty result.
    """
    records = read_marker_records(active_dir)
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
    timestamp = utc_timestamp()
    decisions = []
    for record in records:
        decision = _classify(record, samples)
        if apply and decision.disposition == "terminalize":
            decision = _apply_decision(
                record, decision, timestamp=timestamp, snapshot_processes=len(samples)
            )
        decisions.append(decision)
    return ReconciliationReport(
        str(active_dir.absolute()), len(samples), apply, tuple(decisions)
    )
