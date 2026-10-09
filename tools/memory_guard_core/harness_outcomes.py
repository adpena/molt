from __future__ import annotations

import datetime as dt
import json
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

from tools import memory_guard
from molt.file_publication import atomic_write_bytes


class HarnessLimitsView(Protocol):
    @property
    def max_process_rss_gb(self) -> float: ...

    @property
    def max_total_rss_gb(self) -> float: ...


def utc_timestamp() -> str:
    return (
        dt.datetime.now(dt.timezone.utc)
        .isoformat(timespec="seconds")
        .replace("+00:00", "Z")
    )


def elapsed_text(elapsed_s: float | None) -> str:
    return "unknown" if elapsed_s is None else f"{elapsed_s:.2f}s"


def limit_text(limit_gb: float | None) -> str:
    return "unknown" if limit_gb is None else f"{limit_gb:.2f}GB"


def rss_limit_hint(prefix: str) -> str:
    normalized = memory_guard.normalize_env_prefix(prefix) or "MOLT"
    if normalized == "MOLT":
        return "MOLT_MAX_PROCESS_RSS_GB/MOLT_MAX_TOTAL_RSS_GB"
    return (
        f"{normalized}_MAX_PROCESS_RSS_GB/{normalized}_MAX_TOTAL_RSS_GB "
        "or the parent MOLT_MAX_* RSS limits"
    )


def timeout_hint(prefix: str) -> str:
    normalized = memory_guard.normalize_env_prefix(prefix) or "MOLT"
    return f"{normalized}_TIMEOUT_SEC or MOLT_TEST_PROCESS_TIMEOUT_SEC"


def guard_stderr_message(
    violation: memory_guard.RssViolation | None,
    limits: HarnessLimitsView,
    effective_limits: memory_guard.ResolvedMemoryLimits | None = None,
    *,
    prefix: str,
    elapsed_s: float | None,
    killed_at: str,
) -> str:
    if violation is None:
        return ""
    limit_gb = (
        (
            effective_limits.max_total_rss_gb
            if effective_limits is not None
            else limits.max_total_rss_gb
        )
        if violation.scope == "process_tree"
        else (
            effective_limits.max_process_rss_gb
            if effective_limits is not None
            else limits.max_process_rss_gb
        )
    )
    cleanup = (
        "classified the command as failed from child exit resource usage"
        if violation.scope == "process_rusage"
        else "terminated the tracked process tree to prevent orphaned Molt subprocesses"
    )
    time_label = "observed_at" if violation.scope == "process_rusage" else "killed_at"
    return (
        "memory_guard: RSS limit exceeded; "
        f"{cleanup}: {time_label}={killed_at} elapsed={elapsed_text(elapsed_s)} "
        f"pid={violation.pid} rss={violation.rss_gb:.2f}GB "
        f"limit={limit_text(limit_gb)} scope={violation.scope} "
        f"command={violation.command}\n"
        "memory_guard: next action: inspect child logs and allocations for runaway "
        "work; lower parallelism/input size, or if this workload is expected raise "
        f"{rss_limit_hint(prefix)} within repo policy.\n"
    )


def guard_timeout_message(
    *,
    prefix: str,
    timeout: float | None,
    elapsed_s: float | None,
    killed_at: str,
) -> str:
    timeout_text = "unknown" if timeout is None else f"{timeout:.2f}s"
    return (
        "memory_guard: timeout; terminated the tracked process tree to prevent "
        "orphaned Molt subprocesses: "
        f"killed_at={killed_at} elapsed={elapsed_text(elapsed_s)} "
        f"timeout={timeout_text}\n"
        "memory_guard: next action: inspect child logs for a hang or oversized "
        f"workload; if intentional raise {timeout_hint(prefix)} for this guard "
        "family.\n"
    )


