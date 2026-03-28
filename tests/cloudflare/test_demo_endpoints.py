from __future__ import annotations

from pathlib import Path

import tools.cloudflare_demo_verify as cloudflare_demo_verify


ROOT = Path(__file__).resolve().parents[2]
ENTRY = ROOT / "examples" / "cloudflare-demo" / "src" / "app.py"


def test_demo_matrix_includes_variable_bearing_routes() -> None:
    cases = cloudflare_demo_verify.build_demo_matrix()

    names = {case.name for case in cases}
    targets = {(case.path, case.query) for case in cases}

    assert "generate_one" in names
    assert "sort_query" in names
    assert "sql_query" in names
    assert "demo_landing" in names
    assert "/generate/1" in {path for path, _query in targets}
    assert ("/sort", "data=42,7,19,3,88,1") in targets
    assert ("/sql", "q=SELECT%20*%20FROM%20cities%20LIMIT%201") in targets


def test_demo_source_matrix_runs_cleanly(tmp_path: Path) -> None:
    report = cloudflare_demo_verify.verify_source_matrix(
        ENTRY,
        cloudflare_demo_verify.build_demo_matrix(),
        artifact_root=tmp_path,
    )

    assert report.ok
    assert report.failures == []


def test_demo_source_matrix_writes_summary(tmp_path: Path) -> None:
    report = cloudflare_demo_verify.verify_source_matrix(
        ENTRY,
        cloudflare_demo_verify.build_demo_matrix(),
        artifact_root=tmp_path,
    )

    summary = tmp_path / "summary.json"
    assert report.ok
    assert summary.exists()
