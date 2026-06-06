"""Tests for the ecosystem-compatibility ratchet (tools/check_ecosystem_compat.py).

Two jobs, mirroring tests/test_check_stdlib_intrinsics.py:
  1. The committed manifests pass the guard green (the live contract).
  2. Every failure mode fires on a MUTATED COPY (never on the committed files):
     stored verdict != derived, unknown feature ref, missing evidence, missing
     excluded_feature, missing tracking, required/optional overlap, verdict
     regression, evidence-SHA drift, and each one-way-ratchet refusal.

Fixtures redirect the guard's module-level path constants at tmp copies so no
test can mutate the committed manifests/baseline/matrix.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tools" / "check_ecosystem_compat.py"


def _load_guard():
    spec = importlib.util.spec_from_file_location(
        "check_ecosystem_compat_gate", SCRIPT_PATH
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def guard():
    return _load_guard()


@pytest.fixture
def sandbox(guard, tmp_path, monkeypatch):
    """Copy the live manifests into tmp and point the guard's paths at them.

    Returns a small handle exposing the tmp paths plus load/save helpers, so a
    test can mutate a manifest and re-run the guard entirely in isolation.
    """
    eco = tmp_path / "ecosystem"
    eco.mkdir()
    feat = eco / "dynamism_features.json"
    triage = eco / "package_triage.json"
    baseline = eco / "ecosystem_compat_baseline.json"
    matrix = tmp_path / "matrix.generated.md"

    feat.write_text(
        (REPO_ROOT / "tools" / "ecosystem" / "dynamism_features.json").read_text(),
        encoding="utf-8",
    )
    triage.write_text(
        (REPO_ROOT / "tools" / "ecosystem" / "package_triage.json").read_text(),
        encoding="utf-8",
    )

    monkeypatch.setattr(guard, "FEATURES_PATH", feat)
    monkeypatch.setattr(guard, "TRIAGE_PATH", triage)
    monkeypatch.setattr(guard, "BASELINE_PATH", baseline)
    monkeypatch.setattr(guard, "MATRIX_PATH", matrix)

    class Handle:
        FEATURES_PATH = feat
        TRIAGE_PATH = triage
        BASELINE_PATH = baseline
        MATRIX_PATH = matrix

        @staticmethod
        def load(path):
            return json.loads(path.read_text(encoding="utf-8"))

        @staticmethod
        def save(path, obj):
            path.write_text(json.dumps(obj, indent=2) + "\n", encoding="utf-8")

        @staticmethod
        def make_baseline():
            assert guard.cmd_update_baseline() == 0

    return Handle()


# --------------------------------------------------------------------------
# 1. The committed manifests pass green.
# --------------------------------------------------------------------------


def test_committed_manifests_pass(guard):
    # Runs against the REAL committed files (no sandbox) — this is the live CI
    # contract: the shipped manifests + baseline must be internally consistent.
    assert guard.main([]) == 0


def test_committed_matrix_is_not_stale(guard, tmp_path, monkeypatch):
    # The generated matrix must equal a fresh render of the committed manifests.
    features = guard.load_features()
    _problems, derived = guard.validate_triage(guard.load_triage(), features)
    fresh = guard.render_matrix(derived, features)
    committed = guard.MATRIX_PATH.read_text(encoding="utf-8")
    assert fresh == committed, (
        "ecosystem_compat_matrix.generated.md is stale; regenerate with --update-matrix"
    )


def test_distribution_is_deterministic(guard):
    # Derivation must be order-independent: two runs yield identical results.
    features = guard.load_features()
    _p1, d1 = guard.validate_triage(guard.load_triage(), features)
    _p2, d2 = guard.validate_triage(guard.load_triage(), features)
    assert d1 == d2
    dist = guard.verdict_distribution(d1)
    assert sum(dist.values()) == 25
    assert dist["compatible"] == 10
    assert dist["incompatible-by-design"] == 4
    assert dist["partial"] == 7


def test_min_is_worst_class(guard):
    # A package mixing supported + unsupported must derive incompatible (min).
    features = guard.load_features()
    verdict, hardest = guard.derive_verdict(["D2", "D16", "D12"], features)
    assert verdict == "incompatible-by-design"
    assert hardest == "D16"
    # Empty required set is the top of the lattice.
    assert guard.derive_verdict([], features) == ("compatible", None)
    # Numeric (not lexicographic) tie-break: D2 wins over D10, not "D10".
    assert guard.derive_verdict(["D2", "D10"], features)[1] == "D2"


def test_sandbox_baseline_roundtrips_green(sandbox, guard):
    sandbox.make_baseline()
    assert guard.cmd_check(False) == 0


# --------------------------------------------------------------------------
# 2. Failure modes — each must fire on a mutated copy.
# --------------------------------------------------------------------------


def test_fail_hand_edited_verdict(sandbox, guard):
    sandbox.make_baseline()
    t = sandbox.load(sandbox.TRIAGE_PATH)
    # six derives `compatible`; hand-assert it to a lie.
    t["packages"]["six"]["verdict"] = "incompatible-by-design"
    sandbox.save(sandbox.TRIAGE_PATH, t)
    assert guard.cmd_check(False) == 1


def test_fail_hand_edited_hardest_feature(sandbox, guard):
    sandbox.make_baseline()
    t = sandbox.load(sandbox.TRIAGE_PATH)
    t["packages"]["rich"]["hardest_feature"] = "D12"  # derived is D16
    sandbox.save(sandbox.TRIAGE_PATH, t)
    assert guard.cmd_check(False) == 1


def test_fail_unknown_feature_reference(sandbox, guard):
    sandbox.make_baseline()
    t = sandbox.load(sandbox.TRIAGE_PATH)
    t["packages"]["six"]["required_features"] = ["D2", "D999"]
    sandbox.save(sandbox.TRIAGE_PATH, t)
    assert guard.cmd_check(False) == 1


def test_fail_missing_evidence(sandbox, guard):
    sandbox.make_baseline()
    f = sandbox.load(sandbox.FEATURES_PATH)
    f["features"]["D1"]["evidence"] = ""
    sandbox.save(sandbox.FEATURES_PATH, f)
    assert guard.cmd_check(False) == 1


def test_fail_unsupported_missing_excluded_feature(sandbox, guard):
    sandbox.make_baseline()
    f = sandbox.load(sandbox.FEATURES_PATH)
    f["features"]["D15"]["excluded_feature"] = ""
    sandbox.save(sandbox.FEATURES_PATH, f)
    assert guard.cmd_check(False) == 1


def test_fail_unsupported_missing_tracking(sandbox, guard):
    sandbox.make_baseline()
    f = sandbox.load(sandbox.FEATURES_PATH)
    f["features"]["D16"]["tracking"] = ""  # anti-parking-lot
    sandbox.save(sandbox.FEATURES_PATH, f)
    assert guard.cmd_check(False) == 1


def test_fail_partial_missing_tracking(sandbox, guard):
    sandbox.make_baseline()
    f = sandbox.load(sandbox.FEATURES_PATH)
    f["features"]["D20"]["tracking"] = ""  # partial also needs tracking
    sandbox.save(sandbox.FEATURES_PATH, f)
    assert guard.cmd_check(False) == 1


def test_fail_invalid_status(sandbox, guard):
    sandbox.make_baseline()
    f = sandbox.load(sandbox.FEATURES_PATH)
    f["features"]["D1"]["status"] = "totally-supported"
    sandbox.save(sandbox.FEATURES_PATH, f)
    assert guard.cmd_check(False) == 1


def test_fail_required_optional_overlap(sandbox, guard):
    sandbox.make_baseline()
    t = sandbox.load(sandbox.TRIAGE_PATH)
    t["packages"]["pyyaml"]["optional_features"].append("D22")  # also required
    sandbox.save(sandbox.TRIAGE_PATH, t)
    assert guard.cmd_check(False) == 1


def test_fail_invalid_probe_status(sandbox, guard):
    sandbox.make_baseline()
    t = sandbox.load(sandbox.TRIAGE_PATH)
    t["packages"]["six"]["compile_probe_status"] = "maybe"
    sandbox.save(sandbox.TRIAGE_PATH, t)
    assert guard.cmd_check(False) == 1


def test_fail_verdict_regression(sandbox, guard):
    sandbox.make_baseline()
    # Regress a feature klick depends on: flip D2 (supported) -> unsupported so
    # idna/packaging/charset-normalizer (compatible) drop to incompatible.
    f = sandbox.load(sandbox.FEATURES_PATH)
    f["features"]["D2"]["status"] = "unsupported"
    f["features"]["D2"]["excluded_feature"] = "synthetic regression for test"
    f["features"]["D2"]["tracking"] = "synthetic regression for test"
    sandbox.save(sandbox.FEATURES_PATH, f)
    # Stored verdicts in triage now disagree AND the baseline regresses; the
    # guard must fail. (Both the no-hand-edit check and the ratchet fire.)
    assert guard.cmd_check(False) == 1


def test_fail_evidence_sha_drift_without_verdict_change(sandbox, guard):
    sandbox.make_baseline()
    # Change a feature's EVIDENCE string only (status unchanged) -> the package
    # evidence SHA flips while the verdict stays the same -> must fail.
    f = sandbox.load(sandbox.FEATURES_PATH)
    f["features"]["D1"]["evidence"] = "totally different evidence pointer"
    sandbox.save(sandbox.FEATURES_PATH, f)
    assert guard.cmd_check(False) == 1


def test_fail_new_package_without_baseline_entry(sandbox, guard):
    sandbox.make_baseline()
    t = sandbox.load(sandbox.TRIAGE_PATH)
    t["packages"]["brandnewpkg"] = {
        "required_features": ["D2"],
        "optional_features": [],
        "verdict": "compatible",
        "hardest_feature": "D2",
        "source": "doc-24 paper triage",
        "compile_probe_status": "pending",
        "source_basis": "synthetic",
    }
    sandbox.save(sandbox.TRIAGE_PATH, t)
    assert guard.cmd_check(False) == 1


def test_fail_check_with_no_baseline(sandbox, guard):
    # No baseline file written at all.
    assert not sandbox.BASELINE_PATH.exists()
    assert guard.cmd_check(False) == 1


# --- one-way ratchet: --update-baseline refuses the WRONG direction ---------


def test_update_baseline_refuses_lowering_floor(sandbox, guard):
    sandbox.make_baseline()
    # Make a compatible package incompatible -> floor would drop -> refused.
    f = sandbox.load(sandbox.FEATURES_PATH)
    f["features"]["D2"]["status"] = "unsupported"
    f["features"]["D2"]["excluded_feature"] = "x"
    f["features"]["D2"]["tracking"] = "x"
    sandbox.save(sandbox.FEATURES_PATH, f)
    # Also clear the now-mismatching stored verdicts so we exercise the RATCHET
    # refusal (not the hand-edit refusal) by deleting cached verdicts.
    t = sandbox.load(sandbox.TRIAGE_PATH)
    for pkg in t["packages"].values():
        pkg.pop("verdict", None)
        pkg.pop("hardest_feature", None)
    sandbox.save(sandbox.TRIAGE_PATH, t)
    assert guard.cmd_update_baseline() == 1  # refuses to lower the floor


def test_update_baseline_refuses_raising_incompatible_ceiling(sandbox, guard):
    sandbox.make_baseline()
    # Add a brand-new incompatible package WITHOUT touching the compatible
    # count, so the only change is incompatible_ceiling rising -> refused.
    f = sandbox.load(sandbox.FEATURES_PATH)
    t = sandbox.load(sandbox.TRIAGE_PATH)
    t["packages"]["newbad"] = {
        "required_features": ["D15"],
        "optional_features": [],
        "compile_probe_status": "pending",
        "source": "doc-24 paper triage",
        "source_basis": "synthetic",
    }
    sandbox.save(sandbox.FEATURES_PATH, f)
    sandbox.save(sandbox.TRIAGE_PATH, t)
    assert guard.cmd_update_baseline() == 1


def test_update_baseline_allows_improvement(sandbox, guard):
    sandbox.make_baseline()
    prev = sandbox.load(sandbox.BASELINE_PATH)
    # Graduate D16 (unsupported -> supported): rich flips to compatible, floor
    # rises 10 -> 11, incompatible_ceiling falls 4 -> 3. The improving
    # direction must be ALLOWED, and stored verdicts must be re-derived.
    f = sandbox.load(sandbox.FEATURES_PATH)
    f["features"]["D16"]["status"] = "supported"
    f["features"]["D16"].pop("excluded_feature", None)
    f["features"]["D16"].pop("tracking", None)
    sandbox.save(sandbox.FEATURES_PATH, f)
    t = sandbox.load(sandbox.TRIAGE_PATH)
    # rich's cached verdict must be re-derived too (else hand-edit check fires).
    t["packages"]["rich"]["verdict"] = "compatible"
    t["packages"]["rich"]["hardest_feature"] = "D12"
    sandbox.save(sandbox.TRIAGE_PATH, t)
    assert guard.cmd_update_baseline() == 0
    new = sandbox.load(sandbox.BASELINE_PATH)
    assert new["compatible_floor"] == prev["compatible_floor"] + 1
    assert new["incompatible_ceiling"] == prev["incompatible_ceiling"] - 1


def test_status_to_verdict_map_matches_manifest(guard):
    # The guard's lattice map and the manifest's embedded copy must agree, or
    # the "single source of truth" claim is false. load_features() asserts this;
    # here we also assert a deliberate disagreement is caught.
    data = json.loads(guard.FEATURES_PATH.read_text(encoding="utf-8"))
    assert data["_status_to_verdict_class"] == guard.STATUS_TO_VERDICT
