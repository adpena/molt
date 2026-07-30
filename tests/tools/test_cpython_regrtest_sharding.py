from pathlib import Path

import pytest

from tools import cpython_regrtest


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


def test_junit_aggregates_module_status_and_duration(tmp_path: Path):
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
    assert summary.module_results == [
        {"path": "test_int", "status": "skipped", "duration_s": 0.5},
        {"path": "test_math", "status": "failed", "duration_s": 1.0},
    ]
