"""CI triage reads the failure shapes our CI jobs actually print.

Each fixture is a log written in the exact form one runner job emits, with a
known set of failures; the parser must return that set and nothing from the
passing or expected-panic noise around it.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]


def _load():
    spec = importlib.util.spec_from_file_location(
        "molt_ci_triage", REPO_ROOT / "tools" / "ci_triage.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


ci_triage = _load()

STAMP = "2026-10-14T13:16:01.8687323Z "

PYTEST_XDIST_LOG = "\n".join(
    STAMP + line
    for line in [
        "\x1b[32m[gw0] [ 12%] PASSED tests/tools/test_queue.py::test_ok\x1b[0m",
        "[gw1] [ 88%] FAILED tests/tools/test_queue.py::test_receipt[0-False] ",
        "= Molt immediate pytest failure [gw1]: tests/tools/test_queue.py::test_receipt[0-False] (call) =",
        "    def test_receipt():",
        ">       assert rc == 0",
        "E       assert 2 == 0",
        "E       second line is ignored",
        "FAILED tests/tools/test_other.py::test_named - AssertionError: wrong shape",
        "FAILED tests/tools/test_other.py::test_named - AssertionError: wrong shape",
        "proof-plan: failed portability.queue.windows: command failed",
    ]
)

LIBTEST_LOG = "\n".join(
    STAMP + line
    for line in [
        # An expected panic in a passing #[should_panic] test is noise.
        "thread 'tir::tests::rejects_bad_shape' (26829) panicked at src/lower.rs:79:9:",
        "unregistered op",
        "test tir::tests::rejects_bad_shape - should panic ... ok",
        "thread 'call::tests::kwdefaults_need_binder' (35238) panicked at src/ic_tests.rs:534:9:",
        "non-empty __kwdefaults__ => binder",
        "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
        "test call::tests::kwdefaults_need_binder ... FAILED",
        "",
        "failures:",
        "",
        "failures:",
        "    call::tests::kwdefaults_need_binder",
        "",
        "test result: FAILED. 830 passed; 1 failed; 2 ignored",
        "error: test failed, to rerun pass `-p molt-runtime --lib`",
        "error: 1 target failed:",
        "runtime-gate: child parallel failed with exit code 1",
        "  1 failing test(s):",
        "    http_bridge::tests::header_owner",
        "  driver stderr (last 17 lines):",
        "proof-plan: failed rust.test.default-truth: command failed",
    ]
)

CARGO_DENY_LOG = "\n".join(
    STAMP + line
    for line in [
        "error[vulnerability]: `Debug` impls for client session values expose TLS secrets",
        "    ┌─ Cargo.lock:400:1",
        "error[vulnerability]: `Debug` impls for client session values expose TLS secrets",
        "error[E0308]: mismatched types",
        "proof-plan: failed security.rust.deny: command failed",
    ]
)


def test_pytest_failures_keep_one_entry_and_the_first_assertion() -> None:
    report = ci_triage.parse_job_log("Windows", PYTEST_XDIST_LOG)

    assert report.tests == {
        "tests/tools/test_queue.py::test_receipt[0-False]": "assert 2 == 0",
        "tests/tools/test_other.py::test_named": "AssertionError: wrong shape",
    }
    assert report.proof_commands == ["portability.queue.windows: command failed"]
    assert report.errors == []


def test_libtest_failures_take_their_own_panic_and_skip_expected_panics() -> None:
    report = ci_triage.parse_job_log("Rust", LIBTEST_LOG)

    assert report.tests == {
        "call::tests::kwdefaults_need_binder": (
            "src/ic_tests.rs:534:9 non-empty __kwdefaults__ => binder"
        ),
        "http_bridge::tests::header_owner": "",
    }
    assert report.proof_commands == ["rust.test.default-truth: command failed"]
    # Summary lines that only restate a test failure are not errors.
    assert report.errors == []


def test_cargo_errors_are_unique_and_keep_their_code() -> None:
    report = ci_triage.parse_job_log("Security", CARGO_DENY_LOG)

    assert report.errors == [
        "error[vulnerability]: `Debug` impls for client session values expose TLS secrets",
        "error[E0308]: mismatched types",
    ]
    assert report.tests == {}


LEDGER = """
## Open: CI, harness and release engineering

| ID | Finding | Evidence | Next step |
|----|---------|----------|-----------|
| HF-169 | Windows `test_receipt` failed on every train. | x | y |
| HF-12 | The runtime gate failed `kwdefaults_need_binder`. | x | y |

## Fixed after the handoff

| ID | Finding | Evidence | Fix |
|----|---------|----------|-----|
| HF-F1 | `test_named` once failed. | x | y |
"""


def test_failures_are_new_unless_main_fails_them_in_the_same_job() -> None:
    windows = ci_triage.parse_job_log("Windows", PYTEST_XDIST_LOG)
    rust = ci_triage.parse_job_log("Rust", LIBTEST_LOG)
    main_windows = ci_triage.parse_job_log(
        "Windows",
        STAMP + "[gw2] [ 88%] FAILED tests/tools/test_queue.py::test_receipt[0-False] ",
    )
    # The same test failing under another job name is not the same failure.
    main_other = ci_triage.parse_job_log(
        "Linux", STAMP + "FAILED tests/tools/test_other.py::test_named - boom"
    )
    items = ci_triage.classify(
        [windows, rust],
        [main_windows, main_other],
        ci_triage.open_ledger_rows(LEDGER),
    )
    by_key = {item.key: item for item in items}

    assert by_key["tests/tools/test_queue.py::test_receipt[0-False]"].new is False
    assert by_key["tests/tools/test_other.py::test_named"].new is True
    assert by_key["call::tests::kwdefaults_need_binder"].new is True
    # Owners come from open rows only; a fixed row names no owner.
    assert by_key["tests/tools/test_queue.py::test_receipt[0-False]"].owners == [
        "HF-169"
    ]
    assert by_key["call::tests::kwdefaults_need_binder"].owners == ["HF-12"]
    assert by_key["tests/tools/test_other.py::test_named"].owners == []


def test_without_a_baseline_nothing_is_new_and_the_report_stays_compact() -> None:
    rust = ci_triage.parse_job_log("Rust", LIBTEST_LOG)
    items = ci_triage.classify([rust], None, [])
    assert {item.new for item in items} == {None}

    text = ci_triage.render(
        {"databaseId": 1, "headBranch": "verify/train-32", "headSha": "69870524f00"},
        items,
        ["WASM Validation / wasm-build"],
    )
    assert text.splitlines() == [
        "run 1 verify/train-32 69870524f",
        "  still running: WASM Validation / wasm-build",
        "[Rust]",
        "  proof: rust.test.default-truth: command failed",
        "  test: call::tests::kwdefaults_need_binder -- "
        "src/ic_tests.rs:534:9 non-empty __kwdefaults__ => binder",
        "  test: http_bridge::tests::header_owner",
    ]
