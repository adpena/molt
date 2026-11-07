from __future__ import annotations

from pathlib import Path

import pytest

from tools import gen_cpython_coverage, gen_stringprep_tables, generators


def test_coverage_doc_projects_every_configured_tuple_and_divergence() -> None:
    config = gen_cpython_coverage._load_config()
    doc = gen_cpython_coverage._render_doc(config)
    for row in config["verified"] + config["candidate"]:
        assert f"CPython {row['cpython']} on {row['platform']}" in doc
    for row in config["divergence"]:
        assert f"- `{row['surface']}` (CPython {row['introduced']})" in doc


def test_private_and_unstable_classification_is_fail_closed() -> None:
    assert gen_cpython_coverage._stability("_Py_private") == "private"
    assert gen_cpython_coverage._stability("PyUnstable_probe") == "unstable"


def test_all_registered_outputs_are_byte_synchronized() -> None:
    for path, expected in gen_cpython_coverage.generated_outputs().items():
        assert path.read_text(encoding="utf-8") == expected, path


def test_runner_opens_records_only_for_declaring_generators(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path))
    with generators._generator_records(gen_stringprep_tables, enabled=True) as records:
        assert records is not None
        assert records.scanner == "generator_gen_stringprep_tables"
    with generators._generator_records(gen_stringprep_tables, enabled=False) as records:
        assert records is None
    undeclared = type(gen_stringprep_tables)("undeclared_generator")
    undeclared.generated_outputs = lambda: {}
    with generators._generator_records(undeclared, enabled=True) as records:
        assert records is None
