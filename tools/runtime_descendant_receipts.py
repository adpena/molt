"""Admission of captured self-image runtime test descendants.

Some runtime tests re-execute their own Cargo test image to observe process
exit, abort and cold-runtime behavior. The shared Rust producer
``runtime/test_support/captured_runtime_children.rs`` retains each child's
complete streams in Cargo test-image custody and writes one typed record to
the owning test's stderr. This module is the single consumer authority used by
the Cargo binary runner, the Cargo truth loader and the runtime gate.

Every check re-opens raw evidence: the parent's full captures are re-hashed and
its libtest rows re-derived, the mandatory roster comes from the owners that
actually passed (never from whichever records are present), and each
descendant image, exact argv, typed termination and retained stream is bound
again. Saved summaries are never trusted.

The receipt-only gate has no immutable CPython target-minor authority. Records
declare ``coordinate_authority: unavailable``; a requested minor fails closed.
"""

from __future__ import annotations

import hashlib
import os
from collections.abc import Mapping
from dataclasses import dataclass
from io import StringIO
from pathlib import Path

from molt.exact_json import loads_exact
from tools.libtest_results import parse_libtest

RECORD_PREFIX = "MOLT_RUNTIME_DESCENDANT_RECEIPT "
RECORD_SCHEMA = "molt.runtime-descendant.v1"
SOURCE_IDENTITY_ENV = "MOLT_TEST_SOURCE_IDENTITY_JSON"
STREAM_LABEL = b"runtime-descendant"
COORDINATE_AUTHORITY = "unavailable"
MAX_RECORD_CHARS = 1 << 20
_RECORD_FIELDS = frozenset(
    {
        "schema",
        "role",
        "parent_test",
        "child_test",
        "mode",
        "source_identity",
        "executable",
        "executable_sha256",
        "argv",
        "termination",
        "coordinate_authority",
        "stdout",
        "stderr",
    }
)
_EXIT_ZERO = {"kind": "exit", "code": 0}
# std::process::abort: SIGABRT on POSIX; fast-fail STATUS_STACK_BUFFER_OVERRUN
# on Windows. Neither is an ordinary exit code, and neither admits the other.
_ABORT = {
    "posix": {"kind": "signal", "signal": 6},
    "nt": {"kind": "windows-exception", "code": 0xC0000409},
}


class DescendantEvidenceError(RuntimeError):
    """A runtime descendant cannot be admitted as evidence."""


@dataclass(frozen=True, slots=True)
class Owner:
    """One owning test and the exact children it must have run."""

    role: str
    child: str
    modes: tuple[str, ...]
    ignored: bool = False
    aborts: bool = False
    # Children that exit or abort inside the selected test never reach a
    # libtest summary; the cold child must complete its one exact test.
    completes: bool = False
    stdout_markers: tuple[str, ...] = ()
    stderr_markers: tuple[str, ...] = ()

    def argv(self) -> list[str]:
        return [
            "--exact",
            self.child,
            *(["--ignored"] if self.ignored else []),
            "--nocapture",
            "--test-threads=1",
        ]


EXIT = (
    "state::lifecycle::shutdown_tests::"
    "process_exit_covers_collection_before_pending_callbacks_with_or_without_lease"
)
TRAP = "call::function::tests::assert_no_pending_on_success_traps_stale_exception"
COLD = (
    "wasm_abi_exports::tests::scratch_alloc_cold_resource_denial_is_null_and_nounwind"
)
OWNERS: Mapping[str, Owner] = {
    EXIT: Owner(
        "process-exit-callbacks",
        EXIT,
        ("no-lease", "lease"),
        ignored=True,
        stdout_markers=("shutdown callbacks verified before process exit",),
    ),
    TRAP: Owner(
        "pending-success-trap",
        "call::function::tests::assert_no_pending_on_success_child",
        ("stale-exception",),
        aborts=True,
        stderr_markers=("pending exception on success path",),
    ),
    COLD: Owner("cold-resource-denial", COLD, ("cold",), completes=True),
    "trace_callargs_emits_builder_lifecycle_logs": Owner(
        "trace-call-binding",
        "trace_callargs_child",
        ("trace_callargs_child",),
        stderr_markers=(
            "[molt callargs] new",
            "[molt callargs] push_pos",
            "[molt callargs] free",
        ),
    ),
    "trace_call_bind_ic_emits_hit_log": Owner(
        "trace-call-binding",
        "trace_call_bind_ic_child",
        ("trace_call_bind_ic_child",),
        stderr_markers=("[molt call_bind_ic] hit",),
    ),
    "trace_function_bind_meta_emits_summary": Owner(
        "trace-call-binding",
        "trace_function_bind_meta_child",
        ("trace_function_bind_meta_child",),
        stderr_markers=("[molt bind_meta]", "total_pos=0", "kwonly=1"),
    ),
}


