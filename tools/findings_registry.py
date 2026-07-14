#!/usr/bin/env python3
"""APPARATUS A4 — the molt findings registry: quantitative findings become
queryable, recalibratable records with empirical anchors and an ORPHAN BAN.

Why this exists (operator intent, near-verbatim): the apparatus "helps us
actually remember lessons ... so the operator doesn't have to remind you." A
measured lesson recorded as prose in a memo silently rots — the next session
re-reads a stale line and pays for it (MEMORY.md: "a stale line once cost a
30-min detour"). This registry gives a quantitative finding TEETH: a typed,
append-only, machine-queryable record with predicted-vs-measured anchors, an
authority tier per M28, a domain of validity from M02's verified subset, and a
producer/consumer graph. It is the molt-shaped port of pact's canonical-equations
registry (``src/tac/canonical_equations/equation.py`` + ``registry.py``,
studied in ``docs/agent/APPARATUS_FROM_COMMA_LAB.md`` §1.3 / A4).

THE ORPHAN BAN (the load-bearing invariant)
-------------------------------------------
A finding with NO producers AND NO consumers is REFUSED at construction. A fact
nobody produces and nobody reads is exactly the "tribal knowledge with no
machine-readable consumer" failure mode — it is theater. The construction
surface refuses it at the source, so the registry cannot fill up with orphaned
prose-in-JSON.

Schema (one record; molt-shaped, lifted from CanonicalEquation/EmpiricalAnchor):

  * ``finding_id``            — ``snake_case_vN`` (e.g. ``probe_int_checkedmul_peel_v1``)
  * ``one_line_summary``      — operator-facing, <= 200 chars
  * ``claim``                 — machine-checkable form where possible (non-empty)
  * ``domain_of_validity``    — targets / OS / py-version mapping (M02 verified subset)
  * ``anchors``               — tuple[EmpiricalAnchor]: predicted vs measured,
                                residual, authority_tier (M28), optional noise_floor
  * ``producers`` / ``consumers`` — dotted module-path / tool / gate lists
                                (ORPHAN BAN: at least one of the two must be non-empty)
  * ``verification``          — 4-value taxonomy (see below)
  * ``next_recalibration_trigger`` — when this record must be re-measured

Storage: ``.molt/state/findings_registry.jsonl`` — append-only JSONL, one event
per row referencing the same ``finding_id``; the LATEST event per id is the
current state (latest-event-per-id wins). Writes serialize through an
``msvcrt``-locked (Windows) / ``fcntl``-locked (POSIX) lock file, atomic
tmp+replace. Bare edits are fine to read; mutations go through the helpers here.

CLI::

    python tools/findings_registry.py list
    python tools/findings_registry.py get probe_int_checkedmul_peel_v1
    python tools/findings_registry.py query --consumer tools/bench_evidence.py
    python tools/findings_registry.py query --producer 261efc7b2
    python tools/findings_registry.py query --domain windows
    python tools/findings_registry.py register --from-json record.json
    python tools/findings_registry.py seed        # write the seeded keystones
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import os
import re
import socket
import sys
import time
import uuid
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

_THIS_FILE = Path(__file__).resolve()
REPO_ROOT = _THIS_FILE.parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools._io_utf8 import force_utf8_stdio  # noqa: E402

# This module relays record summaries (which can carry em-dashes / non-cp1252
# glyphs from one_line_summary text) through print(); backstop stdio once so a
# stray byte degrades to \xNN instead of crashing the CLI (M43).
force_utf8_stdio()

SCHEMA_VERSION = "findings_registry_v1_20260710"
"""Pinned schema version. Bump only via an explicit migration landing."""

FINDINGS_REGISTRY_PATH = REPO_ROOT / ".molt" / "state" / "findings_registry.jsonl"
FINDINGS_REGISTRY_LOCK = FINDINGS_REGISTRY_PATH.with_suffix(
    FINDINGS_REGISTRY_PATH.suffix + ".lock"
)

# Lock acquisition timeout (seconds).
LOCK_TIMEOUT_SECONDS = 30

# ---------------------------------------------------------------------------
# Canonical enums
# ---------------------------------------------------------------------------

# ``snake_case`` + trailing ``_vN`` version slug.
_FINDING_ID_RE = re.compile(r"^[a-z][a-z0-9_]*_v\d+$")

# Authority tier per M28 — how the anchor's ``measured`` value was obtained. A
# perf number MUST come from a release build; a parity result from the serial
# differential; a bench delta carries its noise floor; a source-level fact is a
# code/commit inspection. This is what stops a dev-profile or single-run number
# masquerading as a measured authority.
AUTHORITY_MOLT_BUILD_RELEASE = "molt_build_release"  # perf: molt build --release
AUTHORITY_MOLT_DIFF_SERIAL = "molt_diff_serial"  # parity: molt_diff.py --jobs 1
AUTHORITY_BENCH_NOISE_FLOOR = "bench_noise_floor"  # bench number w/ noise floor
AUTHORITY_BUILD_WALL_CLOCK = "build_wall_clock"  # build-time wall-clock (M09)
AUTHORITY_SOURCE_INSPECTION = "source_inspection"  # commit / source-level fact

VALID_AUTHORITY_TIERS = frozenset(
    {
        AUTHORITY_MOLT_BUILD_RELEASE,
        AUTHORITY_MOLT_DIFF_SERIAL,
        AUTHORITY_BENCH_NOISE_FLOOR,
        AUTHORITY_BUILD_WALL_CLOCK,
        AUTHORITY_SOURCE_INSPECTION,
    }
)

# 4-value verification taxonomy (pact Catalog #363), distinguishing a value that
# was verified from one merely inferred or awaiting verification.
VERIFIED_VIA_SOURCE_INSPECTION = "VERIFIED_VIA_SOURCE_INSPECTION"
VERIFIED_VIA_EMPIRICAL_ANCHOR = "VERIFIED_VIA_EMPIRICAL_ANCHOR"
INFERRED_FROM_DOMAIN_LITERATURE = "INFERRED_FROM_DOMAIN_LITERATURE"
ASSUMED_AWAITING_VERIFICATION = "ASSUMED_AWAITING_VERIFICATION"

VALID_VERIFICATIONS = frozenset(
    {
        VERIFIED_VIA_SOURCE_INSPECTION,
        VERIFIED_VIA_EMPIRICAL_ANCHOR,
        INFERRED_FROM_DOMAIN_LITERATURE,
        ASSUMED_AWAITING_VERIFICATION,
    }
)

# Recalibration-trigger taxonomy — when this finding must be re-measured.
RECALIBRATE_ON_NEW_ANCHORS = "when_3+_new_empirical_anchors_in_domain"
RECALIBRATE_ON_RESIDUAL_DRIFT = "when_residual_drift_exceeds_2x"
RECALIBRATE_ON_OPERATOR = "when_operator_invokes_recalibrate_finding"
RECALIBRATE_NEVER_AUTO = "never_auto_operator_only"

VALID_RECALIBRATION_TRIGGERS = frozenset(
    {
        RECALIBRATE_ON_NEW_ANCHORS,
        RECALIBRATE_ON_RESIDUAL_DRIFT,
        RECALIBRATE_ON_OPERATOR,
        RECALIBRATE_NEVER_AUTO,
    }
)

# Event taxonomy for the append-only ledger.
EVENT_REGISTERED = "registered"
EVENT_ANCHOR_APPENDED = "anchor_appended"
EVENT_RECALIBRATED = "recalibrated"
EVENT_DEPRECATED = "deprecated"

VALID_EVENT_TYPES = frozenset(
    {
        EVENT_REGISTERED,
        EVENT_ANCHOR_APPENDED,
        EVENT_RECALIBRATED,
        EVENT_DEPRECATED,
    }
)

def advisory_leg_suggestion(text: str) -> str | None:
    """Suggest a triality leg for lint output; never writes registry state."""
    try:
        from tools.advisory_classifier import finding_tag_suggestion
    except Exception:
        return None
    return finding_tag_suggestion(text)


class InvalidFindingError(ValueError):
    """Raised when a Finding or EmpiricalAnchor violates a construction invariant.

    Every field-level contract is enforced in ``__post_init__`` (not
    docstring-only) so the construction surface refuses bad inputs at the source
    — the ORPHAN BAN is one of these.
    """


class FindingsRegistryCorruptError(RuntimeError):
    """Raised when the JSONL ledger is corrupt (strict, mutating callers)."""


def _utc_now_iso() -> str:
    return _dt.datetime.now(_dt.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def _require_iso_utc(value: str, field_name: str) -> None:
    if not isinstance(value, str) or not value:
        raise InvalidFindingError(f"{field_name} must be a non-empty ISO-UTC string")
    try:
        if value.endswith("Z"):
            _dt.datetime.fromisoformat(value[:-1] + "+00:00")
        else:
            _dt.datetime.fromisoformat(value)
    except ValueError as exc:
        raise InvalidFindingError(
            f"{field_name}={value!r} is not valid ISO-8601 UTC: {exc}"
        ) from exc


# ---------------------------------------------------------------------------
# EmpiricalAnchor
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class EmpiricalAnchor:
    """One predicted-vs-measured measurement attached to a Finding.

    ``authority_tier`` (M28) records HOW ``measured`` was obtained so a
    dev-profile or single-run number cannot pose as a release-build or
    serial-differential authority. ``noise_floor`` (optional) is the composed
    across-run noise magnitude for this anchor's delta; a residual at/below the
    floor is INSTANCE-level, not a verdict (see ``delta_exceeds_floor``).
    """

    anchor_id: str
    predicted: Any
    measured: Any
    residual: float
    authority_tier: str
    measurement_method: str
    source_artifact: str
    measured_utc: str
    noise_floor: float | None = None
    noise_floor_provenance: str | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.anchor_id, str) or not self.anchor_id.strip():
            raise InvalidFindingError("anchor_id must be a non-empty string")
        if any(c in self.anchor_id for c in ("\n", "\t", "\x1f")):
            raise InvalidFindingError("anchor_id must not contain newlines/tabs/0x1f")
        if not isinstance(self.residual, (int, float)) or isinstance(
            self.residual, bool
        ):
            raise InvalidFindingError("residual must be numeric")
        if self.residual != self.residual:  # NaN
            raise InvalidFindingError("residual must not be NaN")
        if self.residual < 0:
            raise InvalidFindingError("residual must be >= 0 (normalized magnitude)")
        if self.authority_tier not in VALID_AUTHORITY_TIERS:
            raise InvalidFindingError(
                f"authority_tier={self.authority_tier!r} must be one of "
                f"{sorted(VALID_AUTHORITY_TIERS)!r} (M28: molt build --release for "
                "perf, molt_diff.py --jobs 1 for parity, a noise floor for benches)"
            )
        if not isinstance(self.measurement_method, str) or not (
            self.measurement_method.strip()
        ):
            raise InvalidFindingError("measurement_method must be a non-empty string")
        if (
            not isinstance(self.source_artifact, str)
            or not self.source_artifact.strip()
        ):
            raise InvalidFindingError(
                "source_artifact must be a non-empty string (commit sha / file:line "
                "/ artifact path — the anchor must cite where it was measured)"
            )
        _require_iso_utc(self.measured_utc, "measured_utc")
        if self.noise_floor is not None:
            if not isinstance(self.noise_floor, (int, float)) or isinstance(
                self.noise_floor, bool
            ):
                raise InvalidFindingError("noise_floor must be a number or None")
            if self.noise_floor != self.noise_floor:  # NaN
                raise InvalidFindingError("noise_floor must not be NaN")
            if self.noise_floor < 0:
                raise InvalidFindingError("noise_floor must be >= 0")
            if not (
                isinstance(self.noise_floor_provenance, str)
                and self.noise_floor_provenance.strip()
            ):
                raise InvalidFindingError(
                    "noise_floor requires a non-empty noise_floor_provenance "
                    "(NO-FAKE: every floor cites how it was measured/bounded)"
                )

    def to_dict(self) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "anchor_id": self.anchor_id,
            "predicted": self.predicted,
            "measured": self.measured,
            "residual": float(self.residual),
            "authority_tier": self.authority_tier,
            "measurement_method": self.measurement_method,
            "source_artifact": self.source_artifact,
            "measured_utc": self.measured_utc,
        }
        if self.noise_floor is not None:
            payload["noise_floor"] = float(self.noise_floor)
            payload["noise_floor_provenance"] = self.noise_floor_provenance
        return payload

    @staticmethod
    def from_dict(d: Mapping[str, Any]) -> "EmpiricalAnchor":
        return EmpiricalAnchor(
            anchor_id=d["anchor_id"],
            predicted=d.get("predicted"),
            measured=d.get("measured"),
            residual=float(d["residual"]),
            authority_tier=d["authority_tier"],
            measurement_method=d["measurement_method"],
            source_artifact=d["source_artifact"],
            measured_utc=d["measured_utc"],
            noise_floor=d.get("noise_floor"),
            noise_floor_provenance=d.get("noise_floor_provenance"),
        )


def delta_exceeds_floor(
    anchor: EmpiricalAnchor, delta: float | None = None
) -> bool | None:
    """Is this anchor's delta ABOVE its composed noise floor?

    ``None`` when the anchor has no ``noise_floor`` (UNMEASURED — the comparison
    cannot be cleared as a verdict), ``True`` when ``|delta| > noise_floor``
    (distinguishable from noise), ``False`` when within the floor (INSTANCE-level,
    not a verdict). ``delta`` defaults to the anchor's own ``residual``. NO-FAKE:
    a floor of ``None`` stays ``None`` here — never silently treated as 0.
    """
    if anchor.noise_floor is None:
        return None
    d = abs(float(anchor.residual if delta is None else delta))
    return d > float(anchor.noise_floor)


# ---------------------------------------------------------------------------
# Finding
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Finding:
    """One quantitative finding + its calibration anchors and provenance graph.

    THE ORPHAN BAN: a finding with empty ``producers`` AND empty ``consumers`` is
    refused at construction — a fact nobody produces and nobody reads is tribal
    knowledge with no machine-readable consumer, which is precisely the class this
    registry exists to extinct.
    """

    finding_id: str
    one_line_summary: str
    claim: str
    domain_of_validity: Mapping[str, Any]
    anchors: tuple[EmpiricalAnchor, ...]
    producers: tuple[str, ...]
    consumers: tuple[str, ...]
    verification: str
    next_recalibration_trigger: str
    created_utc: str
    last_calibration_utc: str
    schema_version: str = SCHEMA_VERSION

    def __post_init__(self) -> None:
        if not isinstance(self.finding_id, str):
            raise InvalidFindingError("finding_id must be a string")
        if not _FINDING_ID_RE.match(self.finding_id):
            raise InvalidFindingError(
                f"finding_id={self.finding_id!r} must match snake_case_vN "
                "(e.g. 'probe_int_checkedmul_peel_v1')"
            )
        if not isinstance(self.one_line_summary, str) or not (
            self.one_line_summary.strip()
        ):
            raise InvalidFindingError("one_line_summary must be a non-empty string")
        if len(self.one_line_summary) > 200:
            raise InvalidFindingError(
                f"one_line_summary length={len(self.one_line_summary)} exceeds 200; "
                "move detail to the finding memo"
            )
        if not isinstance(self.claim, str) or not self.claim.strip():
            raise InvalidFindingError(
                "claim must be a non-empty string (machine-checkable form where "
                "possible — e.g. 'probe_int.py wall_release <= 0.62 * wall_cpython')"
            )
        if not isinstance(self.domain_of_validity, Mapping):
            raise InvalidFindingError("domain_of_validity must be a mapping")
        if not self.domain_of_validity:
            raise InvalidFindingError(
                "domain_of_validity must be non-empty (M02 verified subset: name the "
                "targets / OS / py-version this finding holds on)"
            )
        if not isinstance(self.anchors, tuple):
            raise InvalidFindingError("anchors must be a tuple (frozen)")
        for i, anchor in enumerate(self.anchors):
            if not isinstance(anchor, EmpiricalAnchor):
                raise InvalidFindingError(
                    f"anchors[{i}] must be EmpiricalAnchor, got {type(anchor).__name__}"
                )
        if not isinstance(self.producers, tuple):
            raise InvalidFindingError("producers must be a tuple")
        if not isinstance(self.consumers, tuple):
            raise InvalidFindingError("consumers must be a tuple")
        for label, seq in (
            ("producers", self.producers),
            ("consumers", self.consumers),
        ):
            for item in seq:
                if not isinstance(item, str) or not item.strip():
                    raise InvalidFindingError(
                        f"{label} entries must be non-empty strings"
                    )
        # THE ORPHAN BAN.
        if not self.producers and not self.consumers:
            raise InvalidFindingError(
                f"finding_id={self.finding_id!r} has empty producers AND consumers — "
                "orphan findings are FORBIDDEN (structural extinction of tribal "
                "knowledge with no machine-readable consumer). Declare at least one "
                "producer (a tool/commit/gate that emits this finding's anchors) OR "
                "consumer (a tool/gate/doc that reads this finding)."
            )
        if self.verification not in VALID_VERIFICATIONS:
            raise InvalidFindingError(
                f"verification={self.verification!r} must be one of "
                f"{sorted(VALID_VERIFICATIONS)!r}"
            )
        # An empirical-anchor verification claim needs at least one anchor to back
        # it (an anchor-grade verdict with no anchor is theater).
        if self.verification == VERIFIED_VIA_EMPIRICAL_ANCHOR and not self.anchors:
            raise InvalidFindingError(
                f"finding_id={self.finding_id!r} claims "
                "VERIFIED_VIA_EMPIRICAL_ANCHOR but carries no anchors"
            )
        if self.next_recalibration_trigger not in VALID_RECALIBRATION_TRIGGERS:
            raise InvalidFindingError(
                f"next_recalibration_trigger={self.next_recalibration_trigger!r} "
                f"must be one of {sorted(VALID_RECALIBRATION_TRIGGERS)!r}"
            )
        _require_iso_utc(self.created_utc, "created_utc")
        _require_iso_utc(self.last_calibration_utc, "last_calibration_utc")
        if self.schema_version != SCHEMA_VERSION:
            raise InvalidFindingError(
                f"schema_version={self.schema_version!r} != {SCHEMA_VERSION!r}"
            )

    @property
    def is_well_calibrated(self) -> bool:
        """True iff there is at least one anchor and every residual is < 2.0.

        2.0 = "predicted within 2x of measured" (readable universal threshold).
        No anchors yet => not falsified, not confirmed => reported as not
        well-calibrated (a visible cue to land the first anchor).
        """
        if not self.anchors:
            return False
        return all(a.residual < 2.0 for a in self.anchors)

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "finding_id": self.finding_id,
            "one_line_summary": self.one_line_summary,
            "claim": self.claim,
            "domain_of_validity": dict(self.domain_of_validity),
            "anchors": [a.to_dict() for a in self.anchors],
            "producers": list(self.producers),
            "consumers": list(self.consumers),
            "verification": self.verification,
            "next_recalibration_trigger": self.next_recalibration_trigger,
            "created_utc": self.created_utc,
            "last_calibration_utc": self.last_calibration_utc,
        }

    @staticmethod
    def from_dict(payload: Mapping[str, Any]) -> "Finding":
        anchors = tuple(
            EmpiricalAnchor.from_dict(a) for a in payload.get("anchors", [])
        )
        return Finding(
            finding_id=payload["finding_id"],
            one_line_summary=payload["one_line_summary"],
            claim=payload["claim"],
            domain_of_validity=payload["domain_of_validity"],
            anchors=anchors,
            producers=tuple(payload.get("producers", [])),
            consumers=tuple(payload.get("consumers", [])),
            verification=payload["verification"],
            next_recalibration_trigger=payload["next_recalibration_trigger"],
            created_utc=payload["created_utc"],
            last_calibration_utc=payload["last_calibration_utc"],
            schema_version=payload.get("schema_version", SCHEMA_VERSION),
        )

    def with_new_anchor(self, anchor: EmpiricalAnchor) -> "Finding":
        """Return a new Finding with the anchor appended (frozen-safe)."""
        if not isinstance(anchor, EmpiricalAnchor):
            raise InvalidFindingError(
                f"with_new_anchor expected EmpiricalAnchor, got {type(anchor).__name__}"
            )
        from dataclasses import replace

        return replace(
            self,
            anchors=(*self.anchors, anchor),
            last_calibration_utc=_utc_now_iso(),
        )


# ---------------------------------------------------------------------------
# Cross-platform lock (msvcrt on Windows, fcntl on POSIX)
# ---------------------------------------------------------------------------

import contextlib  # noqa: E402
import threading  # noqa: E402

_lock_depth_tls = threading.local()


def _get_lock_depth() -> int:
    return int(getattr(_lock_depth_tls, "depth", 0))


def _set_lock_depth(value: int) -> None:
    _lock_depth_tls.depth = int(value)


def _lock_held() -> bool:
    return _get_lock_depth() > 0


def _acquire_os_lock(fd: int) -> None:
    """Block until an exclusive lock on ``fd`` is held, or raise TimeoutError.

    Windows uses ``msvcrt.locking`` (byte-range lock at offset 0); POSIX uses
    ``fcntl.flock``. Both retry non-blocking until ``LOCK_TIMEOUT_SECONDS``.
    """
    deadline = time.monotonic() + LOCK_TIMEOUT_SECONDS
    if os.name == "nt":
        import msvcrt

        while True:
            try:
                os.lseek(fd, 0, os.SEEK_SET)
                msvcrt.locking(fd, msvcrt.LK_NBLCK, 1)
                return
            except OSError:
                if time.monotonic() >= deadline:
                    raise TimeoutError(
                        f"could not acquire findings-registry lock within "
                        f"{LOCK_TIMEOUT_SECONDS}s"
                    ) from None
                time.sleep(0.05)
    else:
        import fcntl

        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise TimeoutError(
                        f"could not acquire findings-registry lock within "
                        f"{LOCK_TIMEOUT_SECONDS}s"
                    ) from None
                time.sleep(0.05)


def _release_os_lock(fd: int) -> None:
    if os.name == "nt":
        import msvcrt

        with contextlib.suppress(OSError):
            os.lseek(fd, 0, os.SEEK_SET)
            msvcrt.locking(fd, msvcrt.LK_UNLCK, 1)
    else:
        import fcntl

        with contextlib.suppress(OSError):
            fcntl.flock(fd, fcntl.LOCK_UN)


@contextlib.contextmanager
def registry_lock(lock_path: Path | None = None):
    """Acquire the exclusive registry lock; re-entrant within a thread."""
    p = lock_path or FINDINGS_REGISTRY_LOCK
    p.parent.mkdir(parents=True, exist_ok=True)
    depth = _get_lock_depth()
    if depth > 0:
        _set_lock_depth(depth + 1)
        try:
            yield None
        finally:
            _set_lock_depth(_get_lock_depth() - 1)
        return
    # msvcrt.locking requires a byte to lock: ensure the lock file is non-empty.
    fd = os.open(str(p), os.O_RDWR | os.O_CREAT, 0o644)
    try:
        if os.fstat(fd).st_size == 0:
            os.write(fd, b"\0")
        _acquire_os_lock(fd)
        _set_lock_depth(_get_lock_depth() + 1)
        try:
            yield fd
        finally:
            _set_lock_depth(_get_lock_depth() - 1)
            _release_os_lock(fd)
    finally:
        os.close(fd)


# ---------------------------------------------------------------------------
# Ledger I/O
# ---------------------------------------------------------------------------


def load_events_lenient(path: Path | None = None) -> list[dict[str, Any]]:
    """Read all events; skip malformed lines silently (read-only callers)."""
    p = path or FINDINGS_REGISTRY_PATH
    if not p.exists():
        return []
    rows: list[dict[str, Any]] = []
    try:
        text = p.read_text(encoding="utf-8")
    except OSError:
        return []
    for line in text.splitlines():
        s = line.strip()
        if not s:
            continue
        try:
            r = json.loads(s)
        except json.JSONDecodeError:
            continue
        if isinstance(r, dict):
            rows.append(r)
    return rows


def load_events_strict(path: Path | None = None) -> list[dict[str, Any]]:
    """Strict load for mutating callers; raises on corrupt state."""
    p = path or FINDINGS_REGISTRY_PATH
    if not p.exists():
        return []
    rows: list[dict[str, Any]] = []
    try:
        text = p.read_text(encoding="utf-8")
    except OSError as exc:
        raise FindingsRegistryCorruptError(f"{p} could not be read: {exc}") from exc
    for lineno, line in enumerate(text.splitlines(), start=1):
        s = line.strip()
        if not s:
            continue
        try:
            r = json.loads(s)
        except json.JSONDecodeError as exc:
            raise FindingsRegistryCorruptError(
                f"{p} line {lineno}: invalid JSON: {exc}"
            ) from exc
        if not isinstance(r, dict):
            raise FindingsRegistryCorruptError(
                f"{p} line {lineno}: non-dict root (type={type(r).__name__})"
            )
        rows.append(r)
    return rows


def _save_ledger(rows: list[dict[str, Any]], path: Path | None = None) -> None:
    """Atomic write under lock — tmp + fsync + os.replace."""
    if not _lock_held():
        raise RuntimeError(
            "_save_ledger called WITHOUT holding registry_lock; state writers own "
            "their lock end-to-end."
        )
    p = path or FINDINGS_REGISTRY_PATH
    p.parent.mkdir(parents=True, exist_ok=True)
    payload = "".join(json.dumps(r, sort_keys=True) + "\n" for r in rows)
    tmp = p.with_suffix(p.suffix + f".tmp.{uuid.uuid4().hex[:12]}")
    try:
        # Write + fsync through ONE write handle: on Windows os.fsync on a handle
        # reopened read-only fails with EBADF, so durability must ride the writer.
        with open(tmp, "w", encoding="utf-8", newline="\n") as f:
            f.write(payload)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, p)
    finally:
        if tmp.exists():
            with contextlib.suppress(OSError):
                tmp.unlink()


def _validate_event_record(record: Mapping[str, Any]) -> None:
    if record.get("schema_version") != SCHEMA_VERSION:
        raise ValueError(f"schema_version must be {SCHEMA_VERSION!r}")
    if record.get("event_type") not in VALID_EVENT_TYPES:
        raise ValueError(f"event_type must be one of {sorted(VALID_EVENT_TYPES)!r}")
    fid = record.get("finding_id")
    if not isinstance(fid, str) or not fid.strip():
        raise ValueError("finding_id must be a non-empty string")
    if not isinstance(record.get("finding_payload"), dict):
        raise ValueError("finding_payload must be a dict")


def _append_event_locked(
    event_type: str,
    finding: Finding,
    *,
    path: Path | None = None,
    lock_path: Path | None = None,
    agent: str | None = None,
    notes: str | None = None,
) -> dict[str, Any]:
    record = {
        "schema_version": SCHEMA_VERSION,
        "event_type": event_type,
        "finding_id": finding.finding_id,
        "finding_payload": finding.to_dict(),
        "written_at_utc": _utc_now_iso(),
        "written_pid": os.getpid(),
        "written_host": socket.gethostname(),
        "agent": agent or "claude",
        "notes": notes,
    }
    _validate_event_record(record)
    p_path = path or FINDINGS_REGISTRY_PATH
    l_path = lock_path or FINDINGS_REGISTRY_LOCK
    with registry_lock(l_path):
        try:
            existing = load_events_strict(p_path)
        except FindingsRegistryCorruptError:
            raise
        existing.append(record)
        _save_ledger(existing, p_path)
    return record


def register_finding(
    finding: Finding,
    *,
    path: Path | None = None,
    lock_path: Path | None = None,
    agent: str | None = None,
    notes: str | None = None,
) -> Finding:
    """Append a 'registered' event; returns the finding echo."""
    if not isinstance(finding, Finding):
        raise InvalidFindingError(
            f"register_finding expected Finding, got {type(finding).__name__}"
        )
    _append_event_locked(
        EVENT_REGISTERED,
        finding,
        path=path,
        lock_path=lock_path,
        agent=agent,
        notes=notes,
    )
    return finding


def append_anchor(
    finding_id: str,
    anchor: EmpiricalAnchor,
    *,
    path: Path | None = None,
    lock_path: Path | None = None,
    agent: str | None = None,
    notes: str | None = None,
) -> Finding:
    """Append an anchor to an existing finding; emits 'anchor_appended'."""
    if not isinstance(anchor, EmpiricalAnchor):
        raise InvalidFindingError(
            f"append_anchor expected EmpiricalAnchor, got {type(anchor).__name__}"
        )
    p_path = path or FINDINGS_REGISTRY_PATH
    l_path = lock_path or FINDINGS_REGISTRY_LOCK
    with registry_lock(l_path):
        existing = load_events_strict(p_path)
        latest_payload = None
        for row in existing:
            if row.get("finding_id") == finding_id:
                latest_payload = row.get("finding_payload")
        if latest_payload is None:
            raise InvalidFindingError(
                f"finding_id={finding_id!r} not found; call register_finding first"
            )
        finding = Finding.from_dict(latest_payload)
        updated = finding.with_new_anchor(anchor)
        _append_event_locked(
            EVENT_ANCHOR_APPENDED,
            updated,
            path=p_path,
            lock_path=l_path,
            agent=agent,
            notes=notes,
        )
    return updated


# ---------------------------------------------------------------------------
# Queries (latest-event-per-id)
# ---------------------------------------------------------------------------


def query_findings(*, path: Path | None = None) -> list[Finding]:
    """Return the latest payload per finding_id as reconstructed Findings."""
    rows = load_events_lenient(path)
    latest_by_id: dict[str, dict[str, Any]] = {}
    for row in rows:
        fid = row.get("finding_id")
        if isinstance(fid, str):
            latest_by_id[fid] = row.get("finding_payload", {})
    out: list[Finding] = []
    for payload in latest_by_id.values():
        try:
            out.append(Finding.from_dict(payload))
        except (KeyError, InvalidFindingError):
            # Skip historical rows that fail current invariants (forward-compat).
            continue
    return out


def get_finding(finding_id: str, *, path: Path | None = None) -> Finding | None:
    for f in query_findings(path=path):
        if f.finding_id == finding_id:
            return f
    return None


def query_by_consumer(consumer: str, *, path: Path | None = None) -> list[Finding]:
    if not consumer:
        return []
    out = []
    for f in query_findings(path=path):
        if any(consumer in c or c in consumer for c in f.consumers):
            out.append(f)
    return out


def query_by_producer(producer: str, *, path: Path | None = None) -> list[Finding]:
    if not producer:
        return []
    out = []
    for f in query_findings(path=path):
        if any(producer in p or p in producer for p in f.producers):
            out.append(f)
    return out


def query_by_domain(token: str, *, path: Path | None = None) -> list[Finding]:
    tok = (token or "").lower()
    if not tok:
        return []
    out = []
    for f in query_findings(path=path):
        if tok in json.dumps(dict(f.domain_of_validity)).lower():
            out.append(f)
    return out


# ---------------------------------------------------------------------------
# Seed findings — real molt keystones from MEMORY.md, honestly anchored
# ---------------------------------------------------------------------------
#
# Each seed encodes an EXISTING measured keystone so the registry is not empty
# theater. Every one carries a real anchor (with its commit/authority) and a
# real producer/consumer so the ORPHAN BAN is satisfied honestly — not by a
# placeholder. Timestamps are FIXED (the landing date) so ``seed`` is
# reproducible and the committed JSONL is byte-stable across re-runs.

_SEED_UTC = "2026-07-10T00:00:00Z"


def build_seed_findings() -> list[Finding]:
    """Construct the seeded keystone findings (M47, M46, M55, M09)."""
    findings: list[Finding] = []

    # M47 — int-mul CheckedMul peel: probe_int 1.65x CPython (raw-lane int mul).
    findings.append(
        Finding(
            finding_id="probe_int_checkedmul_peel_v1",
            one_line_summary=(
                "CheckedMul overflow-peel makes probe_int raw-lane int-mul 1.65x "
                "CPython (Cranelift smulhi hi-word overflow check)"
            ),
            claim=(
                "molt build --release probe_int.py wall <= 0.61 * CPython wall on "
                "the checked-int-multiply hot loop (1.65x speedup); native raw i64 "
                "lane with a peeled smulhi overflow guard"
            ),
            domain_of_validity={
                "targets": ["native-cranelift"],
                "os": ["windows", "linux", "macos"],
                "py_version": ">=3.12",
                "workload": "probe_int.py checked-int-multiply accumulator loop",
                "verified_subset": "M02",
            },
            anchors=(
                EmpiricalAnchor(
                    anchor_id="probe_int_checkedmul_peel_landed_261efc7b2",
                    predicted="ratio ~1.6x CPython",
                    measured="1.65x CPython",
                    residual=0.03,
                    authority_tier=AUTHORITY_MOLT_BUILD_RELEASE,
                    measurement_method=(
                        "molt build --release probe_int.py; wall vs CPython 3.12"
                    ),
                    source_artifact="commit 261efc7b2 (int-mul CheckedMul peel LANDED)",
                    measured_utc=_SEED_UTC,
                ),
            ),
            producers=("commit 261efc7b2", "tools/bench_evidence.py"),
            consumers=(
                "docs/agent/PERF_AUTHORITY.md",
                "tools/check_perf_freshness.py",
                "memory/M47",
            ),
            verification=VERIFIED_VIA_EMPIRICAL_ANCHOR,
            next_recalibration_trigger=RECALIBRATE_ON_RESIDUAL_DRIFT,
            created_utc=_SEED_UTC,
            last_calibration_utc=_SEED_UTC,
        )
    )

    # M46 — regular for/while int+float accumulators raw-lane: 1.28-2.3x CPython.
    findings.append(
        Finding(
            finding_id="loop_accumulator_raw_lane_v1",
            one_line_summary=(
                "Regular for/while int+float accumulator loops raw-lane at "
                "1.28-2.3x CPython; sum(genexpr/listcomp over range) raw-lanes too"
            ),
            claim=(
                "molt build --release int/float accumulator for/while loops run "
                "1.28x-2.3x CPython wall (native raw scalar lane, no per-iter box); "
                "sum() over range genexpr/listcomp raw-lanes"
            ),
            domain_of_validity={
                "targets": ["native-cranelift"],
                "os": ["windows", "linux", "macos"],
                "py_version": ">=3.12",
                "workload": "int/float accumulator for/while loops; sum over range",
                "excluded_contexts": [
                    "non-range iterables",
                    "filtered/multi-for comprehensions",
                    "min/max reductions",
                ],
                "verified_subset": "M02",
            },
            anchors=(
                EmpiricalAnchor(
                    anchor_id="loop_accumulator_raw_lane_band_low",
                    predicted="raw-lane speedup band vs CPython",
                    measured="1.28x CPython (band floor)",
                    residual=0.0,
                    authority_tier=AUTHORITY_MOLT_BUILD_RELEASE,
                    measurement_method=(
                        "molt build --release accumulator loops; wall vs CPython 3.12"
                    ),
                    source_artifact="commit dcc00a506 (sum genexpr/listcomp raw-lane)",
                    measured_utc=_SEED_UTC,
                ),
                EmpiricalAnchor(
                    anchor_id="loop_accumulator_raw_lane_band_high",
                    predicted="raw-lane speedup band vs CPython",
                    measured="2.3x CPython (band ceiling)",
                    residual=0.0,
                    authority_tier=AUTHORITY_MOLT_BUILD_RELEASE,
                    measurement_method=(
                        "molt build --release accumulator loops; wall vs CPython 3.12"
                    ),
                    source_artifact="commit dcc00a506 (sum genexpr/listcomp raw-lane)",
                    measured_utc=_SEED_UTC,
                ),
            ),
            producers=("commit dcc00a506", "tools/bench_evidence.py"),
            consumers=(
                "docs/agent/PERF_AUTHORITY.md",
                "memory/M46",
                "docs/foundation/spectral-norm-perf-red",
            ),
            verification=VERIFIED_VIA_EMPIRICAL_ANCHOR,
            next_recalibration_trigger=RECALIBRATE_ON_RESIDUAL_DRIFT,
            created_utc=_SEED_UTC,
            last_calibration_utc=_SEED_UTC,
        )
    )

    # M55 — frontend lowering dominates witness cold compile (~180s serial).
    findings.append(
        Finding(
            finding_id="witness_frontend_lowering_cold_cost_v1",
            one_line_summary=(
                "Frontend lowering dominates cold witness compile: ~180s serial "
                "re-lower of numpy/build; Tarjan SCC condensation fixes serial bail"
            ),
            claim=(
                "cold witness (numpy+scipy) frontend lowering ~= 180s serial when the "
                "shared lowering cache misses; parallel frontend must Tarjan-SCC "
                "condense rather than bail fully serial on any cycle/timeout"
            ),
            domain_of_validity={
                "targets": ["wasm-witness"],
                "os": ["windows", "linux", "macos"],
                "py_version": ">=3.12",
                "workload": "cold numpy+scipy witness frontend lowering",
                "verified_subset": "M02",
            },
            anchors=(
                EmpiricalAnchor(
                    anchor_id="witness_frontend_lowering_cold_180s",
                    predicted="frontend lowering is the witness-compile hotspot",
                    measured="~180s serial cold re-lower (numpy/build)",
                    residual=0.0,
                    authority_tier=AUTHORITY_BUILD_WALL_CLOCK,
                    measurement_method=(
                        "differential native-vs-wasm phase timing on cold witness "
                        "compile (M71 technique); serial re-lower wall"
                    ),
                    source_artifact=(
                        "commit 522b7fe04 (Tarjan SCC condensation fixes serial bail)"
                    ),
                    measured_utc=_SEED_UTC,
                    noise_floor=20.0,
                    noise_floor_provenance=(
                        "cold-compile wall varies ~+/-20s across runs on the "
                        "canonical NVMe box (C:\\Molt); single-box wall, not "
                        "multi-seed statistical"
                    ),
                ),
            ),
            producers=("commit 522b7fe04", "commit 522b7fe04 (SCC condensation)"),
            consumers=(
                "docs/agent/CODEX_CENTURY_GOAL.md",
                "memory/M55",
                "memory/M57",
            ),
            verification=VERIFIED_VIA_EMPIRICAL_ANCHOR,
            next_recalibration_trigger=RECALIBRATE_ON_NEW_ANCHORS,
            created_utc=_SEED_UTC,
            last_calibration_utc=_SEED_UTC,
        )
    )

    # M09 — build wall-clock: incremental-when-sccache-off landed; sccache HARMFUL
    # on Windows. This is a build-time keystone; the anchor is source-level (the
    # landing) because the wall-clock delta is host-sensitive and gated elsewhere.
    findings.append(
        Finding(
            finding_id="incremental_build_sccache_off_v1",
            one_line_summary=(
                "Incremental compilation (sccache OFF) is the Windows build-time "
                "lever; sccache is HARMFUL on Windows and off by default"
            ),
            claim=(
                "on Windows, CARGO incremental=on with sccache DISABLED beats "
                "sccache-on for the molt rebuild inner loop; sccache is off by "
                "default because it regresses Windows metadata-heavy rebuilds"
            ),
            domain_of_validity={
                "targets": ["native-cranelift", "wasm-witness"],
                "os": ["windows"],
                "py_version": ">=3.12",
                "workload": "molt runtime-crate rebuild inner loop",
                "verified_subset": "M02",
            },
            anchors=(
                EmpiricalAnchor(
                    anchor_id="incremental_when_sccache_off_landed_aa15340aa",
                    predicted="incremental-on + sccache-off is the faster Windows lane",
                    measured=(
                        "incremental-when-sccache-off LANDED as the default; sccache "
                        "off by default (measured HARMFUL on Windows)"
                    ),
                    residual=0.0,
                    authority_tier=AUTHORITY_SOURCE_INSPECTION,
                    measurement_method=(
                        "build wall-clock A/B on the canonical box; landed as the "
                        "default lever (M09 BINDING build-time attack)"
                    ),
                    source_artifact="commit aa15340aa (incremental-when-sccache-off)",
                    measured_utc=_SEED_UTC,
                ),
            ),
            producers=("commit aa15340aa",),
            consumers=(
                "docs/agent/BUILD_TIME.md",
                "memory/M09",
                "tools/compile_governor.py",
            ),
            verification=VERIFIED_VIA_SOURCE_INSPECTION,
            next_recalibration_trigger=RECALIBRATE_ON_OPERATOR,
            created_utc=_SEED_UTC,
            last_calibration_utc=_SEED_UTC,
        )
    )

    return findings


def seed_registry(
    *, path: Path | None = None, lock_path: Path | None = None
) -> tuple[list[str], list[str]]:
    """Register any seed findings not already present. Idempotent.

    Returns ``(registered_ids, skipped_ids)``. A seed already present (by id) is
    skipped so re-running ``seed`` does not bloat the ledger.
    """
    p_path = path or FINDINGS_REGISTRY_PATH
    l_path = lock_path or FINDINGS_REGISTRY_LOCK
    registered: list[str] = []
    skipped: list[str] = []
    with registry_lock(l_path):
        present = {f.finding_id for f in query_findings(path=p_path)}
        for finding in build_seed_findings():
            if finding.finding_id in present:
                skipped.append(finding.finding_id)
                continue
            register_finding(
                finding,
                path=p_path,
                lock_path=l_path,
                agent="apparatus-a4-seed",
                notes="seeded keystone from MEMORY.md",
            )
            registered.append(finding.finding_id)
    return registered, skipped


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _print_finding(f: Finding, *, verbose: bool = False) -> None:
    cal = "well-calibrated" if f.is_well_calibrated else "uncalibrated"
    print(f"{f.finding_id}  [{f.verification}] [{cal}]")
    print(f"    {f.one_line_summary}")
    if verbose:
        print(f"    claim: {f.claim}")
        print(f"    domain: {json.dumps(dict(f.domain_of_validity))}")
        print(f"    producers: {list(f.producers)}")
        print(f"    consumers: {list(f.consumers)}")
        for a in f.anchors:
            floor = (
                f" (noise_floor={a.noise_floor})" if a.noise_floor is not None else ""
            )
            print(
                f"    anchor {a.anchor_id}: predicted={a.predicted!r} "
                f"measured={a.measured!r} residual={a.residual} "
                f"[{a.authority_tier}]{floor}"
            )


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    parser = argparse.ArgumentParser(
        description="molt findings registry (APPARATUS A4)."
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_list = sub.add_parser("list", help="list all findings (latest per id)")
    p_list.add_argument("-v", "--verbose", action="store_true")
    p_list.add_argument("--json", action="store_true")

    p_get = sub.add_parser("get", help="get one finding by id")
    p_get.add_argument("finding_id")
    p_get.add_argument("--json", action="store_true")

    p_query = sub.add_parser("query", help="query by consumer / producer / domain")
    p_query.add_argument("--consumer")
    p_query.add_argument("--producer")
    p_query.add_argument("--domain")
    p_query.add_argument("-v", "--verbose", action="store_true")
    p_query.add_argument("--json", action="store_true")

    p_reg = sub.add_parser("register", help="register a finding from a JSON payload")
    p_reg.add_argument(
        "--from-json", required=True, help="path to a Finding.to_dict() JSON"
    )

    sub.add_parser("seed", help="register the seeded keystone findings (idempotent)")

    args = parser.parse_args(argv)

    if args.cmd == "list":
        findings = sorted(query_findings(), key=lambda f: f.finding_id)
        if args.json:
            print(json.dumps([f.to_dict() for f in findings], indent=2, sort_keys=True))
        else:
            if not findings:
                print("(registry empty)")
            for f in findings:
                _print_finding(f, verbose=args.verbose)
        return 0

    if args.cmd == "get":
        f = get_finding(args.finding_id)
        if f is None:
            print(f"finding_id={args.finding_id!r} not found", file=sys.stderr)
            return 1
        if args.json:
            print(json.dumps(f.to_dict(), indent=2, sort_keys=True))
        else:
            _print_finding(f, verbose=True)
        return 0

    if args.cmd == "query":
        results: list[Finding] = []
        if args.consumer:
            results = query_by_consumer(args.consumer)
        elif args.producer:
            results = query_by_producer(args.producer)
        elif args.domain:
            results = query_by_domain(args.domain)
        else:
            print("query needs one of --consumer/--producer/--domain", file=sys.stderr)
            return 2
        if args.json:
            print(json.dumps([f.to_dict() for f in results], indent=2, sort_keys=True))
        else:
            if not results:
                print("(no matches)")
            for f in sorted(results, key=lambda f: f.finding_id):
                _print_finding(f, verbose=args.verbose)
        return 0

    if args.cmd == "register":
        payload = json.loads(Path(args.from_json).read_text(encoding="utf-8"))
        finding = Finding.from_dict(payload)  # raises on orphan / invalid
        register_finding(finding)
        print(f"registered {finding.finding_id}")
        return 0

    if args.cmd == "seed":
        registered, skipped = seed_registry()
        print(
            f"seed: registered {len(registered)} "
            f"({', '.join(registered) or '-'}); skipped {len(skipped)} "
            f"already-present ({', '.join(skipped) or '-'})"
        )
        return 0

    return 2


if __name__ == "__main__":
    raise SystemExit(main())
