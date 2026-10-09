from __future__ import annotations

import json
import os
from copy import deepcopy
from pathlib import Path

import pytest

from tests import runtime_descendant_test_support as support
from tools import runtime_descendant_receipts as descendants

LIB_OWNERS = [support.TRAP, support.COLD]
TRACE_OWNERS = [support.TRACE_CALLARGS, support.TRACE_BIND_IC, support.TRACE_BIND_META]


def lib_receipt(tmp_path: Path, owners=LIB_OWNERS, **options):
    image = support.make_image(tmp_path)
    tests = ["ordinary::parallel_case", support.TRAP_CHILD, *owners]
    records = support.family_records(image, owners)
    receipt = support.binary_receipt(
        tmp_path / "receipts", image, tests, records, **options
    )
    return receipt, records


def verify(receipt, **options):
    return descendants.verify_receipt(receipt, **options)


def test_validator_roster_matches_the_owning_rust_call_sites() -> None:
    roster = {
        (owner, mode): (
            spec.role,
            [
                "--exact",
                spec.child,
                *(["--ignored"] if spec.ignored else []),
                *support.SERIAL,
            ],
            spec.modes[mode].exit_code,
            spec.modes[mode].completes,
        )
        for owner, spec in descendants.OWNERS.items()
        for mode in spec.modes
    }
    assert roster == {
        key: (value["role"], value["args"], value["exit_code"], value["completes"])
        for key, value in support.CONTRACT.items()
    }
    assert len(roster) == 19


@pytest.mark.parametrize(
    ("owners", "children"),
    [
        (LIB_OWNERS, 2),
        ([support.EXIT], 2),
        (TRACE_OWNERS, 3),
        ([support.TRANSACTION], 9),
        ([support.LIFECYCLE], 3),
    ],
    ids=[
        "parallel-trap-cold",
        "isolated-exit-both-leases",
        "trace-image",
        "terminal-transaction",
        "lifecycle-ffi",
    ],
)
def test_completed_owners_with_bound_children_verify(tmp_path, owners, children):
    receipt, _ = lib_receipt(tmp_path, owners)
    assert verify(receipt, receipt_root=tmp_path / "receipts") == {
        "status": "verified",
        "children": children,
        "coordinate_authority": "unavailable",
    }


def test_unrelated_binary_is_not_engaged_even_without_evidence(tmp_path):
    image = support.make_image(tmp_path, "unrelated-0123456789abcdef.exe")
    receipt = support.binary_receipt(tmp_path / "receipts", image, ["ordinary"], [])
    assert verify(receipt) is None
    fake = {
        "test_results": [{"identity": "ordinary", "status": "pass"}],
        "executions": [{"argv": ["missing"], "stdout_evidence": None}],
    }
    assert verify(fake) is None


@pytest.mark.parametrize("index", range(19))
def test_every_mandatory_child_is_derived_from_completed_owner_rows(tmp_path, index):
    owners = [
        support.EXIT,
        *LIB_OWNERS,
        *TRACE_OWNERS,
        support.TRANSACTION,
        support.LIFECYCLE,
    ]
    image = support.make_image(tmp_path)
    records = support.family_records(image, owners)
    removed = records.pop(index)
    receipt = support.binary_receipt(
        tmp_path / "receipts", image, ["ordinary", *owners], records
    )
    with pytest.raises(descendants.DescendantEvidenceError, match="missing mandatory"):
        verify(receipt)
    assert (removed["parent_test"], removed["mode"]) in support.CONTRACT


def test_absent_prefixes_cannot_bypass_the_completed_owner_roster(tmp_path):
    receipt, _ = lib_receipt(tmp_path)
    support.republish(receipt, "stderr", "")
    with pytest.raises(descendants.DescendantEvidenceError, match="missing mandatory"):
        verify(receipt)


