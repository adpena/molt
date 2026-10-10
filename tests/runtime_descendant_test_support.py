"""Real-file runtime descendant receipts for consumer tests.

Builds what the Cargo binary runner and the shared Rust producer
``runtime/test_support/captured_runtime_children.rs`` publish: a test image,
the parent's full stdout/stderr captures under receipt custody, and descendant
streams in Cargo test-image custody. The expected family contract below is
written from the owning Rust tests, not derived from the validator under test.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
from typing import Any

PREFIX = "MOLT_RUNTIME_DESCENDANT_RECEIPT "
SOURCE = {"schema": "molt.git-source.v1", "head": "fixture-head", "tree": "fixture"}
EXIT = (
    "state::lifecycle::shutdown_tests::"
    "process_exit_covers_collection_before_pending_callbacks_with_or_without_lease"
)
TRAP = "call::function::tests::assert_no_pending_on_success_traps_stale_exception"
TRAP_CHILD = "call::function::tests::assert_no_pending_on_success_child"
COLD = (
    "wasm_abi_exports::tests::scratch_alloc_cold_resource_denial_is_null_and_nounwind"
)
TRANSACTION = "test_support::runtime_test_transactions_preserve_terminal_failures"
LIFECYCLE = (
    "state::runtime_state::tests::lifecycle_ffi_panics_fail_closed_without_unwinding"
)
DISCOVERY = "tests::pending_diagnostic_prints_and_retires_original_error_after_summary_encoding_failure"
DISCOVERY_CHILD = "tests::pending_diagnostic_child"
TRACE_CALLARGS = "trace_callargs_emits_builder_lifecycle_logs"
TRACE_BIND_IC = "trace_call_bind_ic_emits_hit_log"
TRACE_BIND_META = "trace_function_bind_meta_emits_summary"
# std::process::abort as each host reports it: SIGABRT, or Windows fast-fail.
ABORT = {
    "posix": {"kind": "signal", "signal": 6},
    "nt": {"kind": "windows-exception", "code": 0xC0000409},
}
SERIAL = ["--nocapture", "--test-threads=1"]

# (owner, mode) -> role, exact child argv after the image, child stdout/stderr
# and the exact exit or abort plus completion. Independent Rust call-site oracle.
CONTRACT: dict[tuple[str, str], dict[str, Any]] = {
    (EXIT, mode): {
        "role": "process-exit-callbacks",
        "child": EXIT,
        "args": ["--exact", EXIT, "--ignored", *SERIAL],
        "stdout": (
            f"\nrunning 1 test\ntest {EXIT} ... "
            f"shutdown callbacks verified before process exit: {mode}\n"
        ),
        "stderr": "",
        "exit_code": 0,
        "completes": False,
    }
    for mode in ("no-lease", "lease")
}
CONTRACT[(TRAP, "stale-exception")] = {
    "role": "pending-success-trap",
    "child": TRAP_CHILD,
    "args": ["--exact", TRAP_CHILD, *SERIAL],
    "stdout": f"\nrunning 1 test\ntest {TRAP_CHILD} ... ",
    "stderr": "pending exception on success path: call_function_obj0 result=0x7\n",
    "exit_code": None,
    "completes": False,
}
CONTRACT[(COLD, "cold")] = {
    "role": "cold-resource-denial",
    "child": COLD,
    "args": ["--exact", COLD, *SERIAL],
    "stdout": (
        f"\nrunning 1 test\ntest {COLD} ... ok\n\ntest result: ok. 1 passed; "
        "0 failed; 0 ignored; 0 measured; 41 filtered out; finished in 0.00s\n\n"
    ),
    "stderr": "",
    "exit_code": 0,
    "completes": True,
}
for _owner, _child, _stderr in (
    (
        TRACE_CALLARGS,
        "trace_callargs_child",
        "[molt callargs] new cap=1\n[molt callargs] push_pos\n[molt callargs] free\n",
    ),
    (TRACE_BIND_IC, "trace_call_bind_ic_child", "[molt call_bind_ic] hit site=1\n"),
    (
        TRACE_BIND_META,
        "trace_function_bind_meta_child",
        "[molt bind_meta] total_pos=0 kwonly=1\n",
    ),
):
    CONTRACT[(_owner, _child)] = {
        "role": "trace-call-binding",
        "child": _child,
        "args": ["--exact", _child, *SERIAL],
        "stdout": f"\nrunning 1 test\ntest {_child} ... ",
        "stderr": _stderr,
        "exit_code": 0,
        "completes": False,
    }
for _mode in (
    "prior",
    "cleanup",
    "both",
    "cold-both",
    "body-only",
    "ordinary",
    "ordinary-return",
    "reentry",
    "healthy",
):
    CONTRACT[(TRANSACTION, _mode)] = {
        "role": "runtime-test-transaction",
        "child": TRANSACTION,
        "args": ["--exact", TRANSACTION, *SERIAL],
        "stdout": (
            f"\nrunning 1 test\ntest {TRANSACTION} ... "
            f"transaction outcome and custody verified: {_mode}\n"
            "ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; "
            "0 measured; 41 filtered out; finished in 0.00s\n\n"
        ),
        "stderr": (
            "molt runtime lifecycle failed: native owners survived the last callback drain before class retirement\n"
            if _mode in {"ordinary", "ordinary-return"}
            else "molt runtime lifecycle failed: injected shutdown drain C extension cleanup panic\n"
            if _mode not in {"body-only", "healthy"}
            else ""
        ),
        "exit_code": 0,
        "completes": True,
    }
for _mode, _exit, _cause in (
    ("init", 0, "injected unpublished runtime init panic"),
    ("shutdown", 0, "injected shutdown drain C extension cleanup panic"),
    ("exit", 1, "injected shutdown drain C extension cleanup panic"),
):
    CONTRACT[(LIFECYCLE, _mode)] = {
        "role": "lifecycle-ffi",
        "child": LIFECYCLE,
        "args": ["--exact", LIFECYCLE, *SERIAL],
        "stdout": f"\nrunning 1 test\ntest {LIFECYCLE} ... "
        + (
            "ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; "
            "0 measured; 41 filtered out; finished in 0.00s\n\n"
            if _mode != "exit"
            else ""
        ),
        "stderr": f"molt runtime lifecycle failed: {_cause}\n",
        "exit_code": _exit,
        "completes": _mode != "exit",
    }
CONTRACT[(DISCOVERY, "render-and-drain")] = {
    "role": "discovery-pending-diagnostic",
    "child": DISCOVERY_CHILD,
    "args": ["--exact", DISCOVERY_CHILD, "--ignored", *SERIAL],
    "stdout": (
        f"\nrunning 1 test\ntest {DISCOVERY_CHILD} ... "
        "discovery diagnostic final drain verified\nok\n\ntest result: ok. "
        "1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s\n\n"
    ),
    "stderr": (
        '===MOLT_DISCOVERY_EXC: pending exception value = "discovery exception custody"\n'
        "[molt-cpython-abi] PyErr_Print: discovery exception custody\n"
        "[molt-cpython-abi] PyErr_Print: \\ud800\n"
        "===MOLT_DISCOVERY_EXC: no pending exception on NULL return\n"
    ),
    "exit_code": 0,
    "completes": True,
}
MODES: dict[str, list[str]] = {}
for _owner, _mode in CONTRACT:
    MODES.setdefault(_owner, []).append(_mode)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def publish(path: Path, text: str) -> dict[str, object]:
    path.parent.mkdir(parents=True, exist_ok=True)
    data = text.encode("utf-8")
    path.write_bytes(data)
    return {"path": str(path), "bytes": len(data), "sha256": sha256(data)}


def make_image(
    directory: Path, name: str = "molt_runtime-0123456789abcdef.exe"
) -> Path:
    image = directory / "target" / "deps" / name
    image.parent.mkdir(parents=True, exist_ok=True)
    image.write_bytes(b"fixture runtime test image " + name.encode())
    return image


def descendant_record(
    image: Path,
    owner: str,
    mode: str,
    *,
    platform: str | None = None,
    source: object = SOURCE,
) -> dict[str, object]:
    contract = CONTRACT[(owner, mode)]
    root = image.parent / "molt-test-artifacts"
    index = sum(1 for _ in root.glob("*")) if root.is_dir() else 0
    directory = root / f"fixture-{index:x}"
    directory.mkdir(parents=True)
    (directory / "artifact-label.txt").write_bytes(b"runtime-descendant")
    termination = (
        ABORT[os.name if platform is None else platform]
        if contract["exit_code"] is None
        else {"kind": "exit", "code": contract["exit_code"]}
    )
    return {
        "schema": "molt.runtime-descendant.v1",
        "role": contract["role"],
        "parent_test": owner,
        "child_test": contract["child"],
        "mode": mode,
        "source_identity": source,
        "executable": str(image),
        "executable_sha256": sha256(image.read_bytes()),
        "argv": [str(image), *contract["args"]],
        "termination": termination,
        "coordinate_authority": "unavailable",
        "stdout": publish(directory / "stdout.log", contract["stdout"]),
        "stderr": publish(directory / "stderr.log", contract["stderr"]),
    }


def family_records(image: Path, owners: list[str], **options: Any) -> list[dict]:
    return [
        descendant_record(image, owner, mode, **options)
        for owner in owners
        for mode in MODES[owner]
    ]


def records_text(records: list[dict]) -> str:
    return "".join(f"\n{PREFIX}{json.dumps(record)}\n" for record in records)


def transcript(names: list[str], *, statuses: dict[str, str] | None = None) -> str:
    """Parallel libtest pretty output with inline results and a final summary."""
    statuses = statuses or {}
    words = {"pass": "ok", "fail": "FAILED", "ignored": "ignored"}
    rows = "".join(
        f"test {name} ... {words[statuses.get(name, 'pass')]}\n" for name in names
    )
    counts = {
        status: sum(statuses.get(name, "pass") == status for name in names)
        for status in words
    }
    outcome = "FAILED" if counts["fail"] else "ok"
    return (
        f"\nrunning {len(names)} tests\n{rows}\ntest result: {outcome}. "
        f"{counts['pass']} passed; {counts['fail']} failed; "
        f"{counts['ignored']} ignored; 0 measured; 0 filtered out; "
        "finished in 0.01s\n\n"
    )


def binary_receipt(
    root: Path,
    image: Path,
    tests: list[str],
    records: list[dict],
    *,
    args: list[str] | None = None,
    statuses: dict[str, str] | None = None,
    source: object = SOURCE,
    run_id: str = "run",
    invocation: str = "invocation",
    stdout_text: str | None = None,
    stderr_text: str | None = None,
) -> dict[str, object]:
    """A runner-shaped v2 receipt whose evidence is real files under ``root``."""
    args = ["--test-threads=4", "--nocapture"] if args is None else args
    statuses = statuses or {}
    evidence = root / "evidence" / invocation
    stdout = publish(
        evidence / "baseline.stdout.log",
        transcript(tests, statuses=statuses) if stdout_text is None else stdout_text,
    )
    stderr = publish(
        evidence / "baseline.stderr.log",
        records_text(records) if stderr_text is None else stderr_text,
    )
    rows = [{"identity": name, "status": statuses.get(name, "pass")} for name in tests]
    failed = [row["identity"] for row in rows if row["status"] == "fail"]
    image_bytes = image.read_bytes()
    return {
        "schema": "molt.cargo-test-binary.v2",
        "receipt_custody_root": str(root),
        "run_id": run_id,
        "source_identity": source,
        "invocation_id": invocation,
        "executable": str(image),
        "executable_resolved": str(image.resolve()),
        "executable_size": len(image_bytes),
        "executable_sha256": sha256(image_bytes),
        "inherited_args": args,
        "resource_process_isolation": False,
        "status": "failed" if failed else "success",
        "returncode": 1 if failed else 0,
        "reported_failures": failed,
        "failure_identities": failed,
        "test_results": rows,
        "result_accounting": {
            "schema": "molt.libtest-accounting.v1",
            "complete": True,
            "observed_results": len(rows),
            "declared_results": len(rows),
            "issues": [],
        },
        "diagnosis": None,
        "executions": [
            {
                "argv": [str(image), *args],
                "infrastructure_failure": None,
                "stdout_evidence": stdout["path"],
                "stderr_evidence": stderr["path"],
                "stdout_bytes": stdout["bytes"],
                "stderr_bytes": stderr["bytes"],
                "stdout_sha256": stdout["sha256"],
                "stderr_sha256": stderr["sha256"],
            }
        ],
    }


def republish(receipt: dict, stream: str, text: str) -> None:
    """Replace one parent capture and keep the receipt's hashes consistent."""
    execution = receipt["executions"][0]
    path = Path(execution[f"{stream}_evidence"])
    data = text.encode("utf-8")
    path.write_bytes(data)
    execution[f"{stream}_bytes"] = len(data)
    execution[f"{stream}_sha256"] = sha256(data)