def guard_exit_signal_message(
    returncode: int,
    *,
    elapsed_s: float | None,
    observed_at: str,
) -> str:
    payload = memory_guard.exit_signal_payload(returncode)
    if payload is None:
        return ""
    signame = payload["name"] or f"signal {payload['signal']}"
    return (
        "memory_guard: command exited with "
        f"{signame} status ({returncode}); no RSS violation observed: "
        f"observed_at={observed_at} elapsed={elapsed_text(elapsed_s)}\n"
        "memory_guard: next action: inspect child stderr/logs or host signal "
        "source, including the direct-child RLIMIT_RSS backstop; if "
        "host memory pressure was involved, rerun with guard samples and lower "
        "parallelism.\n"
    )


def guard_parent_signal_message(
    guard_signal: int,
    *,
    elapsed_s: float | None,
    observed_at: str,
    primary_reason: str | None = None,
) -> str:
    payload = memory_guard.exit_signal_payload(128 + guard_signal)
    signame = (
        payload["name"]
        if payload is not None and payload["name"] is not None
        else f"signal {guard_signal}"
    )
    if primary_reason is None:
        return (
            "memory_guard: guard parent received "
            f"{signame}; terminated tracked process tree before exiting: "
            f"observed_at={observed_at} elapsed={elapsed_text(elapsed_s)}\n"
            "memory_guard: next action: inspect the parent host/control-plane "
            "signal source and child logs; the guard parent received the signal "
            "and wrote this custody record before exiting.\n"
        )
    return (
        "memory_guard: guard parent also received "
        f"{signame} while primary incident remained {primary_reason}: "
        f"observed_at={observed_at} elapsed={elapsed_text(elapsed_s)}\n"
        "memory_guard: next action: inspect the parent host/control-plane "
        "signal source and child logs; preserve the primary incident "
        "classification when triaging this run.\n"
    )


def guard_orphan_cleanup_message(
    process_groups: Sequence[int],
    *,
    elapsed_s: float | None,
    killed_at: str,
) -> str:
    if not process_groups:
        return ""
    pgids = ",".join(str(pgid) for pgid in process_groups)
    return (
        "memory_guard: orphaned child processes detected after command exit; "
        "terminated tracked process groups to prevent accumulation: "
        f"killed_at={killed_at} elapsed={elapsed_text(elapsed_s)} "
        f"pgids={pgids} reason=direct child exited while descendants were still "
        "live\n"
        "memory_guard: next action: inspect child process lifecycle and logs; "
        "make helpers shut down explicitly, or run intentional warm daemons inside "
        "a suite-level sentinel that drains at scope exit.\n"
    )


def rss_record_payload(
    record: memory_guard.RssViolation | None,
) -> dict[str, object] | None:
    if record is None:
        return None
    return {
        "pid": record.pid,
        "rss_kb": record.rss_kb,
        "rss_gb": record.rss_gb,
        "command": record.command,
        "scope": record.scope,
    }


def guarded_command_status(
    *,
    returncode: int,
    violation: memory_guard.RssViolation | None,
    timed_out: bool,
    orphaned_process_groups: Sequence[int],
    guard_signal: int | None = None,
    infrastructure_failure: memory_guard.GuardInfrastructureFailure | None = None,
    cancelled: bool = False,
) -> str:
    if violation is not None:
        return "rss_limit_exceeded"
    if timed_out:
        return "timeout"
    if guard_signal is not None:
        return "guard_interrupted"
    if infrastructure_failure is not None:
        return "infrastructure_error"
    if cancelled:
        return "cancelled"
    if memory_guard.exit_signal_payload(returncode) is not None:
        return "signal_exit"
    if returncode != 0:
        return "failed"
    if orphaned_process_groups:
        return "pass_with_orphan_cleanup"
    return "pass"


SUITE_TRIP_FILE_ENV = "MOLT_DIFF_MEMORY_GUARD_TRIP_FILE"


