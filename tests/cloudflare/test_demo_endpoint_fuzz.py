from __future__ import annotations

from pathlib import Path

import pytest

import tools.cloudflare_demo_verify as cloudflare_demo_verify


ROOT = Path(__file__).resolve().parents[2]
ENTRY = ROOT / "examples" / "cloudflare-demo" / "src" / "app.py"


def test_demo_fuzz_matrix_covers_malformed_and_boundary_inputs() -> None:
    cases = cloudflare_demo_verify.build_demo_fuzz_matrix()

    targets = {(case.path, case.query) for case in cases}

    assert ("/fib/-1", "") in targets
    assert ("/fib/1000000000000", "") in targets
    assert ("/generate/-1", "") in targets
    assert ("/generate/999999999999", "") in targets
    assert ("/sort", "data=3,,1,foo,2") in targets
    assert ("/sort", "data=1&data=2&&") in targets
    assert (
        "/sql",
        "q=SELECT%20*%20FROM%20cities%20WHERE%20name%20LIKE%20%27%ZZ%27",
    ) in targets


def test_demo_fuzz_matrix_runs_cleanly(tmp_path: Path) -> None:
    report = cloudflare_demo_verify.verify_source_matrix(
        ENTRY,
        cloudflare_demo_verify.build_demo_fuzz_matrix(),
        artifact_root=tmp_path,
    )

    assert report.ok
    assert report.failures == []


def test_demo_http_body_sanitizer_rejects_nul_prefix() -> None:
    with pytest.raises(cloudflare_demo_verify.VerificationError):
        cloudflare_demo_verify.assert_clean_text_body(b"\x00Error")
