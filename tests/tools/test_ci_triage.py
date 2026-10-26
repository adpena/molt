"""CI triage reads the failure shapes our CI jobs actually print.

Each fixture is a log written in the exact form one runner job emits, with a
known set of failures; the parser must return that set and nothing from the
passing or expected-panic noise around it.
"""

from __future__ import annotations

import hashlib
import io
import json
from pathlib import Path
import sys
import zipfile

import pytest

from tools import ci_triage
from tools import ci_proof_evidence


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


def _ci_run(**changes: object) -> dict:
    return {
        "databaseId": 41,
        "headSha": "a" * 40,
        "headBranch": "main",
        "status": "completed",
        "conclusion": "success",
        "attempt": 1,
        "event": "push",
        "jobs": [],
        **changes,
    }


def _artifact(identity: int, name: str, payload: bytes = b"") -> dict:
    return {
        "id": identity,
        "name": name,
        "expired": False,
        "digest": "sha256:" + hashlib.sha256(payload).hexdigest(),
        "workflow_run": {"id": 41, "head_sha": "a" * 40},
    }


def _zip(payload: dict[str, bytes]) -> bytes:
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, content in payload.items():
            archive.writestr(name, content)
    return stream.getvalue()


def test_ci_artifacts_keep_pagination_and_ignore_diagnostics() -> None:
    selection = _artifact(1, "proof-selection-1")
    receipts = [_artifact(2, "proof-receipt-linux"), _artifact(3, "proof-receipt-mac")]
    actual = ci_proof_evidence._selected_artifacts(
        [
            {"artifacts": [selection, receipts[0]]},
            {"artifacts": [receipts[1], {"name": "proof-diagnostics-linux"}]},
        ],
        _ci_run(),
    )
    assert actual == (selection, tuple(receipts))


@pytest.mark.parametrize(
    "defect",
    [
        "wrong-run",
        "wrong-source",
        "expired",
        "missing-digest",
        "duplicate-id",
        "duplicate-name",
        "prior-selection-attempt",
        "missing-selection",
    ],
)
def test_ci_artifact_custody_refuses_incomplete_or_ambiguous_evidence(
    defect: str,
) -> None:
    selection = _artifact(1, "proof-selection-1")
    receipt = _artifact(2, "proof-receipt-linux")
    artifacts = [selection, receipt]
    if defect == "wrong-run":
        receipt["workflow_run"]["id"] = 42
    elif defect == "wrong-source":
        receipt["workflow_run"]["head_sha"] = "b" * 40
    elif defect == "expired":
        receipt["expired"] = True
    elif defect == "missing-digest":
        del receipt["digest"]
    elif defect == "duplicate-id":
        artifacts.append(_artifact(2, "proof-receipt-other"))
    elif defect == "duplicate-name":
        artifacts.append(_artifact(3, "proof-receipt-linux"))
    elif defect == "prior-selection-attempt":
        selection["name"] = "proof-selection-0"
    else:
        artifacts.pop(0)
    with pytest.raises(ValueError):
        ci_proof_evidence._selected_artifacts([{"artifacts": artifacts}], _ci_run())


@pytest.mark.parametrize(
    ("status", "conclusion", "expected"),
    [
        ("in_progress", None, "pending"),
        ("completed", "failure", "failed"),
        ("completed", "cancelled", "failed"),
        ("completed", "skipped", "failed"),
    ],
)
def test_ci_verification_never_retrieves_failed_or_pending_run(
    status: str,
    conclusion: str | None,
    expected: str,
) -> None:
    result = ci_proof_evidence.verify_run(
        _ci_run(status=status, conclusion=conclusion),
        repo_root=Path("unused"),
        commands=None,
        read_run=lambda _: pytest.fail("unexpected observation"),
    )
    assert result.status == expected
    assert result.evidence_dir is None


@pytest.mark.parametrize(
    ("status", "conclusion", "expected"),
    [
        ("in_progress", None, 3),
        ("completed", "failure", 1),
        ("completed", "cancelled", 1),
        ("completed", "success", 0),
    ],
)
def test_triage_exit_status_reports_run_outcome_even_without_new_failures(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture,
    status: str,
    conclusion: str | None,
    expected: int,
) -> None:
    monkeypatch.setattr(
        ci_triage, "_run", lambda _: _ci_run(status=status, conclusion=conclusion)
    )
    monkeypatch.setattr(ci_triage, "_failed_reports", lambda _: ([], []))
    assert ci_triage.main(["41", "--compare", "none", "--json"]) == expected
    payload = json.loads(capsys.readouterr().out)
    assert payload["run"]["status"] == status
    assert payload["run"]["conclusion"] == conclusion
    assert payload["verification"] is None