@dataclass(frozen=True, slots=True)
class SuiteRssTrip:
    message: str
    victim_pgid: int
    process_identities: tuple[tuple[int, int], ...]
    termination_attempted: bool
    unidentified_samples: int = 0
    observed_at_ns: int | None = None
    # (victim PID, birth, live-admission time, frozen ancestor identities)
    custody_ancestry: tuple[tuple[int, int, int, tuple[tuple[int, int], ...]], ...] = ()

    @property
    def details(self) -> str:
        if not self.unidentified_samples:
            return self.message
        return (
            f"{self.message}\nmemory_guard: omitted {self.unidentified_samples} "
            "unidentified victim samples; attribution uses captured births only"
        )

    def matches(
        self,
        child: memory_guard.GuardedChildProcess | None,
        owned: Sequence[tuple[int, memory_guard.ProcessIdentity]],
        *,
        request_started_at_ns: int | None = None,
    ) -> bool:
        if child is None or not self.termination_attempted:
            return False
        if request_started_at_ns is not None and (
            self.observed_at_ns is None or self.observed_at_ns < request_started_at_ns
        ):
            return False
        direct = (
            (type(child.started_at_ns) is int and child.started_at_ns > 0)
            and (child.pgid if child.pgid is not None else child.pid)
            == self.victim_pgid
            and (
                child.pid,
                child.started_at_ns,
            )
            in self.process_identities
        )
        # The guard's existing tree/job tracker admitted these exact births
        # while live. Windows descendants can occupy separate process groups.
        # No ancestry reconstruction or current PID lookup happens after exit.
        descendant = any(
            pid != child.pid
            and type(identity.started_at_ns) is int
            and (pid, identity.started_at_ns) in self.process_identities
            for pid, identity in owned
        )
        request_descendant = request_started_at_ns is not None and any(
            admitted >= request_started_at_ns
            and (child.pid, child.started_at_ns) in ancestors
            for _pid, _born, admitted, ancestors in self.custody_ancestry
        )
        return direct or descendant or request_descendant


@dataclass(frozen=True, slots=True)
class SuiteTripEvidence:
    trips: tuple[SuiteRssTrip, ...] = ()
    infrastructure_failure: memory_guard.GuardInfrastructureFailure | None = None

    @property
    def message(self) -> str:
        if self.infrastructure_failure is not None:
            return "\n".join(self.infrastructure_failure.details)
        return "\n".join(dict.fromkeys(trip.details for trip in self.trips))


def suite_trip_failure(message: str) -> SuiteTripEvidence:
    return SuiteTripEvidence(
        infrastructure_failure=memory_guard.GuardInfrastructureFailure(
            phase="rss_trip_evidence", details=(message,)
        )
    )