def test_forged_saved_rows_are_rederived_from_the_full_parent_capture(tmp_path):
    receipt, _ = lib_receipt(tmp_path)
    support.republish(receipt, "stderr", "")
    receipt["test_results"] = [
        {"identity": "ordinary::parallel_case", "status": "pass"}
    ]
    receipt["runtime_descendants"] = {"status": "verified", "children": 2}
    with pytest.raises(descendants.DescendantEvidenceError, match="disagree"):
        verify(receipt)


def test_failed_owner_needs_no_child_but_cannot_hide_a_passing_one(tmp_path):
    image = support.make_image(tmp_path)
    records = support.family_records(image, [support.COLD])
    tests = ["ordinary", *LIB_OWNERS]
    statuses = {support.TRAP: "fail"}
    receipt = support.binary_receipt(
        tmp_path / "receipts", image, tests, records, statuses=statuses
    )
    assert verify(receipt)["children"] == 1
    support.republish(receipt, "stderr", "")
    with pytest.raises(descendants.DescendantEvidenceError, match="wasm_abi_exports"):
        verify(receipt)


def test_child_of_an_ignored_or_unrun_owner_is_foreign(tmp_path):
    image = support.make_image(tmp_path)
    records = support.family_records(image, [support.EXIT, support.COLD])
    receipt = support.binary_receipt(
        tmp_path / "receipts", image, ["ordinary", support.COLD], records
    )
    with pytest.raises(
        descendants.DescendantEvidenceError, match="without a completed owner"
    ):
        verify(receipt)


@pytest.mark.parametrize(
    ("change", "message"),
    [
        ("duplicate", "duplicate runtime descendant"),
        ("source", "source identity differs"),
        ("unadmitted_source", "source identity is not admitted"),
        ("image_path", "foreign image"),
        ("image_hash", "foreign image"),
        ("argv_extra", "exact argv"),
        ("argv_image", "exact argv"),
        ("missing_serial", "exact argv"),
        ("parent", "without a completed owner"),
        ("mode", "without a completed owner"),
        ("role", "exact argv"),
        ("child", "exact argv"),
        ("coordinate", "coordinate authority"),
        ("schema", "schema"),
        ("extra_field", "schema"),
        ("missing_field", "schema"),
        ("exit_code", "termination"),
        ("bool_exit", "termination"),
        ("stream_outside_custody", "test-image custody"),
        ("stream_wrong_name", "test-image custody"),
        ("stream_hash", "stream changed"),
        ("stream_size", "stream changed"),
        ("stream_label", "not a descendant capture"),
        ("two_owners", "two owners"),
    ],
)
def test_foreign_or_mutated_child_record_is_rejected(tmp_path, change, message):
    receipt, records = lib_receipt(tmp_path)
    cold = next(row for row in records if row["parent_test"] == support.COLD)
    trap = next(row for row in records if row["parent_test"] == support.TRAP)
    image = Path(cold["executable"])
    if change == "duplicate":
        records.append(deepcopy(cold))
    elif change == "source":
        cold["source_identity"] = {**support.SOURCE, "head": "foreign"}
    elif change == "unadmitted_source":
        receipt["source_identity"] = None
        for row in records:
            row["source_identity"] = None
    elif change == "image_path":
        foreign = image.with_name("other-0123456789abcdef.exe")
        foreign.write_bytes(image.read_bytes())
        cold["executable"] = str(foreign)
    elif change == "image_hash":
        cold["executable_sha256"] = "0" * 64
    elif change == "argv_extra":
        cold["argv"].append("--include-ignored")
    elif change == "argv_image":
        cold["argv"][0] = str(image.with_name("other.exe"))
    elif change == "missing_serial":
        cold["argv"].remove("--test-threads=1")
    elif change == "parent":
        cold["parent_test"] = "wasm_abi_exports::tests::other"
    elif change == "mode":
        cold["mode"] = "warm"
    elif change == "role":
        cold["role"] = "pending-success-trap"
    elif change == "child":
        cold["child_test"] = support.TRAP_CHILD
    elif change == "coordinate":
        cold["coordinate_authority"] = {"python": "3.14"}
    elif change == "schema":
        cold["schema"] = "molt.runtime-descendant.v0"
    elif change == "extra_field":
        cold["python_minor"] = "3.13"
    elif change == "missing_field":
        del cold["termination"]
    elif change == "exit_code":
        cold["termination"] = {"kind": "exit", "code": 101}
    elif change == "bool_exit":
        cold["termination"] = {"kind": "exit", "code": False}
    elif change == "stream_outside_custody":
        outside = support.publish(tmp_path / "outside" / "stdout.log", "x")
        cold["stdout"] = outside
    elif change == "stream_wrong_name":
        path = Path(cold["stdout"]["path"])
        renamed = path.with_name("stderr.log.bak")
        renamed.write_bytes(path.read_bytes())
        cold["stdout"]["path"] = str(renamed)
    elif change == "stream_hash":
        cold["stdout"]["sha256"] = "0" * 64
    elif change == "stream_size":
        cold["stdout"]["bytes"] += 1
    elif change == "stream_label":
        (Path(cold["stdout"]["path"]).parent / "artifact-label.txt").write_text(
            "other", encoding="utf-8"
        )
    elif change == "two_owners":
        cold["stderr"] = deepcopy(trap["stderr"])
    support.republish(receipt, "stderr", support.records_text(records))
    with pytest.raises(descendants.DescendantEvidenceError, match=message):
        verify(receipt)


