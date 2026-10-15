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
