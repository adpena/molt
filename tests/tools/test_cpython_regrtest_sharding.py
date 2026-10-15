import shlex
import time
from pathlib import Path

import pytest

from tools import cpython_regrtest
from tools.compat import regrtest_results


def test_tests_from_is_exact_and_disables_no_other_work_implicitly(tmp_path: Path):
    selection = tmp_path / "selection.txt"
    selection.write_text("test_math\ntest_int\n", encoding="utf-8")

    config = cpython_regrtest.parse_args(["--tests-from", str(selection), "--no-diff"])

    assert config.tests == ["test_math", "test_int"]
    assert config.diff_enabled is False


@pytest.mark.parametrize(
    "contents, message",
    [
        ("test_math\ntest_math\n", "duplicate test module"),
        ("../test_math\n", "invalid test module"),
        ("# no modules\n", "selection is empty"),
    ],
)
def test_tests_from_rejects_ambiguous_membership(
    tmp_path: Path, contents: str, message: str
):
    selection = tmp_path / "selection.txt"
    selection.write_text(contents, encoding="utf-8")

    with pytest.raises(SystemExit):
        cpython_regrtest.parse_args(["--tests-from", str(selection), "--no-diff"])


def test_junit_names_the_failed_modules(tmp_path: Path):
    junit = tmp_path / "junit.xml"
    junit.write_text(
        '<testsuite tests="3" failures="1" errors="0" skipped="1">'
        '<testcase classname="test.test_math.MathTests" name="test_a" time="0.25"/>'
        '<testcase classname="test.test_math.MathTests" name="test_b" time="0.75">'
        "<failure/>"
        "</testcase>"
        '<testcase classname="test.test_int.IntTests" name="test_c" time="0.5">'
        "<skipped/>"
        "</testcase>"
        "</testsuite>",
        encoding="utf-8",
    )

    summary = cpython_regrtest.parse_junit(junit)

    assert summary.failed_modules == ["test_math"]
    assert (summary.tests, summary.failures, summary.skipped) == (3, 1, 1)


def _selection(tmp_path: Path) -> str:
    selection = tmp_path / "selection.txt"
    selection.write_text("test_math\n", encoding="utf-8")
    return str(selection)


def test_an_explicit_cpython_dir_is_the_checkout_used(tmp_path: Path):
    # Nightly 38652601508: `--cpython-dir third_party/cpython` became
    # `third_party/cpython-3.12` by a name-containment rule, so every regrtest
    # shard failed before running a test.
    given = tmp_path / "cpython"
    explicit = cpython_regrtest.parse_args(
        ["--uv", "--uv-python", "3.12", "--cpython-dir", str(given)]
        + ["--tests-from", _selection(tmp_path), "--no-diff"]
    )
    assert explicit.cpython_dir == given.resolve()
    default = cpython_regrtest.parse_args(
        [
            "--uv",
            "--uv-python",
            "3.12",
            "--tests-from",
            _selection(tmp_path),
            "--no-diff",
        ]
    )
    assert default.cpython_dir.name == "cpython-3.12"
    assert isinstance(default.cpython_custody, cpython_regrtest.GitCustody)


def test_shard_plan_custody_needs_its_plan_and_no_clone(tmp_path: Path):
    base = ["--tests-from", _selection(tmp_path), "--no-diff"]
    plan = tmp_path / "plan.json"
    config = cpython_regrtest.parse_args(
        base + ["--cpython-custody", "shard-plan", "--shard-plan", str(plan)]
    )
    assert config.cpython_custody == cpython_regrtest.ShardPlanCustody(plan.resolve())
    with pytest.raises(ValueError, match="requires --shard-plan"):
        cpython_regrtest.parse_args(base + ["--cpython-custody", "shard-plan"])
    with pytest.raises(ValueError, match="requires --shard-plan"):
        cpython_regrtest.parse_args(
            base
            + ["--cpython-custody", "shard-plan", "--shard-plan", str(plan), "--clone"]
        )
    with pytest.raises(ValueError, match="requires --cpython-custody shard-plan"):
        cpython_regrtest.parse_args(base + ["--shard-plan", str(plan)])


def test_shard_plan_custody_admits_only_the_pinned_plan_tree(monkeypatch, tmp_path):
    from tools import nightly_sharding

    source = next(iter(cpython_regrtest.load_cpython_sources().values()))
    plan_commit = {"value": source.revision}
    monkeypatch.setattr(
        nightly_sharding,
        "_load_plan",
        lambda path: {"cpython_commit": plan_commit["value"]},
    )
    custody = cpython_regrtest.ShardPlanCustody(tmp_path / "plan.json")
    tree = nightly_sharding.ROOT / "third_party" / "cpython"

    def check(directory: Path) -> None:
        cpython_regrtest.ensure_cpython_checkout(
            directory,
            source,
            custody=custody,
            allow_clone=False,
            log_handle=None,
            dry_run=False,
        )

    check(tree)
    with pytest.raises(RuntimeError, match="covers"):
        check(tmp_path / "elsewhere")
    plan_commit["value"] = "0" * 40
    with pytest.raises(RuntimeError, match="does not carry the pinned CPython source"):
        check(tree)


def test_deadline_admission_needs_the_per_test_timeout(tmp_path: Path):
    base = ["--tests-from", _selection(tmp_path), "--no-diff"]
    with pytest.raises(SystemExit):
        cpython_regrtest.parse_args(base + ["--deadline-s", "100"])
    with pytest.raises(SystemExit):
        cpython_regrtest.parse_args(base + ["--timeout", "60", "--deadline-s", "0"])

    before = time.time()
    config = cpython_regrtest.parse_args(
        base + ["--timeout", "60", "--deadline-s", "100"]
    )
    assert config.admit_until is not None
    assert before + 100 <= config.admit_until <= time.time() + 100


def test_the_shim_receives_the_deadline_and_the_result_log(tmp_path: Path):
    log = tmp_path / "results.jsonl"
    config = cpython_regrtest.parse_args(
        ["--tests-from", _selection(tmp_path), "--no-diff", "--timeout", "60"]
        + ["--deadline-s", "100", "--results-jsonl", str(log)]
    )
    cmd = cpython_regrtest.build_regrtest_cmd(
        config, tmp_path / "regrtest.py", [], ["python3"]
    )
    shim_cmd = shlex.split(cmd[cmd.index("--python") + 1])

    assert shim_cmd[shim_cmd.index("--results-jsonl") + 1] == str(log)
    assert float(shim_cmd[shim_cmd.index("--admit-until") + 1]) == config.admit_until
    assert cmd[cmd.index("--timeout") + 1] == "60"


def test_item_rows_come_from_the_result_log(tmp_path: Path):
    log = tmp_path / "results.jsonl"
    regrtest_results.append(log, test="test_math", state="PASSED", duration_s=2.0)
    regrtest_results.append(
        log, test="test_os", state="DID_NOT_RUN", duration_s=None, reason="deadline"
    )

    rows = cpython_regrtest.item_rows(["test_math", "test_os", "test_io"], log)

    assert rows == [
        {"path": "test_math", "status": "passed", "duration_s": 2.0, "reason": ""},
        {
            "path": "test_os",
            "status": "not_run",
            "duration_s": None,
            "reason": "deadline",
        },
        # Selected but never reported: it did not run.
        {
            "path": "test_io",
            "status": "not_run",
            "duration_s": None,
            "reason": "no result row",
        },
    ]