@pytest.mark.parametrize(
    "defect", [None, "bytes", "unsafe-member", "run-changed", "proof-failed"]
)
def test_ci_transport_uses_consumed_archive_and_canonical_verifier(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    defect: str | None,
) -> None:
    from tools import proof_plan, proof_plan_cli

    selected_zip = _zip({"selection.json": b'{"source_commit":"' + b"a" * 40 + b'"}'})
    receipt_zip = _zip(
        {"../outside": b"unsafe"}
        if defect == "unsafe-member"
        else {
            "proof.json": b"retained exact receipt bytes",
        }
    )
    artifacts = [
        _artifact(1, "proof-selection-1", selected_zip),
        _artifact(2, "proof-receipt-linux", receipt_zip),
    ]
    if defect == "bytes":
        artifacts[1]["digest"] = "sha256:" + "0" * 64
    monkeypatch.setattr(
        proof_plan, "_source_identity", lambda: {"commit": "a" * 40, "tree": "b" * 40}
    )
    monkeypatch.setattr(proof_plan, "_source_tree_changes", lambda: ())
    monkeypatch.setattr(proof_plan.ProofPlan, "load", staticmethod(lambda _: object()))
    monkeypatch.setattr(
        proof_plan_cli, "read_selection_file", lambda *args, **kwargs: None
    )
    monkeypatch.setattr(ci_proof_evidence, "scratch_dir", lambda *args: tmp_path)
    calls = []

    def verify(api, plan, selection, receipt_root, *, tier, github_context):
        assert selection.read_bytes() == b'{"source_commit":"' + b"a" * 40 + b'"}'
        assert (
            receipt_root / "2" / "proof.json"
        ).read_bytes() == b"retained exact receipt bytes"
        assert tier == "main"
        assert github_context == {"run_id": 41, "run_attempt": 1, "event_name": "push"}
        calls.append(selection)
        return ["required proof did not pass"] if defect == "proof-failed" else []

    monkeypatch.setattr(proof_plan_cli, "verify_selection_file", verify, raising=False)

    class Commands:
        def check_output(self, argv, **kwargs):
            assert argv[:5] == [
                "gh",
                "api",
                "--paginate",
                "--slurp",
                "repos/{owner}/{repo}/actions/runs/41/artifacts?per_page=100",
            ]
            return json.dumps([{"artifacts": artifacts}])

        def run(self, argv, **kwargs):
            assert argv[:2] == ["gh", "api"]
            assert kwargs["capture_output"] is True
            assert (
                kwargs["stderr_capture_path"].parent
                == kwargs["stdout_capture_path"].parent
            )
            assert kwargs["capture_tail_bytes"] == 64 * 1024
            assert kwargs["text"] is False
            identity = int(argv[-1].split("/")[-2])
            if defect is None and identity == 1:
                # Exercise the real guarded binary capture boundary. GitHub is
                # the substituted peer; launch/capture/closure remain real.
                from tools.command_execution import CommandExecutor

                return CommandExecutor.for_file(__file__).run(
                    [
                        sys.executable,
                        "-c",
                        "import sys; sys.stdout.buffer.write(bytes.fromhex("
                        + repr(selected_zip.hex())
                        + "))",
                    ],
                    **kwargs,
                )
            kwargs["stdout_capture_path"].write_bytes(
                {1: selected_zip, 2: receipt_zip}[identity]
            )

    result = ci_proof_evidence.verify_run(
        _ci_run(),
        repo_root=tmp_path,
        commands=Commands(),
        read_run=lambda _: _ci_run(attempt=2) if defect == "run-changed" else _ci_run(),
    )
    assert result.status == (
        "verified"
        if defect is None
        else "failed"
        if defect == "proof-failed"
        else "unavailable"
    )
    assert len(calls) == (0 if defect in {"bytes", "unsafe-member"} else 1)
    # Failure retains the original ZIPs and inventory; it does not retry or erase evidence.
    assert result.evidence_dir is not None
    assert (Path(result.evidence_dir) / "inventory.json").is_file()
    assert (Path(result.evidence_dir) / "2.zip").read_bytes() == receipt_zip


def test_ci_verification_refuses_wrong_checkout_before_download(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tools import proof_plan

    monkeypatch.setattr(proof_plan, "_source_identity", lambda: {"commit": "b" * 40})
    result = ci_proof_evidence.verify_run(
        _ci_run(),
        repo_root=tmp_path,
        commands=None,
        read_run=lambda _: {},
    )
    assert result.status == "unavailable"
    assert "a" * 40 in result.errors[0]
    assert result.evidence_dir is None


def test_main_baseline_skips_the_selected_completed_run(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture,
) -> None:
    def history(argv):
        assert argv[:3] == ["run", "list", "--branch"]
        return [
            {"databaseId": 42, "status": "in_progress"},
            {"databaseId": 41, "status": "completed"},
            {"databaseId": 40, "status": "completed"},
        ]

    observed = []

    def run(identity):
        observed.append(identity)
        return _ci_run(databaseId=identity, conclusion="failure")

    def reports(current):
        names = ["tests/x.py::known"]
        if current["databaseId"] == 41:
            names.append("tests/x.py::new")
        return [ci_triage.JobReport("unit", tests=dict.fromkeys(names, "failure"))], []

    monkeypatch.setattr(ci_triage, "_gh_json", history)
    monkeypatch.setattr(ci_triage, "_run", run)
    monkeypatch.setattr(ci_triage, "_failed_reports", reports)
    assert ci_triage.main(["41", "--json"]) == 1
    payload = json.loads(capsys.readouterr().out)
    assert observed == [41, 40]
    assert payload["baseline"]["databaseId"] == 40
    assert {item["key"]: item["new"] for item in payload["failures"]} == {
        "tests/x.py::known": False,
        "tests/x.py::new": True,
    }