def test_both_shutdown_modes_need_their_own_retained_streams(tmp_path):
    image = support.make_image(tmp_path)
    no_lease, lease = support.family_records(image, [support.EXIT])
    # Supply both literal proof markers so this control reaches the independent
    # shared-stream-owner check rather than failing mode binding first.
    stdout_path = Path(no_lease["stdout"]["path"])
    no_lease["stdout"] = support.publish(
        stdout_path,
        stdout_path.read_text()
        + "shutdown callbacks verified before process exit: lease\n",
    )
    lease["stdout"] = deepcopy(no_lease["stdout"])
    lease["stderr"] = deepcopy(no_lease["stderr"])
    receipt = support.binary_receipt(
        tmp_path / "receipts", image, [support.EXIT], [no_lease, lease]
    )
    with pytest.raises(descendants.DescendantEvidenceError, match="share one owner"):
        verify(receipt)


@pytest.mark.parametrize(
    ("owner", "stream", "text", "message"),
    [
        (
            support.EXIT,
            "stdout",
            f"\nrunning 1 test\ntest {support.EXIT} ... ",
            "lacks",
        ),
        (support.TRAP, "stderr", "thread panicked\n", "lacks"),
        (support.TRACE_BIND_IC, "stderr", "[molt callargs] new\n", "lacks"),
        (
            support.COLD,
            "stdout",
            f"\nrunning 1 test\ntest {support.COLD} ... ",
            "one-test",
        ),
        (
            support.COLD,
            "stdout",
            support.CONTRACT[(support.COLD, "cold")]["stdout"]
            .replace("1 passed; 0 failed", "0 passed; 1 failed")
            .replace("... ok", "... FAILED")
            .replace("result: ok", "result: FAILED"),
            "one-test",
        ),
        (
            support.TRAP,
            "stdout",
            "\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.00s\n",
            "selected test",
        ),
        (
            support.EXIT,
            "stdout",
            support.CONTRACT[(support.COLD, "cold")]["stdout"].replace(
                support.COLD, support.EXIT
            )
            + "shutdown callbacks verified before process exit\n",
            "selected test",
        ),
    ],
    ids=[
        "exit-without-callback-completion",
        "trap-without-intentional-trap",
        "trace-without-its-own-log",
        "cold-without-summary",
        "cold-failed",
        "trap-filtered-nothing",
        "exit-returned-to-libtest",
    ],
)
def test_child_semantics_come_from_its_own_retained_streams(
    tmp_path, owner, stream, text, message
):
    image = support.make_image(tmp_path)
    records = support.family_records(image, [owner])
    for record in records:
        published = support.publish(Path(record[stream]["path"]), text)
        record[stream] = published
    receipt = support.binary_receipt(
        tmp_path / "receipts", image, ["ordinary", owner], records
    )
    with pytest.raises(descendants.DescendantEvidenceError, match=message):
        verify(receipt)


