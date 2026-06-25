"""Tests for the perf authority boundary (tools/perf_authority.py).

These pin the two non-negotiable invariants of the canonicalized perf source
of truth:

  * A missing / None / non-positive / non-finite molt time can NEVER yield a
    finite (or infinite) speedup ratio - a BUILD_FAILED / daemon_crash cell
    must render as n/a, never as ~0.01x. This is the mechanical kill for the
    forensic "build failure rendered as a 0.01x regression" mislead.
  * Every non-canonical lane (bench.py, bench/harness.py) stamps
    authoritative=False so its numbers self-identify and are never cited.
"""

from __future__ import annotations

import math
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_authority as pa  # noqa: E402
import check_perf_freshness as freshness  # noqa: E402


# --- safe_speedup: a build failure can never become a ratio ------------------


@pytest.mark.parametrize(
    "cpy,mlt",
    [
        (1.0, None),  # molt time missing (build failure / crash)
        (None, 1.0),  # cpython time missing
        (None, None),
        (1.0, 0.0),  # zero molt time (degenerate)
        (1.0, -0.5),  # negative molt time
        (0.0, 1.0),  # zero cpython time
        (-1.0, 1.0),  # negative cpython time
        (float("nan"), 1.0),  # non-finite
        (1.0, float("nan")),
        (float("inf"), 1.0),
        (1.0, float("inf")),
        ("oops", 1.0),  # non-numeric (defensive)
        (1.0, "oops"),
    ],
)
def test_safe_speedup_unmeasurable_is_none(cpy: object, mlt: object) -> None:
    result = pa.safe_speedup(cpy, mlt)  # type: ignore[arg-type]
    assert result is None, (
        f"unmeasurable ({cpy!r}, {mlt!r}) must yield None, got {result!r} - "
        "a build failure must NEVER render as a finite ratio"
    )


def test_safe_speedup_build_failure_shape_never_finite() -> None:
    # The exact shape the bench lanes produce on build failure: molt_time=None.
    # Assert no input with molt_time=None ever yields a finite number.
    for cpy in (0.001, 0.063, 1.0, 33.25, 1e9):
        out = pa.safe_speedup(cpy, None)
        assert out is None
        assert not (isinstance(out, float) and math.isfinite(out))


def test_safe_speedup_real_measurements() -> None:
    # A genuine fast result: 0.063s cpython / 0.001s molt = 63x faster.
    assert pa.safe_speedup(0.063, 0.001) == pytest.approx(63.0)
    # A genuine SLOW result is honestly < 1.0 (not hidden, not inflated): the
    # canonical board would mark this RED - that is correct, it is a real ratio.
    assert pa.safe_speedup(0.001, 0.1) == pytest.approx(0.01)


def test_safe_speedup_direction_is_speedup() -> None:
    # > 1.0 means molt faster (cpython_time / molt_time).
    assert pa.safe_speedup(2.0, 1.0) == pytest.approx(2.0)
    assert pa.safe_speedup(1.0, 2.0) == pytest.approx(0.5)


# --- non_canonical_provenance: numbers self-identify -------------------------


def test_non_canonical_stamp_is_never_authoritative() -> None:
    for source, profile in [
        ("tools/bench.py", "release-fast"),
        ("bench/harness.py", "dev"),
        ("bench/harness.py", "release"),
    ]:
        prov = pa.non_canonical_provenance(
            profile=profile, source=source, git_rev="deadbeef"
        )
        assert prov["authoritative"] is False
        assert prov["source"] == "non-canonical"
        assert prov["lane"] == source
        assert prov["profile"] == profile
        assert "perf_scoreboard.py" in str(prov["canonical_gate"])
        assert "release-fast" in str(prov["canonical_gate"])
        assert prov["git_rev"] == "deadbeef"
        # The reason must point a reader back at the canonical gate.
        assert "perf_scoreboard.py" in str(prov["authoritative_reason"])


def test_non_canonical_stamp_resolves_rev_when_omitted() -> None:
    prov = pa.non_canonical_provenance(profile="dev", source="bench/harness.py")
    # git_rev is auto-filled from HEAD (or "unknown" if git unavailable) -
    # never missing.
    assert isinstance(prov["git_rev"], str) and prov["git_rev"]


def test_canonical_gate_names_full_release_fast_backend_contract() -> None:
    gate = pa.CANONICAL_GATE

    for token in (
        "tools/perf_scoreboard.py",
        "--set core",
        "--backend native",
        "--backend llvm",
        "--profile release-fast",
        "--samples 5",
        "--warmup 2",
        "--repeat 5",
        "--classify",
        "--require-quiescent",
    ):
        assert token in gate
    assert "--no-gate" not in gate
    assert "--allow-nonauthoritative" not in gate


