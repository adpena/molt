from __future__ import annotations

from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process
from tools import gen_cpython_coverage


def _source_repository(root: Path) -> Path:
    """A real Git root: the audit admits only Git's source inventory."""
    completed = run_guarded_test_process(["git", "init", "-q", str(root)])
    assert completed.returncode == 0, completed.stderr
    return root


def test_coverage_doc_projects_every_configured_tuple_and_divergence() -> None:
    config = gen_cpython_coverage._load_config()
    doc = gen_cpython_coverage._outputs(config)[gen_cpython_coverage.DOC]
    for row in config["verified"] + config["candidate"]:
        assert f"CPython {row['cpython']} on {row['platform']}" in doc
    for row in config["divergence"]:
        assert f"- `{row['surface']}` (CPython {row['introduced']})" in doc


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
    scan = _source_repository(tmp_path) / "scan"
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
    scan = _source_repository(tmp_path) / "scan"
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


def test_audit_ignores_build_output_inside_a_source_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A Cargo target under a source root never reaches the projection."""
    scan = _source_repository(tmp_path) / "scan"
    (scan / "target-local" / "out").mkdir(parents=True)
    (tmp_path / ".gitignore").write_text("target-*/\n", encoding="utf-8")
    (scan / "probe.py").write_text('version = "3.15"\n', encoding="utf-8")
    (scan / "target-local" / "out" / "ssl.h").write_text(
        "#define OPENSSL_VERSION 3.3\n", encoding="utf-8"
    )
    monkeypatch.setattr(gen_cpython_coverage, "ROOT", tmp_path)
    rendered = gen_cpython_coverage._audit(
        {"version_assumption_audit": {"source_roots": ["scan"]}}
    )
    assert [line for line in rendered.splitlines() if line.startswith("| `")] == [
        "| `scan/probe.py` | 1 | scattered-assumption | `3.15` |"
    ]


# --- Scan records: tools/generators.py replays unchanged audit rows ---------

from tools import generators, scan_records  # noqa: E402


def _coverage_records(cache: Path) -> scan_records.ScanRecords:
    return scan_records.ScanRecords(
        "generator_gen_cpython_coverage",
        owner=Path(gen_cpython_coverage.__file__),
        cache_root=cache,
    )


def _rows(rendered: str) -> list[str]:
    return [line for line in rendered.splitlines() if line.startswith("| `")]


def test_audit_rows_replay_and_catch_a_seeded_assumption(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo = _source_repository(tmp_path / "repo")
    scan = repo / "scan"
    scan.mkdir()
    configured = scan / "configured.toml"
    configured.write_text('version = "3.15"\n', encoding="utf-8")
    # The same bytes outside the configuration are a scattered assumption.
    (scan / "copy.toml").write_text('version = "3.15"\n', encoding="utf-8")
    clean = scan / "clean.py"
    clean.write_text("value = 1\n", encoding="utf-8")
    monkeypatch.setattr(gen_cpython_coverage, "ROOT", repo)
    monkeypatch.setattr(gen_cpython_coverage, "CONFIG", configured)
    config = {"version_assumption_audit": {"source_roots": ["scan"]}}
    cache = tmp_path / "cache"
    expected = gen_cpython_coverage._audit(config)
    assert _rows(expected) == [
        "| `scan/configured.toml` | 1 | authority-routed | `3.15` |",
        "| `scan/copy.toml` | 1 | scattered-assumption | `3.15` |",
    ]
    for expected_misses in (3, 0):
        records = _coverage_records(cache)
        assert gen_cpython_coverage._audit(config, records) == expected
        assert records.misses == expected_misses
        records.publish()

    clean.write_text('value = "3.16"\n', encoding="utf-8")
    for expected_misses in (1, 0):
        records = _coverage_records(cache)
        rows = _rows(gen_cpython_coverage._audit(config, records))
        assert "| `scan/clean.py` | 1 | scattered-assumption | `3.16` |" in rows
        assert records.misses == expected_misses
        records.publish()


def test_audit_rows_replay_the_real_tree(tmp_path: Path) -> None:
    config = gen_cpython_coverage._load_config()
    cold = _coverage_records(tmp_path)
    computed = gen_cpython_coverage._audit(config, cold)
    assert cold.publish() is True
    warm = _coverage_records(tmp_path)
    assert gen_cpython_coverage._audit(config, warm) == computed
    assert (warm.hits, warm.misses) == (cold.misses + cold.hits, 0)
    assert len(_rows(computed)) > 100


def test_runner_opens_records_only_for_declaring_generators(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path))
    with generators._generator_records(gen_cpython_coverage, enabled=True) as records:
        assert records is not None
        assert records.scanner == "generator_gen_cpython_coverage"
    with generators._generator_records(gen_cpython_coverage, enabled=False) as records:
        assert records is None
    undeclared = type(gen_cpython_coverage)("undeclared_generator")
    undeclared.generated_outputs = lambda: {}
    with generators._generator_records(undeclared, enabled=True) as records:
        assert records is None