def _canonical(value: object) -> Path:
    if not isinstance(value, str) or not value:
        raise DescendantEvidenceError("evidence path is not a nonempty string")
    if value.startswith("\\\\?\\UNC\\"):
        value = "\\\\" + value[8:]
    elif value.startswith("\\\\?\\"):
        value = value[4:]
    return Path(value).resolve()


def _same_path(left: Path, right: Path) -> bool:
    return os.path.normcase(str(left)) == os.path.normcase(str(right))


def _file_identity(path: Path) -> tuple[int, str]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            size += len(chunk)
            digest.update(chunk)
    return size, digest.hexdigest()


def _exact(value: object, expected: object) -> bool:
    """JSON equality that never conflates bool/int or int/float."""
    if type(value) is not type(expected):
        return False
    if isinstance(expected, dict):
        assert isinstance(value, dict)
        return value.keys() == expected.keys() and all(
            _exact(value[key], expected[key]) for key in expected
        )
    if isinstance(expected, list):
        assert isinstance(value, list)
        return len(value) == len(expected) and all(
            _exact(item, other) for item, other in zip(value, expected)
        )
    return value == expected


def _records(path: Path) -> list[dict[str, object]]:
    records: list[dict[str, object]] = []
    with path.open("r", encoding="utf-8", errors="replace") as handle:
        while line := handle.readline(MAX_RECORD_CHARS + 1):
            if len(line) > MAX_RECORD_CHARS:
                if line.startswith(RECORD_PREFIX):
                    raise DescendantEvidenceError("oversized runtime descendant record")
                while not line.endswith("\n"):
                    line = handle.readline(MAX_RECORD_CHARS + 1)
                    if not line:
                        break
                continue
            text = line.rstrip("\r\n")
            if not text.startswith(RECORD_PREFIX):
                continue
            try:
                record = loads_exact(text[len(RECORD_PREFIX) :])
            except ValueError as exc:
                raise DescendantEvidenceError(
                    f"malformed runtime descendant record: {exc}"
                ) from exc
            if not isinstance(record, dict):
                raise DescendantEvidenceError(
                    "runtime descendant record is not an object"
                )
            records.append(record)
    return records


def _engaged(receipt: Mapping[str, object]) -> bool:
    """Whether raw parent evidence or saved rows show an owner family."""
    rows = receipt.get("test_results")
    if isinstance(rows, list) and any(
        isinstance(row, dict) and row.get("identity") in OWNERS for row in rows
    ):
        return True
    executions = receipt.get("executions")
    if not isinstance(executions, list) or not executions:
        return False
    baseline = executions[0]
    if not isinstance(baseline, dict):
        return False
    argv = baseline.get("argv")
    for stream in ("stdout", "stderr"):
        value = baseline.get(f"{stream}_evidence")
        if not isinstance(value, str) or not value or not Path(value).is_file():
            continue
        try:
            if _records(Path(value)):
                return True
            if stream == "stdout" and isinstance(argv, list):
                with Path(value).open(
                    "r", encoding="utf-8", errors="replace"
                ) as handle:
                    report = parse_libtest(handle, tuple(str(item) for item in argv))
                if any(row["identity"] in OWNERS for row in report.rows()):
                    return True
        except (OSError, DescendantEvidenceError):
            # Unreadable or malformed evidence at an owner-shaped location must
            # be judged by full verification, not silently skipped.
            return True
    return False


def _parent_capture(
    receipt: Mapping[str, object],
    execution: Mapping[str, object],
    stream: str,
    custody_root: Path,
) -> Path:
    path = _canonical(execution.get(f"{stream}_evidence"))
    invocation = receipt.get("invocation_id")
    if (
        not isinstance(invocation, str)
        or not invocation
        or not _same_path(path.parent, custody_root / "evidence" / invocation)
        or not path.name.endswith(f".{stream}.log")
    ):
        raise DescendantEvidenceError(
            f"parent {stream} capture escaped receipt custody"
        )
    size, digest = _file_identity(path)
    recorded = execution.get(f"{stream}_bytes")
    if type(recorded) is not int or (recorded, execution.get(f"{stream}_sha256")) != (
        size,
        digest,
    ):
        raise DescendantEvidenceError(
            f"parent {stream} capture changed after publication"
        )
    return path


