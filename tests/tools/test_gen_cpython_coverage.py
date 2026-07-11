from __future__ import annotations

import json

from tools import gen_cpython_coverage


def test_live_exports_are_covered_by_matrix() -> None:
    matrix = json.loads(gen_cpython_coverage.MATRIX.read_text(encoding="utf-8"))
    assert {row["symbol"] for row in matrix["symbols"]} == {
        row["symbol"] for row in gen_cpython_coverage._exports()
    }


def test_private_and_unstable_classification_is_fail_closed() -> None:
    assert gen_cpython_coverage._stability("_Py_private") == "private"
    assert gen_cpython_coverage._stability("PyUnstable_probe") == "unstable"