def _suite_rss_trip(payload: object) -> SuiteRssTrip:
    if not isinstance(payload, dict) or payload.get("event") != "guard_tripped":
        raise ValueError("trip entry is not a guard_tripped object")
    violation = payload.get("violation")
    if (
        not isinstance(violation, dict)
        or type(violation.get("rss_kb")) is not int
        or violation["rss_kb"] <= 0
        or not isinstance(violation.get("scope"), str)
        or violation["scope"]
        not in {"process", "process_tree", "diff_global_process_groups"}
    ):
        raise ValueError("trip entry has no measured RSS violation")
    event = payload.get("shared_sentinel_event")
    if (
        not isinstance(event, dict)
        or event.get("event") != "repo_process_guard_tripped"
    ):
        raise ValueError("trip entry has no sentinel victim custody")
    pgid = event.get("victim_pgid")
    measured = event.get("violation")
    termination = event.get("termination")
    if (
        type(pgid) is not int
        or pgid <= 0
        or not isinstance(measured, dict)
        or type(measured.get("pgid")) is not int
        or measured["pgid"] != pgid
        or not isinstance(termination, dict)
        or termination.get("rss_triggered") is not True
        or type(termination.get("attempted")) is not bool
    ):
        raise ValueError("trip entry has invalid victim group or termination custody")
    samples = measured.get("process_samples")
    if not isinstance(samples, list) or not samples:
        raise ValueError("trip entry has no victim process identities")
    identities = []
    unidentified = 0
    for sample in samples:
        if not isinstance(sample, dict):
            unidentified += 1
            continue
        pid, born = sample.get("pid"), sample.get("started_at_ns")
        if type(pid) is not int or pid <= 0 or type(born) is not int or born <= 0:
            unidentified += 1
            continue
        identities.append((pid, born))
    if not identities:
        raise ValueError(
            f"trip has no identifiable victim births; omitted {unidentified} samples"
        )
    observed = event.get("observed_at_ns")
    if observed is not None and (type(observed) is not int or observed <= 0):
        raise ValueError("trip observation clock is invalid")
    ancestry = []
    raw_ancestry = event.get("custody_ancestry", [])
    if not isinstance(raw_ancestry, list):
        raise ValueError("trip custody ancestry must be a list")
    for record in raw_ancestry:
        if not isinstance(record, dict):
            raise ValueError("trip custody ancestry record must be an object")
        victim = (record.get("pid"), record.get("started_at_ns"))
        admitted = record.get("admitted_at_ns")
        ancestors = record.get("ancestors")
        if (
            any(type(value) is not int or value <= 0 for value in victim)
            or victim not in identities
            or observed is None
            or type(admitted) is not int
            or not 0 < admitted <= observed
            or not isinstance(ancestors, list)
            or not ancestors
        ):
            raise ValueError("trip custody ancestry is not bound to a victim birth")
        chain = []
        for ancestor in ancestors:
            if not isinstance(ancestor, dict) or any(
                type(ancestor.get(key)) is not int or ancestor[key] <= 0
                for key in ("pid", "started_at_ns")
            ):
                raise ValueError("trip custody ancestor lacks a creation identity")
            identity = (ancestor["pid"], ancestor["started_at_ns"])
            if identity == victim or identity in chain:
                raise ValueError("trip custody ancestry contains a cycle")
            chain.append(identity)
        ancestry.append((*victim, admitted, tuple(chain)))
    message = payload.get("message")
    if not isinstance(message, str) or not message:
        message = "memory_guard: RSS limit exceeded under suite sentinel"
    return SuiteRssTrip(
        message,
        pgid,
        tuple(sorted(set(identities))),
        termination["attempted"],
        unidentified,
        observed,
        tuple(ancestry),
    )


def _suite_trip_entries(payload: object) -> list[dict[str, object]]:
    if not isinstance(payload, dict) or payload.get("event") != "guard_tripped":
        raise ValueError("suite trip record must be a guard_tripped object")
    entries = payload.get("trips")
    if not isinstance(entries, list) or not entries:
        raise ValueError("suite trip record has no victim entries")
    for entry in entries:
        _suite_rss_trip(entry)
    return entries


def read_suite_trip(
    environ: Mapping[str, str], *, path: Path | None = None
) -> SuiteTripEvidence | None:
    if path is None:
        raw = environ.get(SUITE_TRIP_FILE_ENV, "").strip()
        if not raw:
            return None
        path = Path(raw).expanduser()
    try:
        entries = _suite_trip_entries(json.loads(path.read_text(encoding="utf-8")))
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as exc:
        return suite_trip_failure(
            f"memory_guard: invalid suite trip evidence at {path}: {exc}"
        )
    return SuiteTripEvidence(tuple(_suite_rss_trip(entry) for entry in entries))


def publish_suite_trip(path: Path, entry: dict[str, object]) -> None:
    """Atomically extend the sole suite marker; the suite sentinel is its writer.

    Failed serialization/publication propagates to the sentinel's retained typed
    outcome. Preserve earlier victims so later terminations cannot erase custody.
    """
    incoming = _suite_rss_trip(entry)
    try:
        entries = _suite_trip_entries(json.loads(path.read_text(encoding="utf-8")))
    except FileNotFoundError:
        entries = []
    for index, previous in enumerate(entries):
        recorded = _suite_rss_trip(previous)
        if (recorded.victim_pgid, recorded.process_identities) == (
            incoming.victim_pgid,
            incoming.process_identities,
        ):
            if not incoming.termination_attempted or recorded.termination_attempted:
                return
            entries[index] = entry
            break
    else:
        entries.append(entry)
    atomic_write_bytes(
        path,
        (
            json.dumps(
                {"event": "guard_tripped", "trips": entries}, indent=2, sort_keys=True
            )
            + "\n"
        ).encode("utf-8"),
    )
