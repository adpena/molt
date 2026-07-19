"""Tests for the molt subagent-dispatch contract apparatus (APPARATUS A7).

Proves three things the ask requires:
  (a) compose() output CONTAINS each requested constant's text;
  (b) the integrity gate FAILS on a synthetic deletion/blank and PASSES on the real module;
  (c) an example composed prompt for a real molt lane shape renders correctly.

Sources: docs/agent/APPARATUS_FROM_COMMA_LAB.md A7 + 1.10; the constants mechanize
M05/M06/M12/M14/M17-M20/M23/M28/M33/M62/M63/M70.
"""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import sys
import types
from pathlib import Path

import pytest

from tools import check_subagent_contract as gate
from tools import dispatch_prompt as dp
from tools import subagent_contract as sc

_REPO = Path(__file__).resolve().parents[2]


# --- (module invariants) -------------------------------------------------------------------


@pytest.mark.parametrize("name", sc.CONTRACT_CONSTANT_NAMES)
def test_constant_non_empty_and_keeps_key_phrase(name: str) -> None:
    value = getattr(sc, name)
    assert isinstance(value, str) and value.strip(), f"{name} must be a non-empty string"
    assert sc.KEY_PHRASES[name] in value, f"{name} lost its key phrase"


def test_registries_cover_exactly_the_constants() -> None:
    names = set(sc.CONTRACT_CONSTANT_NAMES)
    assert set(sc.KEY_PHRASES) == names
    assert set(sc.MECHANIZES) == names
    # Every block documents a real M## anchor.
    for name, anchor in sc.MECHANIZES.items():
        assert anchor.strip() and "M" in anchor, f"{name} has no M## anchor"


def test_gate_required_set_matches_module_registry() -> None:
    # The gate's hardcoded authority and the module registry must agree in the healthy repo
    # (they are deliberately independent copies; a genuine add/remove updates both).
    assert set(gate.REQUIRED_CONSTANTS) == set(sc.CONTRACT_CONSTANT_NAMES)


# --- (a) compose() contains each requested constant's text ---------------------------------


def test_compose_all_contains_every_block() -> None:
    prompt = dp.compose("TASK BODY", sections=None)
    assert prompt.startswith("TASK BODY")
    for name in sc.CONTRACT_CONSTANT_NAMES:
        assert getattr(sc, name) in prompt, f"compose(all) missing {name}"


def test_compose_subset_contains_only_requested_blocks() -> None:
    prompt = dp.compose("TASK BODY", sections=["landing", "web", "git", "verify"])
    # 'verify' expands to two constants; all four keys -> five blocks.
    for name in ("LANDING_CONTRACT", "WEB_AUTHORITY", "GIT_CUSTODY",
                 "VERIFY_FULL_SUITE", "FRESH_CONTEXT_VERIFIER"):
        assert getattr(sc, name) in prompt, f"requested block {name} missing"
    for name in ("NO_FAKES", "MODEL_TIER", "DISTILL", "BUILD_ENV",
                 "GROUNDED_PROGRESS", "NO_ENDING_ON_PROMISES", "TRIALITY_WIRING"):
        assert getattr(sc, name) not in prompt, f"unrequested block {name} leaked in"


def test_compose_is_order_stable_regardless_of_key_order() -> None:
    a = dp.compose("T", sections=["git", "web", "landing"])
    b = dp.compose("T", sections=["landing", "web", "git"])
    assert a == b, "composed prompt must be canonical-ordered, not typed-order"


def test_compose_verify_block_is_the_recent_miss() -> None:
    # The exact clause a recent lane was missing must be present when 'verify' is selected.
    prompt = dp.compose("T", sections=["verify"])
    assert "cargo test -p <crate>" in prompt
    assert "NOT a single narrow gate" in prompt


def test_compose_unknown_section_fails_loud() -> None:
    with pytest.raises(KeyError):
        dp.compose("T", sections=["landing", "not-a-section"])


def test_standard_contract_rejects_unknown_name() -> None:
    with pytest.raises(KeyError):
        sc.standard_contract(("GROUNDED_PROGRESS", "NOPE"))


# --- (b) the integrity gate has TEETH ------------------------------------------------------


