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
from tools.libtest_results import (
    isolated_test_rows,
    listed_test_identities,
    parse_libtest,
)
from tools.proof_queue_pkg import custody_cas

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
# std::process::abort: SIGABRT on POSIX; fast-fail STATUS_STACK_BUFFER_OVERRUN
# on Windows. Neither is an ordinary exit code, and neither admits the other.
_ABORT = {
    "posix": {"kind": "signal", "signal": 6},
    "nt": {"kind": "windows-exception", "code": 0xC0000409},
}


class DescendantEvidenceError(RuntimeError):
    """A runtime descendant cannot be admitted as evidence."""


@dataclass(frozen=True, slots=True)
class Outcome:
    # None admits only the existing OS-specific abort, never an arbitrary signal.
    exit_code: int | None = 0
    completes: bool = False
    stdout_markers: tuple[str, ...] = ()
    stderr_markers: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class Owner:
    """One owning test and the exact children it must have run."""

    role: str
    child: str
    # Each real mode owns both its exact termination and libtest completion.
    modes: Mapping[str, Outcome]
    ignored: bool = False
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
TRANSACTION = "test_support::runtime_test_transactions_preserve_terminal_failures"
LIFECYCLE = (
    "state::runtime_state::tests::lifecycle_ffi_panics_fail_closed_without_unwinding"
)
DISCOVERY = "tests::pending_diagnostic_prints_and_retires_original_error_after_summary_encoding_failure"
ISOLATE_OWNER = (
    "concurrency::isolates::bootstrap_failure_tests::"
    "failed_isolate_preserves_state_and_blocks_process_retirement"
)
THREAD_ADMISSION = (
    "concurrency::isolates::bootstrap_failure_tests::"
    "native_thread_admission_and_origin_retirement"
)
ABI_MALFORMED_HOOKS = (
    "test_abi_transaction_rejects_malformed_first_install_before_attachment"
)
ABI_MISSING_TEXT = (
    "missing_string_and_bytes_allocators_select_refusal_over_an_incoming_error"
)
OWNERS: Mapping[str, Owner] = {
    "env_var_init_installs_tracker": Owner(
        "resource-startup",
        "env_var_init_installs_tracker_child",
        {"env_var_init_installs_tracker_child": Outcome(0, True)},
        ignored=True,
    ),
    "molt_memory_limit_alias_enforces_via_real_init_path": Owner(
        "resource-startup",
        "molt_memory_limit_alias_enforces_via_real_init_path_child",
        {"molt_memory_limit_alias_enforces_via_real_init_path_child": Outcome(0, True)},
        ignored=True,
    ),
    "no_memory_limit_env_means_unchanged_behavior": Owner(
        "resource-startup",
        "no_memory_limit_env_means_unchanged_behavior_child",
        {"no_memory_limit_env_means_unchanged_behavior_child": Outcome(0, True)},
        ignored=True,
    ),
    "human_size_and_raw_bytes_resolve_to_one_limit": Owner(
        "resource-startup",
        "human_size_and_raw_bytes_resolve_to_one_limit_child",
        {"human_size_and_raw_bytes_resolve_to_one_limit_child": Outcome(0, True)},
        ignored=True,
    ),
    ABI_MALFORMED_HOOKS: Owner(
        "abi-hook-registration",
        ABI_MALFORMED_HOOKS,
        {
            "malformed-first-install": Outcome(
                0,
                True,
                stdout_markers=("malformed-first-install-refused\n",),
            )
        },
    ),
    ABI_MISSING_TEXT: Owner(
        "abi-hook-registration",
        ABI_MISSING_TEXT,
        {
            "missing-text-allocators": Outcome(
                0,
                True,
                stdout_markers=("missing-text-allocators-refused\n",),
            )
        },
    ),
    ISOLATE_OWNER: Owner(
        "isolate-owner-lifetime",
        ISOLATE_OWNER,
        {
            mode: Outcome(
                0,
                True,
                stdout_markers=(f"isolate owner lifetime verified: {mode}\n",),
                stderr_markers=(
                    (
                        "MOLT_ISOLATE_TEARDOWN_PANIC: retaining failed isolate at",
                        "molt runtime shutdown refused: 1 active execution lease(s) remain live\n",
                    )
                    if mode != "successful"
                    else ()
                ),
            )
            for mode in ("successful", "failed", "thread-state")
        },
    ),
    THREAD_ADMISSION: Owner(
        "native-thread-admission",
        THREAD_ADMISSION,
        {
            **{
                mode: Outcome(
                    0,
                    True,
                    stdout_markers=(f"native thread lifetime verified: {mode}\n",),
                )
                for mode in (
                    "delayed-isolate",
                    "delayed-shared",
                    "cancel-before-start",
                    "spawn-failure-shared",
                    "spawn-failure-bytes",
                    "spawn-failure-isolate",
                    "nested-shared",
                    "shared-call-identity",
                    "shared-payload-panic",
                    "shared-cleanup-panic",
                    "joined-service-inflight",
                    "joined-service-admitted",
                    "joined-service-primary",
                    "joined-service-isolate",
                    "joined-service-partial-start",
                )
            },
            "joined-service-failed-join": Outcome(
                0,
                True,
                stdout_markers=(
                    "native thread lifetime verified: joined-service-failed-join\n",
                ),
                stderr_markers=(
                    "molt runtime lifecycle failed: runtime teardown refused: joined service workers failed; origin retained\n",
                ),
            ),
            "joined-service-cleanup-panic": Outcome(
                0,
                True,
                stdout_markers=(
                    "native thread lifetime verified: joined-service-cleanup-panic\n",
                ),
                stderr_markers=(
                    "molt runtime lifecycle failed: runtime teardown refused: joined service workers failed; origin retained\n",
                ),
            ),
            "joined-service-foreign-c": Outcome(
                0,
                True,
                stdout_markers=(
                    "native thread lifetime verified: joined-service-foreign-c\n",
                ),
                stderr_markers=(
                    "runtime teardown refused: foreign retained PyThreadState owners remain live",
                ),
            ),
            "joined-service-exit": Outcome(
                1,
                stderr_markers=(
                    "molt runtime process exit refused: joined service callback cannot retire its own runtime\n",
                ),
            ),
            "process-service-timeout": Outcome(
                0,
                True,
                stdout_markers=(
                    "native thread lifetime verified: process-service-timeout\n",
                ),
                stderr_markers=(
                    "molt runtime lifecycle failed: runtime teardown refused: process workers remain live or failed; origin retained\n",
                ),
            ),
            "exit-with-pending-isolate": Outcome(
                1,
                stderr_markers=(
                    "molt runtime process exit refused: 1 active execution lease(s) remain live\n",
                ),
            ),
        },
    ),
    DISCOVERY: Owner(
        "discovery-pending-diagnostic",
        "tests::pending_diagnostic_child",
        {
            "render-and-drain": Outcome(
                0,
                True,
                stdout_markers=("discovery diagnostic final drain verified\n",),
                stderr_markers=(
                    "[molt-cpython-abi] PyErr_Print: discovery exception custody\n",
                    "[molt-cpython-abi] PyErr_Print: \\ud800\n",
                    '===MOLT_DISCOVERY_EXC: pending exception value = "discovery exception custody"\n',
                    "===MOLT_DISCOVERY_EXC: no pending exception on NULL return",
                ),
            ),
        },
        ignored=True,
    ),
    TRANSACTION: Owner(
        "runtime-test-transaction",
        TRANSACTION,
        {
            mode: Outcome(
                0,
                True,
                stdout_markers=(f"transaction outcome and custody verified: {mode}\n",),
                stderr_markers=(
                    (
                        "molt runtime lifecycle failed: native owners survived the last callback drain before class retirement\n",
                    )
                    if mode in {"ordinary", "ordinary-return"}
                    else (
                        "molt runtime lifecycle failed: injected shutdown drain C extension cleanup panic\n",
                    )
                    if mode not in {"body-only", "healthy"}
                    else ()
                ),
            )
            for mode in (
                "prior",
                "cleanup",
                "both",
                "cold-both",
                "body-only",
                "ordinary",
                "ordinary-return",
                "reentry",
                "healthy",
            )
        },
    ),
    LIFECYCLE: Owner(
        "lifecycle-ffi",
        LIFECYCLE,
        {
            "init": Outcome(
                0,
                True,
                stderr_markers=(
                    "molt runtime lifecycle failed: injected unpublished runtime init panic\n",
                ),
            ),
            "shutdown": Outcome(
                0,
                True,
                stderr_markers=(
                    "molt runtime lifecycle failed: injected shutdown drain C extension cleanup panic\n",
                ),
            ),
            "exit": Outcome(
                1,
                stderr_markers=(
                    "molt runtime lifecycle failed: injected shutdown drain C extension cleanup panic\n",
                ),
            ),
            "shutdown-thread-state": Outcome(
                0,
                True,
                stdout_markers=(
                    "lifecycle thread-state retention verified: shutdown-thread-state\n",
                ),
                stderr_markers=(
                    "molt runtime lifecycle failed: injected thread-state finalizer panic\n",
                ),
            ),
            "exit-thread-state": Outcome(
                1,
                stdout_markers=(
                    "lifecycle thread-state failure armed: exit-thread-state\n",
                ),
                stderr_markers=(
                    "molt runtime lifecycle failed: injected thread-state finalizer panic\n",
                ),
            ),
        },
    ),
    EXIT: Owner(
        "process-exit-callbacks",
        EXIT,
        {
            mode: Outcome(
                stdout_markers=(
                    f"shutdown callbacks verified before process exit: {mode}\n",
                )
            )
            for mode in ("no-lease", "lease")
        },
        ignored=True,
    ),
    TRAP: Owner(
        "pending-success-trap",
        "call::function::tests::assert_no_pending_on_success_child",
        {"stale-exception": Outcome(None)},
        stderr_markers=("pending exception on success path",),
    ),
    COLD: Owner("cold-resource-denial", COLD, {"cold": Outcome(0, True)}),
    "trace_callargs_emits_builder_lifecycle_logs": Owner(
        "trace-call-binding",
        "trace_callargs_child",
        {"trace_callargs_child": Outcome()},
        stderr_markers=(
            "[molt callargs] new",
            "[molt callargs] push_pos",
            "[molt callargs] free",
        ),
    ),
    "trace_call_bind_ic_emits_hit_log": Owner(
        "trace-call-binding",
        "trace_call_bind_ic_child",
        {"trace_call_bind_ic_child": Outcome()},
        stderr_markers=("[molt call_bind_ic] hit",),
    ),
    "trace_function_bind_meta_emits_summary": Owner(
        "trace-call-binding",
        "trace_function_bind_meta_child",
        {"trace_function_bind_meta_child": Outcome()},
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


def retain_file(source: Path, root: Path) -> dict[str, object]:
    """Retain observed bytes as evidence, never as an execution-path substitute."""
    source = source.resolve(strict=True)
    expected = _file_identity(source)
    reference = custody_cas.put_file(
        root / "custody-cas", source, logical_name="evidence"
    )
    if (
        _file_identity(source) != expected
        or (reference.size_bytes, reference.sha256) != expected
    ):
        raise DescendantEvidenceError("file changed during evidence retention")
    return reference.as_dict()


def retained_file(reference: object, *, root: Path) -> Path:
    if not isinstance(reference, dict) or reference.get("executable") is not False:
        raise DescendantEvidenceError(
            "retained bytes are evidence, not executable custody"
        )
    custody_cas.verify_file_ref(reference, expected_root=root / "custody-cas")
    return Path(reference["path"])


def receipt_file(receipt: Mapping[str, object], source: Path) -> Path:
    root = _canonical(receipt.get("retained_root"))
    custody = _canonical(receipt.get("receipt_custody_root"))
    if root == Path(root.anchor) or not custody.is_relative_to(root):
        raise DescendantEvidenceError("retained root escaped run custody")
    rows = receipt.get("retained_files")
    if not isinstance(rows, dict) or str(source) not in rows:
        raise DescendantEvidenceError("binary receipt lacks retained source binding")
    return retained_file(rows[str(source)], root=root)


def retain_receipt_files(receipt: dict, *, root: Path) -> None:
    """Publish the executed image and exact typed descendant stream evidence."""
    root = root.resolve()
    custody = _canonical(receipt["receipt_custody_root"])
    if root == Path(root.anchor) or not custody.is_relative_to(root):
        raise DescendantEvidenceError("retained root escaped run custody")
    image = _canonical(receipt["executable_resolved"])
    expected = (receipt["executable_size"], receipt["executable_sha256"])
    if _file_identity(image) != expected:
        raise DescendantEvidenceError("executed image changed before retention")
    sources = {image}
    if _engaged(receipt):
        for _rows, records in _parent_evidence(receipt, image, custody):
            for record in records:
                for name in ("stdout", "stderr"):
                    value = record.get(name)
                    # Check original ownership and bytes before copy; the
                    # receiver repeats this against the immutable copy.
                    _descendant_stream(value, image=image, name=name)
                    path = _canonical(value["path"])
                    sources.update((path, path.parent / "artifact-label.txt"))
    receipt["retained_root"] = str(root)
    receipt["retained_files"] = {
        str(path): retain_file(path, root) for path in sorted(sources)
    }
    if (
        _file_identity(image) != expected
        or _file_identity(receipt_file(receipt, image)) != expected
    ):
        raise DescendantEvidenceError("executed image retention differs from admission")


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
    selected = (
        executions
        if receipt.get("resource_process_isolation") is True
        else executions[:1]
    )
    for execution in selected:
        if not isinstance(execution, dict):
            continue
        argv = execution.get("argv")
        for stream in ("stdout", "stderr"):
            value = execution.get(f"{stream}_evidence")
            if not isinstance(value, str) or not value or not Path(value).is_file():
                continue
            try:
                if _records(Path(value)):
                    return True
                if stream == "stdout" and isinstance(argv, list):
                    with Path(value).open(
                        "r", encoding="utf-8", errors="replace"
                    ) as handle:
                        if "--list" in argv:
                            if any(
                                name in OWNERS
                                for name in listed_test_identities(handle)
                            ):
                                return True
                        else:
                            report = parse_libtest(
                                handle, tuple(str(item) for item in argv)
                            )
                            if any(row["identity"] in OWNERS for row in report.rows()):
                                return True
            except (OSError, ValueError, DescendantEvidenceError):
                # Malformed owner-shaped evidence requires full verification.
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


def _parent_evidence(
    receipt: Mapping[str, object], image: Path, custody_root: Path
) -> list[tuple[list[dict[str, str]], list[dict[str, object]]]]:
    """Re-derive actual executions, keeping each child's parent scope intact."""
    isolated = receipt.get("resource_process_isolation")
    executions = receipt.get("executions")
    if type(isolated) is not bool or not isinstance(executions, list) or not executions:
        raise DescendantEvidenceError("runtime descendants lack parent executions")
    selected = executions if isolated else executions[:1]
    groups = []
    all_rows = []
    discovered = []
    captures: set[Path] = set()
    for index, execution in enumerate(selected):
        if not isinstance(execution, dict):
            raise DescendantEvidenceError("invalid parent execution")
        argv = execution.get("argv")
        if (
            not isinstance(argv, list)
            or not argv
            or not all(isinstance(arg, str) for arg in argv)
        ):
            raise DescendantEvidenceError("parent execution lacks exact argv")
        if not _same_path(_canonical(argv[0]), image):
            raise DescendantEvidenceError(
                "parent execution did not run the receipt image"
            )
        stdout = _parent_capture(receipt, execution, "stdout", custody_root)
        stderr = _parent_capture(receipt, execution, "stderr", custody_root)
        if stdout in captures or stderr in captures:
            raise DescendantEvidenceError("parent executions share a capture")
        captures.update((stdout, stderr))
        if _records(stdout):
            raise DescendantEvidenceError(
                "runtime descendant record escaped its canonical stderr stream"
            )
        records = _records(stderr)
        if isolated and index == 0:
            if argv.count("--list") != 1 or records:
                raise DescendantEvidenceError(
                    "resource discovery is not descendant execution evidence"
                )
            if (
                execution.get("infrastructure_failure") is not None
                or execution.get("timed_out") is not False
                or not _exact(
                    execution.get("termination"), {"kind": "exit", "returncode": 0}
                )
            ):
                raise DescendantEvidenceError(
                    "resource discovery did not complete successfully"
                )
            with stdout.open("r", encoding="utf-8", errors="replace") as handle:
                try:
                    discovered = listed_test_identities(handle)
                except ValueError as exc:
                    raise DescendantEvidenceError(str(exc)) from exc
            continue
        with stdout.open("r", encoding="utf-8", errors="replace") as handle:
            report = parse_libtest(handle, tuple(argv))
        if isolated:
            if (
                "--list" in argv
                or argv.count("--exact") != 1
                or argv.index("--exact") + 1 >= len(argv)
                or index > len(discovered)
                or argv[argv.index("--exact") + 1] != discovered[index - 1]
                or not report.serial
            ):
                raise DescendantEvidenceError(
                    "isolated parent escaped its exact discovered test"
                )
            termination = execution.get("termination")
            rows = isolated_test_rows(
                report,
                discovered[index - 1],
                infrastructure_failure=execution.get("infrastructure_failure"),
                timed_out=execution.get("timed_out") is not False,
                termination_kind=termination.get("kind")
                if isinstance(termination, dict)
                else None,
            )
        else:
            rows = (
                []
                if execution.get("infrastructure_failure") is not None
                else report.rows()
            )
        if receipt.get("status") == "success" and (
            not report.complete
            or (
                isolated
                and (
                    len(rows) != 1
                    or rows[0]["status"] not in {"pass", "ignored"}
                    or execution.get("timed_out") is not False
                    or not _exact(
                        execution.get("termination"), {"kind": "exit", "returncode": 0}
                    )
                )
            )
        ):
            raise DescendantEvidenceError(
                "successful parent lacks complete re-derived libtest accounting"
            )
        all_rows.extend(rows)
        groups.append((rows, records))
    if (
        isolated
        and receipt.get("status") == "success"
        and len(groups) != len(discovered)
    ):
        raise DescendantEvidenceError(
            "successful resource receipt omitted discovered executions"
        )
    if all_rows != receipt.get("test_results"):
        raise DescendantEvidenceError(
            "re-derived parent libtest rows disagree with the saved ledger"
        )
    return groups


def _descendant_stream(
    value: object,
    *,
    image: Path,
    name: str,
    receipt: Mapping[str, object] | None = None,
) -> tuple[Path, str]:
    if not isinstance(value, dict) or set(value) != {"path", "bytes", "sha256"}:
        raise DescendantEvidenceError(f"unbound runtime descendant {name} stream")
    path = _canonical(value["path"])
    owner = path.parent
    root = (image.parent / "molt-test-artifacts").resolve()
    if path.name != f"{name}.log" or not _same_path(owner.parent, root):
        raise DescendantEvidenceError(
            f"runtime descendant {name} stream escaped Cargo test-image custody"
        )
    label = owner / "artifact-label.txt"
    retained = path if receipt is None else receipt_file(receipt, path)
    label = label if receipt is None else receipt_file(receipt, label)
    if label.read_bytes() != STREAM_LABEL:
        raise DescendantEvidenceError(
            f"runtime descendant {name} stream owner is not a descendant capture"
        )
    raw = retained.read_bytes()
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
    receipt: Mapping[str, object],
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
    outcome = owner.modes[record["mode"]]
    expected = (
        _ABORT.get(platform)
        if outcome.exit_code is None
        else {"kind": "exit", "code": outcome.exit_code}
    )
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
        record["stdout"], image=image, name="stdout", receipt=receipt
    )
    stderr_owner, stderr = _descendant_stream(
        record["stderr"], image=image, name="stderr", receipt=receipt
    )
    if not _same_path(stdout_owner, stderr_owner):
        raise DescendantEvidenceError(f"{parent}: descendant streams have two owners")
    report = parse_libtest(StringIO(stdout), tuple(argv))
    if outcome.completes:
        if not report.complete or report.rows() != [
            {"identity": owner.child, "status": "pass"}
        ]:
            raise DescendantEvidenceError(
                f"{parent} ({record['mode']}): descendant lacks exact one-test completion; "
                f"issues={report.issues!r}, rows={report.rows()!r}, pending={report.pending!r}"
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
        (owner.stdout_markers + outcome.stdout_markers, stdout, "stdout"),
        (owner.stderr_markers + outcome.stderr_markers, stderr, "stderr"),
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
    image = _canonical(receipt.get("executable_resolved"))
    image_sha256 = receipt.get("executable_sha256")
    image_size = receipt.get("executable_size")
    if type(image_size) is not int or _file_identity(receipt_file(receipt, image)) != (
        image_size,
        image_sha256,
    ):
        raise DescendantEvidenceError("parent test image changed after publication")
    custody_root = _canonical(receipt.get("receipt_custody_root"))
    if receipt_root is not None and not _same_path(
        custody_root, Path(receipt_root).resolve()
    ):
        raise DescendantEvidenceError("receipt escaped its loader's custody root")
    groups = _parent_evidence(receipt, image, custody_root)
    source = receipt.get("source_identity")
    required: set[tuple[str, str]] = set()
    observed: set[tuple[str, str]] = set()
    owners: set[str] = set()
    for rows, records in groups:
        # A record belongs to the execution that emitted it, never another
        # isolated test with a passing row in the aggregate ledger.
        completed = {
            row["identity"]: row["status"]
            for row in rows
            if row["identity"] in OWNERS and row["status"] in {"pass", "fail"}
        }
        required.update(
            (parent, mode)
            for parent, status in completed.items()
            if status == "pass"
            for mode in OWNERS[parent].modes
        )
        if (required or records) and not isinstance(source, dict):
            raise DescendantEvidenceError(
                "runtime descendant source identity is not admitted"
            )
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
                receipt=receipt,
            )
            if owner in owners:
                raise DescendantEvidenceError(
                    "runtime descendant streams share one owner"
                )
            owners.add(owner)
    missing = sorted(required - observed)
    if missing:
        raise DescendantEvidenceError(
            f"missing mandatory runtime descendant records: {missing!r}"
        )
    return {
        "status": "verified",
        "children": len(observed),
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