def _descendant_stream(value: object, *, image: Path, name: str) -> tuple[Path, str]:
    if not isinstance(value, dict) or set(value) != {"path", "bytes", "sha256"}:
        raise DescendantEvidenceError(f"unbound runtime descendant {name} stream")
    path = _canonical(value["path"])
    owner = path.parent
    root = (image.parent / "molt-test-artifacts").resolve()
    if path.name != f"{name}.log" or not _same_path(owner.parent, root):
        raise DescendantEvidenceError(
            f"runtime descendant {name} stream escaped Cargo test-image custody"
        )
    if (owner / "artifact-label.txt").read_bytes() != STREAM_LABEL:
        raise DescendantEvidenceError(
            f"runtime descendant {name} stream owner is not a descendant capture"
        )
    raw = path.read_bytes()
    if (
        type(value["bytes"]) is not int
        or value["bytes"] != len(raw)
        or value["sha256"] != hashlib.sha256(raw).hexdigest()
    ):
        raise DescendantEvidenceError(f"runtime descendant {name} stream changed")
    return owner, raw.decode("utf-8", errors="replace")


def _verify_record(
    record: dict[str, object],
    *,
    source: object,
    image: Path,
    image_sha256: object,
    platform: str,
) -> str:
    """Verify one record whose (owner, mode) is admitted; return its stream owner."""
    parent = record["parent_test"]
    assert isinstance(parent, str)
    owner = OWNERS[parent]
    if not _exact(record["source_identity"], source):
        raise DescendantEvidenceError(
            f"{parent}: descendant source identity differs from its parent receipt"
        )
    if (
        not _same_path(_canonical(record["executable"]), image)
        or record["executable_sha256"] != image_sha256
    ):
        raise DescendantEvidenceError(f"{parent}: descendant executed a foreign image")
    argv = record["argv"]
    if (
        not isinstance(argv, list)
        or not argv
        or not all(isinstance(item, str) for item in argv)
        or not _same_path(_canonical(argv[0]), image)
        or argv[1:] != owner.argv()
        or record["role"] != owner.role
        or record["child_test"] != owner.child
    ):
        raise DescendantEvidenceError(f"{parent}: wrong descendant role or exact argv")
    expected = _ABORT.get(platform) if owner.aborts else _EXIT_ZERO
    if expected is None:
        raise DescendantEvidenceError(
            f"{parent}: no typed abort termination for platform {platform!r}"
        )
    if not _exact(record["termination"], expected):
        raise DescendantEvidenceError(
            f"{parent} ({record['mode']}): unexpected descendant termination "
            f"{record['termination']!r}"
        )
    stdout_owner, stdout = _descendant_stream(
        record["stdout"], image=image, name="stdout"
    )
    stderr_owner, stderr = _descendant_stream(
        record["stderr"], image=image, name="stderr"
    )
    if not _same_path(stdout_owner, stderr_owner):
        raise DescendantEvidenceError(f"{parent}: descendant streams have two owners")
    report = parse_libtest(StringIO(stdout), tuple(argv))
    if owner.completes:
        if not report.complete or report.rows() != [
            {"identity": owner.child, "status": "pass"}
        ]:
            raise DescendantEvidenceError(
                f"{parent}: descendant lacks exact one-test completion"
            )
    elif (
        report.issues
        or report.declared != 1
        or report.observations
        or report.pending != (owner.child,)
        or report.summary is not None
    ):
        raise DescendantEvidenceError(
            f"{parent}: descendant did not start exactly its selected test "
            "before terminating inside it"
        )
    for markers, text, stream in (
        (owner.stdout_markers, stdout, "stdout"),
        (owner.stderr_markers, stderr, "stderr"),
    ):
        missing = [marker for marker in markers if marker not in text]
        if missing:
            raise DescendantEvidenceError(
                f"{parent} ({record['mode']}): descendant {stream} lacks {missing!r}"
            )
    return os.path.normcase(str(stdout_owner))


