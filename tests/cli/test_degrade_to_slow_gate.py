"""Tests for the degrade-to-slow enforcement gate (tools/degrade_to_slow_gate.py).

Covers:
  * the gate is GREEN on the checked-in seeded registry (the real tree);
  * the gate's classification/ratchet/anchor/teeth invariants hold on the seed;
  * a NEGATIVE CONTROL that mutation-proves the drift teeth: injecting a
    synthetic unregistered ``reason="foo_fallback_serial"`` degrade site into a
    scanned root makes the gate FAIL, and removing it makes the gate PASS again.
    This proves the scanner actually detects new silent-degrade sites -- a gate
    that cannot fail on a real violation is theater.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

_REPO_ROOT = Path(__file__).resolve().parents[2]
_GATE_PATH = _REPO_ROOT / "tools" / "degrade_to_slow_gate.py"


def _load_gate_module():
    """Import tools/degrade_to_slow_gate.py as a module (it lives outside a
    package). Fresh import each call so REPO_ROOT/REGISTRY_PATH can be rebound
    per test without cross-test leakage."""
    mod_name = "_degrade_gate_under_test"
    spec = importlib.util.spec_from_file_location(mod_name, _GATE_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # Register before exec so @dataclass field resolution (which looks up
    # sys.modules[cls.__module__]) can find the module.
    sys.modules[mod_name] = module
    try:
        spec.loader.exec_module(module)
    except Exception:
        sys.modules.pop(mod_name, None)
        raise
    return module


# ---------------------------------------------------------------------------
# The gate is green on the real, checked-in seed.
# ---------------------------------------------------------------------------
def test_gate_is_green_on_seeded_registry() -> None:
    gate = _load_gate_module()
    report = gate.run_gate()
    assert report.ok, "seeded registry must pass the gate; errors:\n" + "\n".join(
        report.errors
    )
    # Every row is a valid classification and the pending count honors the ratchet.
    assert report.metabug_fix_pending_count <= report.metabug_fix_pending_baseline
    assert report.registry_row_count > 0
    assert report.discovered_site_count > 0


def test_seed_registry_rows_are_well_formed() -> None:
    gate = _load_gate_module()
    rows, baseline = gate.load_registry()
    assert baseline >= 0
    for row in rows:
        assert row.classification in gate.VALID_CLASSIFICATIONS
        assert row.justification.strip(), f"{row.file} has empty justification"
        # Non-sound rows must carry a fast_path_test.
        if row.classification in gate.NON_SOUND:
            assert row.fast_path_test, (
                f"{row.classification} row {row.file} must name a fast_path_test"
            )


def test_every_discovered_site_is_registered_in_seed() -> None:
    """Drift invariant on the real tree: no discovered degrade site lacks a row."""
    gate = _load_gate_module()
    rows, _ = gate.load_registry()
    registered_files = {r.file for r in rows}
    unregistered = sorted(
        {s.file for s in gate.discover_sites()} - registered_files
    )
    assert not unregistered, (
        "discovered degrade sites with no registry row: " + ", ".join(unregistered)
    )


# ---------------------------------------------------------------------------
# Negative control: mutation-prove the drift teeth.
# ---------------------------------------------------------------------------
def _write_synthetic_tree(root: Path, *, with_degrade: bool) -> Path:
    """Build a minimal fake repo the gate can scan. One scanned root
    (src/molt/cli) with a source file that optionally contains an unregistered
    degrade signature. A zero-row registry so anchor/ratchet checks are trivially
    satisfied and only the drift teeth are exercised."""
    cli_dir = root / "src" / "molt" / "cli"
    cli_dir.mkdir(parents=True, exist_ok=True)
    body = "def do_work(mode):\n"
    if with_degrade:
        # Assignment context so the decision-context guard counts it.
        body += '    reason = "foo_fallback_serial"\n'
        body += "    return reason\n"
    else:
        body += "    return mode\n"
    (cli_dir / "synthetic_mod.py").write_text(body, encoding="utf-8")

    registry = root / "tools" / "degrade_to_slow_registry.toml"
    registry.parent.mkdir(parents=True, exist_ok=True)
    registry.write_text(
        "[meta]\nmetabug_fix_pending_baseline = 0\n", encoding="utf-8"
    )
    return registry


def test_negative_control_unregistered_degrade_site_fails_then_passes(
    tmp_path: Path,
) -> None:
    gate = _load_gate_module()

    # --- inject a synthetic unregistered degrade site -> gate FAILS ---------
    registry = _write_synthetic_tree(tmp_path, with_degrade=True)

    # Sanity: the scanner actually sees the synthetic site.
    discovered = gate.discover_sites(tmp_path, gate.SCAN_ROOTS)
    assert any(
        s.file == "src/molt/cli/synthetic_mod.py"
        and "foo_fallback_serial" in s.marker
        for s in discovered
    ), "scanner failed to discover the injected degrade signature"

    report_fail = gate.run_gate(registry, repo_root=tmp_path)
    assert not report_fail.ok, (
        "gate MUST fail on an unregistered synthetic degrade site (drift teeth)"
    )
    assert any(
        "synthetic_mod.py" in e and "UNREGISTERED" in e for e in report_fail.errors
    ), f"expected an UNREGISTERED failure, got: {report_fail.errors}"

    # --- remove the degrade signature -> gate PASSES -----------------------
    _write_synthetic_tree(tmp_path, with_degrade=False)
    report_pass = gate.run_gate(registry, repo_root=tmp_path)
    assert report_pass.ok, (
        "gate must pass once the unregistered degrade site is removed; errors:\n"
        + "\n".join(report_pass.errors)
    )
    assert report_pass.discovered_site_count == 0


def test_telemetry_dictionary_keys_are_not_degrade_decisions(tmp_path: Path) -> None:
    gate = _load_gate_module()
    cli_dir = tmp_path / "src" / "molt" / "cli"
    cli_dir.mkdir(parents=True, exist_ok=True)
    (cli_dir / "telemetry.py").write_text(
        "COUNTERS = {\n"
        '    "cache_hits": 0,\n'
        '    "native_support_slice_cache_misses": 0,\n'
        "}\n",
        encoding="utf-8",
    )

    assert gate.discover_sites(tmp_path, gate.SCAN_ROOTS) == []


def test_negative_control_ratchet_regression_fails(tmp_path: Path) -> None:
    """A metabug_fix_pending row above the baseline fails the ratchet."""
    gate = _load_gate_module()
    cli_dir = tmp_path / "src" / "molt" / "cli"
    cli_dir.mkdir(parents=True, exist_ok=True)
    # A real file+anchor so the pending row's anchor check passes; the FAILURE
    # under test is the ratchet, not a missing anchor.
    (cli_dir / "pending_mod.py").write_text(
        "def slow_path():\n    return 1\n", encoding="utf-8"
    )
    registry = tmp_path / "tools" / "degrade_to_slow_registry.toml"
    registry.parent.mkdir(parents=True, exist_ok=True)
    registry.write_text(
        "[meta]\n"
        "metabug_fix_pending_baseline = 0\n\n"
        "[[site]]\n"
        'file = "src/molt/cli/pending_mod.py"\n'
        'symbol = "slow_path"\n'
        'classification = "metabug_fix_pending"\n'
        'justification = "synthetic pending row over baseline"\n'
        'fast_path_test = "test_some_future_fast_path"\n',
        encoding="utf-8",
    )
    report = gate.run_gate(registry, repo_root=tmp_path)
    assert not report.ok
    assert any("ratchet regression" in e for e in report.errors), report.errors


def test_negative_control_make_loud_without_diagnostic_fails(
    tmp_path: Path,
) -> None:
    """A make_loud row whose file reaches NO diagnostic emit fails the teeth."""
    gate = _load_gate_module()
    cli_dir = tmp_path / "src" / "molt" / "cli"
    cli_dir.mkdir(parents=True, exist_ok=True)
    # A file that silently degrades with NO warnings.append/logger/print.
    (cli_dir / "silent_mod.py").write_text(
        "def maybe_fallback(x):\n    return x  # no diagnostic anywhere\n",
        encoding="utf-8",
    )
    # The proving test exists so the fast_path_test check is not the failure.
    tests_dir = tmp_path / "tests"
    tests_dir.mkdir(parents=True, exist_ok=True)
    (tests_dir / "test_proof.py").write_text(
        "def test_proof_loud():\n    assert True\n", encoding="utf-8"
    )
    registry = tmp_path / "tools" / "degrade_to_slow_registry.toml"
    registry.parent.mkdir(parents=True, exist_ok=True)
    registry.write_text(
        "[meta]\n"
        "metabug_fix_pending_baseline = 0\n\n"
        "[[site]]\n"
        'file = "src/molt/cli/silent_mod.py"\n'
        'symbol = "maybe_fallback"\n'
        'classification = "make_loud"\n'
        'justification = "synthetic make_loud with no diagnostic"\n'
        'fast_path_test = "test_proof_loud"\n',
        encoding="utf-8",
    )
    report = gate.run_gate(registry, repo_root=tmp_path)
    assert not report.ok
    assert any(
        "reaches NO diagnostic emit" in e for e in report.errors
    ), report.errors


if __name__ == "__main__":  # pragma: no cover
    sys.exit(pytest.main([__file__, "-q"]))