def test_gate_passes_on_the_real_module() -> None:
    result = gate.audit()
    assert result.ok, f"real module must pass integrity: {result.failures}"
    assert result.failures == []


def _clone_module_namespace() -> types.SimpleNamespace:
    """A mutable stand-in carrying every real contract attribute the gate reads."""
    ns = types.SimpleNamespace()
    for name in sc.CONTRACT_CONSTANT_NAMES:
        setattr(ns, name, getattr(sc, name))
    ns.CONTRACT_CONSTANT_NAMES = tuple(sc.CONTRACT_CONSTANT_NAMES)
    ns.MECHANIZES = dict(sc.MECHANIZES)
    return ns


def test_gate_fails_on_deleted_constant() -> None:
    ns = _clone_module_namespace()
    delattr(ns, "VERIFY_FULL_SUITE")  # synthetic removal of the recent-miss block
    result = gate.audit(ns)
    assert not result.ok
    assert any("VERIFY_FULL_SUITE" in f and "MISSING" in f for f in result.failures)


def test_gate_fails_on_blanked_constant() -> None:
    ns = _clone_module_namespace()
    ns.NO_FAKES = "   "  # blanked to whitespace
    result = gate.audit(ns)
    assert not result.ok
    assert any("NO_FAKES" in f for f in result.failures)


def test_gate_fails_on_lost_key_phrase() -> None:
    ns = _clone_module_namespace()
    ns.GIT_CUSTODY = "Use git carefully."  # non-empty but drops the load-bearing clause
    result = gate.audit(ns)
    assert not result.ok
    assert any("GIT_CUSTODY" in f and "key phrase" in f for f in result.failures)


def test_gate_fails_on_emptied_registry() -> None:
    # Emptying the module's own registry to hide a deletion must not self-waive the gate.
    ns = _clone_module_namespace()
    ns.CONTRACT_CONSTANT_NAMES = ()
    result = gate.audit(ns)
    assert not result.ok
    assert any("registry" in f for f in result.failures)


def test_gate_fails_on_missing_mechanizes_anchor() -> None:
    ns = _clone_module_namespace()
    ns.MECHANIZES = {k: v for k, v in sc.MECHANIZES.items() if k != "MODEL_TIER"}
    result = gate.audit(ns)
    assert not result.ok
    assert any("MODEL_TIER" in f and "MECHANIZES" in f for f in result.failures)


def test_gate_cli_exit_codes() -> None:
    # PASS -> exit 0 on the real repo.
    proc = run_guarded_test_process(
        [sys.executable, str(_REPO / "tools" / "check_subagent_contract.py")],
        capture_output=True, text=True, cwd=str(_REPO),
    )
    assert proc.returncode == 0, proc.stdout + proc.stderr
    assert "PASS" in proc.stdout


# --- (c) an example composed prompt for a real molt lane renders correctly ------------------


def test_example_real_lane_prompt_renders() -> None:
    task = (
        "Fix the fn-level `<loop-IV> % <int-const>` NaN-box stringification bug (M36): "
        "reconcile the carrier disagreement in value_range.rs / arith_division.rs / "
        "scalar_carriers.rs so the result stringifies as its integer value, not raw i64 bits."
    )
    prompt = dp.compose(task, sections=["verify", "fakes", "git", "build", "landing", "triality"])
    # Task body first, then the selected blocks, blank-line separated.
    assert prompt.startswith(task)
    assert "\n\n" in prompt
    # The lane inherits the exact clauses it needs, mechanically:
    assert "cargo test -p <crate>" in prompt          # VERIFY_FULL_SUITE (M05)
    assert "zero stubs, zero theater" in prompt        # NO_FAKES (M05)
    assert "NEVER `git add -A`" in prompt              # GIT_CUSTODY
    assert "never change source, seal, worktree, or toolchain custody" in prompt
    assert "every work turn lands something real" in prompt  # LANDING_CONTRACT (M12)
    assert "all three legs" in prompt                  # TRIALITY_WIRING (M63)
    # And does NOT carry the blocks it did not request.
    assert "Fable orchestrates" not in prompt          # MODEL_TIER not selected
