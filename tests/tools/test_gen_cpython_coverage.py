from __future__ import annotations

import json

import pytest

from tools import gen_cpython_coverage


def test_live_exports_are_covered_by_matrix() -> None:
    matrix = json.loads(gen_cpython_coverage.MATRIX.read_text(encoding="utf-8"))
    assert {row["symbol"] for row in matrix["symbols"]} == {
        row["symbol"] for row in gen_cpython_coverage._exports()
    }


def test_private_and_unstable_classification_is_fail_closed() -> None:
    assert gen_cpython_coverage._stability("_Py_private") == "private"
    assert gen_cpython_coverage._stability("PyUnstable_probe") == "unstable"


def test_all_registered_outputs_are_byte_synchronized() -> None:
    for path, expected in gen_cpython_coverage._outputs().items():
        assert path.read_text(encoding="utf-8") == expected, path


@pytest.mark.parametrize(
    "source_root",
    ["../outside", str((gen_cpython_coverage.ROOT.parent / "outside").resolve())],
)
def test_audit_source_roots_fail_closed_outside_the_repo(source_root: str) -> None:
    config = gen_cpython_coverage._load_config()
    config["version_assumption_audit"] = {"source_roots": [source_root]}
    with pytest.raises(ValueError, match="repo-relative"):
        gen_cpython_coverage._audit_source_roots(config)


def test_audit_source_roots_reject_duplicate_authority() -> None:
    config = gen_cpython_coverage._load_config()
    config["version_assumption_audit"] = {"source_roots": ["src", "src"]}
    with pytest.raises(ValueError, match="duplicate audit source root"):
        gen_cpython_coverage._audit_source_roots(config)


def test_audit_source_roots_reject_overlapping_authority() -> None:
    config = gen_cpython_coverage._load_config()
    config["version_assumption_audit"] = {"source_roots": ["src", "src/molt"]}
    with pytest.raises(ValueError, match="overlapping audit source roots"):
        gen_cpython_coverage._audit_source_roots(config)