@pytest.mark.parametrize("minor", ["3.12", "3.13", "3.14", "3.15"])
def test_requested_python_minor_fails_closed_without_minor_authority(tmp_path, minor):
    receipt, _ = lib_receipt(tmp_path)
    with pytest.raises(descendants.DescendantEvidenceError, match="minor authority"):
        verify(receipt, required_minor=minor)
    image = support.make_image(tmp_path / "unrelated", "unrelated-0123456789abcdef.exe")
    unrelated = support.binary_receipt(
        tmp_path / "unrelated-receipts", image, ["x"], []
    )
    with pytest.raises(descendants.DescendantEvidenceError, match="minor authority"):
        verify(unrelated, required_minor=minor)


@pytest.mark.parametrize(
    ("termination", "admitted"),
    [
        ({"kind": "signal", "signal": 6}, {"posix"}),
        ({"kind": "windows-exception", "code": 0xC0000409}, {"nt"}),
        ({"kind": "exit", "code": -1073740791}, set()),
        ({"kind": "exit", "code": 0xC0000409}, set()),
        ({"kind": "exit", "code": 101}, set()),
        ({"kind": "exit", "code": 134}, set()),
        ({"kind": "signal", "signal": 11}, set()),
        ({"kind": "windows-exception", "code": 0xC0000005}, set()),
    ],
)
def test_abort_termination_stays_typed_per_platform(tmp_path, termination, admitted):
    receipt, records = lib_receipt(tmp_path)
    trap = next(row for row in records if row["parent_test"] == support.TRAP)
    trap["termination"] = termination
    support.republish(receipt, "stderr", support.records_text(records))
    for platform in ("posix", "nt"):
        if platform in admitted:
            assert verify(receipt, platform=platform)["children"] == 2
        else:
            with pytest.raises(
                descendants.DescendantEvidenceError, match="termination"
            ):
                verify(receipt, platform=platform)


@pytest.mark.parametrize(
    ("change", "message"),
    [
        ("parent_stdout_bytes", "stdout capture changed"),
        ("parent_stderr_bytes", "stderr capture changed"),
        ("descendant_bytes", "stream changed"),
        ("incomplete_success", "complete re-derived"),
        ("foreign_loader_root", "loader's custody root"),
        ("capture_outside_custody", "escaped receipt custody"),
        ("record_on_stdout", "canonical stderr"),
        ("image_changed", "image changed"),
        ("baseline_image", "did not run the receipt image"),
    ],
)
def test_promotion_reopens_raw_parent_and_descendant_evidence(
    tmp_path, change, message
):
    receipt, records = lib_receipt(tmp_path)
    execution = receipt["executions"][0]
    stdout = Path(execution["stdout_evidence"])
    root = tmp_path / "receipts"
    if change == "parent_stdout_bytes":
        stdout.write_bytes(stdout.read_bytes() + b"mutation\n")
    elif change == "parent_stderr_bytes":
        path = Path(execution["stderr_evidence"])
        path.write_bytes(path.read_bytes() + b"mutation\n")
    elif change == "descendant_bytes":
        Path(records[0]["stderr"]["path"]).write_text("mutation", encoding="utf-8")
    elif change == "incomplete_success":
        text = stdout.read_text(encoding="utf-8").split("test result:")[0]
        support.republish(receipt, "stdout", text)
    elif change == "foreign_loader_root":
        root = tmp_path / "other-receipts"
    elif change == "capture_outside_custody":
        moved = tmp_path / "elsewhere" / stdout.name
        moved.parent.mkdir()
        moved.write_bytes(stdout.read_bytes())
        execution["stdout_evidence"] = str(moved)
    elif change == "record_on_stdout":
        support.republish(
            receipt,
            "stdout",
            stdout.read_text(encoding="utf-8") + support.records_text(records[:1]),
        )
    elif change == "image_changed":
        image = Path(receipt["executable_resolved"])
        image.write_bytes(image.read_bytes() + b"rebuilt")
    elif change == "baseline_image":
        execution["argv"][0] = str(tmp_path / "other.exe")
    with pytest.raises(descendants.DescendantEvidenceError, match=message):
        verify(receipt, receipt_root=root)


