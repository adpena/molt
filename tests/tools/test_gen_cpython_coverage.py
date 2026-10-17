from __future__ import annotations

import json
from pathlib import Path

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


@pytest.mark.parametrize(
    ("source", "expected", "kind"),
    [
        ('version = "3.15.0"', "3.15", "scattered-assumption"),
        ('version = "v3.16.0a1"', "3.16", "scattered-assumption"),
        ('version = "3.100"', "3.100", "scattered-assumption"),
        ('version = "3.9"', "3.9", "scattered-assumption"),
        ("API >= 0x030F0000", "0x030F0000", "scattered-assumption"),
        ("API >= 0x03100000", "0x03100000", "scattered-assumption"),
        ("API >= 0X03640000", "0X03640000", "scattered-assumption"),
        ("API >= 0x030a00f0", "0x030a00f0", "scattered-assumption"),
        ("sys.version_info", "sys.version_info", "operational-tooling"),
        ("_sys.version_info", "_sys.version_info", "operational-tooling"),
        (
            "import sys as notsys\nnotsys.version_info",
            "notsys.version_info",
            "operational-tooling",
        ),
        (
            "import sys as platform_python\nplatform_python.version_info",
            "platform_python.version_info",
            "scattered-assumption",
        ),
        (
            "from sys import version_info as selected_version",
            "version_info",
            "scattered-assumption",
        ),
        ("PY_VERSION_HEX", "PY_VERSION_HEX", "scattered-assumption"),
        # Ambiguous numbers remain review hits; lexical shape is not provenance.
        ("ratio = 3.14159", "3.14159", "scattered-assumption"),
        ("bits = 0x03001234", "0x03001234", "scattered-assumption"),
        ("ratio = 123.15", None, None),
        ("ratio = 13.15", None, None),
        ("ratio = 1.3.15", None, None),
        ("bits = 0x030F", None, None),
        ("bits = 0x030F00000", None, None),
        ("bits = 0x1030F0000", None, None),
        ("bits = 0x030G0000", None, None),
        ("bits = 0x030F0000G", None, None),
        ("NOT_PY_VERSION_HEX", None, None),
        ("sys.version_info_suffix", None, None),
        ("version_information", None, None),
        ("unrelated = 42", None, None),
    ],
)
def test_audit_version_inventory_is_independent_of_supported_minors(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    source: str,
    expected: str | None,
    kind: str | None,
) -> None:
    scan = tmp_path / "scan"
    scan.mkdir()
    (scan / "probe.py").write_text(source, encoding="utf-8")
    monkeypatch.setattr(gen_cpython_coverage, "ROOT", tmp_path)
    rendered = gen_cpython_coverage._audit(
        {"version_assumption_audit": {"source_roots": ["scan"]}}
    )
    rows = [line for line in rendered.splitlines() if line.startswith("| `")]
    assert rows == (
        [] if expected is None else [f"| `scan/probe.py` | 1 | {kind} | `{expected}` |"]
    )


def test_audit_retains_file_level_review_classification(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    scan = tmp_path / "scan"
    scan.mkdir()
    for name, source in {
        "configured.toml": 'version = "3.15"',
        "marker_only.py": '# target_python\nversion = "3.15"',
        "operational.py": '# uv run --python\nversion = "3.15"',
        "scattered.py": 'version = "3.15"\nnext_version = "3.16"',
    }.items():
        (scan / name).write_text(source, encoding="utf-8")
    monkeypatch.setattr(gen_cpython_coverage, "ROOT", tmp_path)
    monkeypatch.setattr(gen_cpython_coverage, "CONFIG", scan / "configured.toml")
    rendered = gen_cpython_coverage._audit(
        {"version_assumption_audit": {"source_roots": ["scan"]}}
    )
    assert [line for line in rendered.splitlines() if line.startswith("| `")] == [
        "| `scan/configured.toml` | 1 | authority-routed | `3.15` |",
        "| `scan/marker_only.py` | 1 | authority-routed | `3.15` |",
        "| `scan/operational.py` | 1 | operational-tooling | `3.15` |",
        "| `scan/scattered.py` | 2 | scattered-assumption | `3.15` |",
    ]
    assert "- `authority-routed` files: 2" in rendered
    assert "- `operational-tooling` files: 1" in rendered
    assert "- `scattered-assumption` files: 1" in rendered
    assert "raw lexical hits" in rendered
    assert "do not prove authority routing or semantic conformance" in rendered