def _verify(
    receipt: Mapping[str, object],
    *,
    receipt_root: Path | None,
    platform: str,
) -> dict[str, object]:
    executions = receipt.get("executions")
    if (
        receipt.get("resource_process_isolation") is not False
        or not isinstance(executions, list)
        or not executions
        or not isinstance(executions[0], dict)
    ):
        raise DescendantEvidenceError(
            "runtime descendants require one baseline libtest execution"
        )
    baseline = executions[0]
    argv = baseline.get("argv")
    if (
        not isinstance(argv, list)
        or not argv
        or not all(isinstance(item, str) for item in argv)
    ):
        raise DescendantEvidenceError("baseline execution lacks exact argv")
    image = _canonical(receipt.get("executable_resolved"))
    image_sha256 = receipt.get("executable_sha256")
    image_size = receipt.get("executable_size")
    if type(image_size) is not int or _file_identity(image) != (
        image_size,
        image_sha256,
    ):
        raise DescendantEvidenceError("parent test image changed after publication")
    if not _same_path(_canonical(argv[0]), image):
        raise DescendantEvidenceError(
            "baseline execution did not run the receipt image"
        )
    custody_root = _canonical(receipt.get("receipt_custody_root"))
    if receipt_root is not None and not _same_path(
        custody_root, Path(receipt_root).resolve()
    ):
        raise DescendantEvidenceError("receipt escaped its loader's custody root")
    stdout_path = _parent_capture(receipt, baseline, "stdout", custody_root)
    stderr_path = _parent_capture(receipt, baseline, "stderr", custody_root)
    with stdout_path.open("r", encoding="utf-8", errors="replace") as handle:
        report = parse_libtest(handle, tuple(argv))
    rows = [] if baseline.get("infrastructure_failure") is not None else report.rows()
    if rows != receipt.get("test_results"):
        raise DescendantEvidenceError(
            "re-derived parent libtest rows disagree with the saved ledger"
        )
    if receipt.get("status") == "success" and not report.complete:
        raise DescendantEvidenceError(
            "successful parent lacks complete re-derived libtest accounting"
        )
    if _records(stdout_path):
        raise DescendantEvidenceError(
            "runtime descendant record escaped its canonical stderr stream"
        )
    records = _records(stderr_path)
    completed = {
        row["identity"]: row["status"]
        for row in rows
        if row["identity"] in OWNERS and row["status"] in {"pass", "fail"}
    }
    required = {
        (parent, mode)
        for parent, status in completed.items()
        if status == "pass"
        for mode in OWNERS[parent].modes
    }
    source = receipt.get("source_identity")
    if (required or records) and not isinstance(source, dict):
        raise DescendantEvidenceError(
            "runtime descendant source identity is not admitted"
        )
    observed: set[tuple[str, str]] = set()
    owners: set[str] = set()
    for record in records:
        if (
            set(record) != _RECORD_FIELDS
            or record["schema"] != RECORD_SCHEMA
            or record["coordinate_authority"] != COORDINATE_AUTHORITY
        ):
            raise DescendantEvidenceError(
                "foreign runtime descendant schema or coordinate authority"
            )
        parent, mode = record["parent_test"], record["mode"]
        if (
            not isinstance(parent, str)
            or not isinstance(mode, str)
            or parent not in completed
            or mode not in OWNERS[parent].modes
        ):
            raise DescendantEvidenceError(
                f"runtime descendant without a completed owner: {parent!r} ({mode!r})"
            )
        if (parent, mode) in observed:
            raise DescendantEvidenceError(
                f"duplicate runtime descendant record: {parent} ({mode})"
            )
        observed.add((parent, mode))
        owner = _verify_record(
            record,
            source=source,
            image=image,
            image_sha256=image_sha256,
            platform=platform,
        )
        if owner in owners:
            raise DescendantEvidenceError("runtime descendant streams share one owner")
        owners.add(owner)
    missing = sorted(required - observed)
    if missing:
        raise DescendantEvidenceError(
            f"missing mandatory runtime descendant records: {missing!r}"
        )
    return {
        "status": "verified",
        "children": len(records),
        "coordinate_authority": COORDINATE_AUTHORITY,
    }


def verify_receipt(
    receipt: Mapping[str, object],
    *,
    receipt_root: Path | None = None,
    platform: str | None = None,
    required_minor: str | None = None,
) -> dict[str, object] | None:
    """Re-derive one binary receipt's descendant contract from raw evidence.

    Returns ``None`` when neither saved rows nor raw parent evidence show an
    owner family, so unrelated binaries keep their existing accounting.
    Raises :class:`DescendantEvidenceError` on any violation.
    """
    if required_minor is not None:
        raise DescendantEvidenceError(
            "receipt-only runtime gate has no immutable CPython minor authority; "
            f"requested minor {required_minor!r} is not admissible"
        )
    if not _engaged(receipt):
        return None
    try:
        return _verify(
            receipt,
            receipt_root=receipt_root,
            platform=os.name if platform is None else platform,
        )
    except (OSError, ValueError) as exc:
        raise DescendantEvidenceError(
            f"unreadable runtime descendant evidence: {exc}"
        ) from exc


def receipt_outcome(
    receipt: Mapping[str, object], *, receipt_root: Path | None = None
) -> dict[str, object] | None:
    """Typed verification outcome for a receipt, or ``None`` when unrelated."""
    try:
        return verify_receipt(receipt, receipt_root=receipt_root)
    except DescendantEvidenceError as exc:
        return {
            "status": "failed",
            "error": str(exc),
            "coordinate_authority": COORDINATE_AUTHORITY,
        }