def test_complete_captures_beyond_the_receipt_tail_are_read(tmp_path):
    receipt, records = lib_receipt(tmp_path)
    tests = [row["identity"] for row in receipt["test_results"]]
    noise = "".join(f"noise line {index:05d} {'x' * 64}\n" for index in range(2_000))
    support.republish(receipt, "stdout", noise + support.transcript(tests))
    support.republish(receipt, "stderr", noise + support.records_text(records) + noise)
    assert Path(receipt["executions"][0]["stderr_evidence"]).stat().st_size > 16_384 * 8
    assert verify(receipt)["children"] == 2


def test_malformed_record_line_is_rejected_not_skipped(tmp_path):
    receipt, records = lib_receipt(tmp_path)
    text = support.records_text(records) + f"{support.PREFIX}{{not json\n"
    support.republish(receipt, "stderr", text)
    with pytest.raises(descendants.DescendantEvidenceError, match="malformed"):
        verify(receipt)
    duplicate_key = json.dumps(records[0])[:-1] + ', "mode": "cold"}'
    support.republish(receipt, "stderr", f"{support.PREFIX}{duplicate_key}\n")
    with pytest.raises(descendants.DescendantEvidenceError, match="malformed"):
        verify(receipt)


def test_receipt_outcome_reports_failure_without_raising(tmp_path):
    receipt, _ = lib_receipt(tmp_path)
    support.republish(receipt, "stderr", "")
    outcome = descendants.receipt_outcome(receipt)
    assert outcome is not None
    assert outcome["status"] == "failed"
    assert "missing mandatory" in outcome["error"]
    assert outcome["coordinate_authority"] == "unavailable"


@pytest.mark.skipif(os.name != "nt", reason="Windows verbatim path spelling")
def test_windows_verbatim_producer_paths_bind_the_same_image(tmp_path):
    receipt, records = lib_receipt(tmp_path)
    for record in records:
        record["executable"] = "\\\\?\\" + record["executable"]
        record["stdout"]["path"] = "\\\\?\\" + record["stdout"]["path"]
        record["stderr"]["path"] = "\\\\?\\" + record["stderr"]["path"]
    support.republish(receipt, "stderr", support.records_text(records))
    assert verify(receipt)["children"] == 2


@pytest.mark.parametrize(
    ("mode", "termination"),
    [
        ("init", {"kind": "exit", "code": 1}),
        ("shutdown", {"kind": "exit", "code": 1}),
        ("exit", {"kind": "exit", "code": 0}),
        ("exit", {"kind": "exit", "code": True}),
        ("exit", {"kind": "exit", "code": 1.0}),
        ("exit", {"kind": "signal", "signal": 6}),
        ("exit", {"kind": "windows-exception", "code": 0xC0000409}),
    ],
)
def test_lifecycle_modes_require_exact_typed_termination(tmp_path, mode, termination):
    receipt, records = lib_receipt(tmp_path, owners=[support.LIFECYCLE])
    next(row for row in records if row["mode"] == mode)["termination"] = termination
    support.republish(receipt, "stderr", support.records_text(records))
    with pytest.raises(descendants.DescendantEvidenceError, match="termination"):
        verify(receipt)


