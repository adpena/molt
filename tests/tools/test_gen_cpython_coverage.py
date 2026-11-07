from __future__ import annotations

from tools import gen_cpython_coverage


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