def test_perf_gate_workflow_runs_canonical_matrix_contract() -> None:
    workflow = (REPO_ROOT / ".github/workflows/perf-gate.yml").read_text(
        encoding="utf-8"
    )

    assert "backend: [native, llvm]" in workflow
    assert "fail-fast: false" in workflow
    assert "perfscore-${{ matrix.backend }}" in workflow
    assert '--backend "${{ matrix.backend }}"' in workflow
    for token in (
        "--set core",
        "--profile release-fast",
        "--samples 5",
        "--warmup 2",
        "--repeat 5",
        "--classify",
        "--require-quiescent",
        "--print-provenance",
    ):
        assert token in workflow
    assert "--no-gate" not in workflow
    assert "--allow-nonauthoritative" not in workflow
    assert "memory note" not in workflow
    assert "push-requires-ssh" not in workflow


def test_ci_docs_gate_runs_perf_freshness_and_authority_tests() -> None:
    workflow = (REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")

    assert "tools/check_perf_freshness.py" in workflow
    assert "tests/tools/test_perf_authority.py" in workflow


# --- freshness helpers -------------------------------------------------------


def test_doc_age_days_parses_common_forms() -> None:
    assert pa.doc_age_days(None) is None
    assert pa.doc_age_days("") is None
    assert pa.doc_age_days("not-a-date") is None
    # Date-only (hand-written docs) and full ISO with Z both parse.
    assert pa.doc_age_days("2026-03-20") is not None
    assert pa.doc_age_days("2026-03-20T10:22:49Z") is not None
    assert pa.doc_age_days("2026-03-20T10:22:49+00:00") is not None


def test_doc_age_days_is_monotonic() -> None:
    import datetime as dt

    now = dt.datetime(2026, 6, 25, tzinfo=dt.timezone.utc)
    old = pa.doc_age_days("2026-03-20", now=now)
    recent = pa.doc_age_days("2026-06-24", now=now)
    assert old is not None and recent is not None
    assert old > recent > 0


def test_stale_banner_self_identifies() -> None:
    banner = pa.STALE_BANNER(generated_at="2026-03-20", git_rev="abc123")
    assert pa.STALE_BANNER_MARK in banner
    assert "NOT AUTHORITATIVE" in banner
    assert "perf_scoreboard.py" in banner
    assert "2026-03-20" in banner
    assert "abc123" in banner


def test_git_ancestor_unknown_rev_is_undeterminable() -> None:
    # An unknown/empty rev cannot be proven an ancestor - returns None, not a
    # false "fresh".
    assert pa.git_rev_is_ancestor_of_origin(None) is None
    assert pa.git_rev_is_ancestor_of_origin("") is None
    assert pa.git_rev_is_ancestor_of_origin("unknown") is None


# --- check_perf_freshness gate: fail-closed on stale/undateable --------------

import datetime as _dt  # noqa: E402


_NOW = _dt.datetime(2026, 6, 25, tzinfo=_dt.timezone.utc)


def _eval(tmp_path: Path, name: str, body: str) -> dict:
    p = tmp_path / name
    p.write_text(body, encoding="utf-8")
    return freshness.evaluate_doc(p, max_age_days=30.0, now=_NOW)


def test_freshness_no_perf_numbers_is_not_a_hazard(tmp_path: Path) -> None:
    rec = _eval(
        tmp_path,
        "rootcause.md",
        "# Root cause\n\nWe expected a 20-40% speedup from this change.\n",
    )
    # Bare prose mention of "speedup" is NOT a citable number.
    assert rec["has_perf_numbers"] is False
    assert rec["hazard"] is False
    assert rec["verdict"] == "no-perf-numbers"


def test_freshness_undateable_numeric_doc_is_hazard(tmp_path: Path) -> None:
    rec = _eval(
        tmp_path,
        "abandoned.md",
        "# Old bench\n\n| bench_fib.py | 0.01x |\n",
    )
    # Citable ratio, no date, no rev, not stamped -> fail-closed hazard.
    assert rec["has_perf_numbers"] is True
    assert rec["hazard"] is True
    assert rec["verdict"] == "stale-hazard"


def test_freshness_stamped_doc_clears_hazard(tmp_path: Path) -> None:
    body = (
        pa.STALE_BANNER(generated_at="2026-01-01", git_rev="unknown")
        + "\n# Old bench\n\n| bench_fib.py | 0.01x |\n"
    )
    rec = _eval(tmp_path, "stamped.md", body)
    assert rec["has_perf_numbers"] is True
    assert rec["hazard"] is False
    assert rec["verdict"] == "stale-stamped"


def test_freshness_old_dated_doc_is_hazard(tmp_path: Path) -> None:
    rec = _eval(
        tmp_path,
        "dated.md",
        "# Bench\n\ngenerated_at: 2026-01-01\n\n| bench_fib.py | 0.01x |\n",
    )
    assert rec["hazard"] is True
    assert any("old" in r for r in rec["reasons"])


def test_live_repo_perf_docs_are_fresh_or_stamped() -> None:
    # The actual repo gate must be GREEN: every perf doc presenting citable
    # numbers is either fresh or stamped stale. This keeps the canonicalization
    # enforced - a new unstamped stale doc fails this test.
    report = freshness.run(pa.DEFAULT_STALE_DAYS)
    hazards = [r["path"] for r in report["records"] if r["hazard"]]
    assert hazards == [], f"unstamped-stale perf docs present: {hazards}"