@pytest.mark.parametrize("mode", ["init", "shutdown", "exit"])
def test_lifecycle_modes_keep_their_actual_completion_contract(tmp_path, mode):
    receipt, records = lib_receipt(tmp_path, owners=[support.LIFECYCLE])
    row = next(record for record in records if record["mode"] == mode)
    prefix = f"\nrunning 1 test\ntest {support.LIFECYCLE} ... "
    text = prefix if mode != "exit" else support.transcript([support.LIFECYCLE])
    row["stdout"] = support.publish(Path(row["stdout"]["path"]), text)
    support.republish(receipt, "stderr", support.records_text(records))
    with pytest.raises(
        descendants.DescendantEvidenceError,
        match="one-test completion|before terminating inside it",
    ):
        verify(receipt)


@pytest.mark.parametrize(
    ("owner", "left", "right", "stream"),
    [
        (support.TRANSACTION, "prior", "cleanup", "stdout"),
        (support.TRANSACTION, "ordinary", "ordinary-return", "stdout"),
        (support.LIFECYCLE, "init", "shutdown", "stderr"),
        (support.EXIT, "no-lease", "lease", "stdout"),
    ],
)
def test_same_child_modes_cannot_exchange_valid_retained_output(
    tmp_path, owner, left, right, stream
):
    image = support.make_image(tmp_path)
    records = support.family_records(image, [owner])
    by_mode = {record["mode"]: record for record in records}
    # Keep both genuine streams, their hashes, owners, complete roster, exact
    # child argv and equal termination. Only the claimed mode is exchanged.
    by_mode[left]["mode"], by_mode[right]["mode"] = right, left
    receipt = support.binary_receipt(tmp_path / "receipts", image, [owner], records)
    with pytest.raises(
        descendants.DescendantEvidenceError, match=f"descendant {stream} lacks"
    ):
        verify(receipt)


def test_mode_specific_proof_matches_each_literal_child_contract():
    expected = {
        (support.TRANSACTION, mode): (
            (f"transaction outcome and custody verified: {mode}\n",),
            (),
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
    }
    expected.update(
        {
            (support.EXIT, "no-lease"): (
                ("shutdown callbacks verified before process exit: no-lease\n",),
                (),
            ),
            (support.EXIT, "lease"): (
                ("shutdown callbacks verified before process exit: lease\n",),
                (),
            ),
            (support.LIFECYCLE, "init"): (
                (),
                (
                    "molt runtime lifecycle failed: injected unpublished runtime init panic\n",
                ),
            ),
            (support.LIFECYCLE, "shutdown"): (
                (),
                (
                    "molt runtime lifecycle failed: injected shutdown drain C extension cleanup panic\n",
                ),
            ),
            (support.LIFECYCLE, "exit"): (
                (),
                (
                    "molt runtime lifecycle failed: injected shutdown drain C extension cleanup panic\n",
                ),
            ),
        }
    )
    for (owner, mode), contract in support.CONTRACT.items():
        stdout, stderr = expected.get((owner, mode), ((), ()))
        actual = descendants.OWNERS[owner].modes[mode]
        assert (actual.stdout_markers, actual.stderr_markers) == (stdout, stderr)
        assert all(marker in contract["stdout"] for marker in stdout)
        assert all(marker in contract["stderr"] for marker in stderr)


def test_transaction_mode_prefix_is_not_complete_mode_evidence(tmp_path):
    image = support.make_image(tmp_path)
    records = support.family_records(image, [support.TRANSACTION])
    ordinary = next(row for row in records if row["mode"] == "ordinary")
    ordinary["stdout"] = support.publish(
        Path(ordinary["stdout"]["path"]),
        support.CONTRACT[(support.TRANSACTION, "ordinary-return")]["stdout"],
    )
    receipt = support.binary_receipt(
        tmp_path / "receipts", image, [support.TRANSACTION], records
    )
    with pytest.raises(descendants.DescendantEvidenceError, match="stdout lacks"):
        verify(receipt)
