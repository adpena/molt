"""Tests for the molt subagent-dispatch contract apparatus (APPARATUS A7).

Proves three things the ask requires:
  (a) compose() output CONTAINS each requested constant's text;
  (b) the integrity gate FAILS on a synthetic deletion/blank and PASSES on the real module;
  (c) an example composed prompt for a real molt lane shape renders correctly.

Sources: docs/agent/APPARATUS_FROM_COMMA_LAB.md A7 + 1.10; the constants mechanize
M05/M06/M12/M14/M17-M20/M23/M28/M33/M62/M63/M70.
"""

from __future__ import annotations
from dataclasses import asdict, replace
import json
from tests.process_guard_common import run_guarded_test_process

import sys
import types
from pathlib import Path

import pytest

from tools import check_subagent_contract as gate
from tools import dispatch_prompt as dp
from tools import subagent_contract as sc

_REPO = Path(__file__).resolve().parents[2]
_BOUNDS = dp.DispatchBounds(
    outcome="Reduce end-to-end edit-to-result latency",
    deliverable="Identify the dominant repeated harness cost and return evidence",
    integration_owner="parent",
    proof_owner="parent",
    owned_paths=(),
    reserved_paths=(),
    proof_commands=(),
    budget_minutes=5,
    stop_when="The retained profile identifies the dominant cost or evidence is insufficient",
)


@pytest.mark.parametrize(
    "field",
    [
        "outcome",
        "deliverable",
        "integration_owner",
        "proof_owner",
        "owned_paths",
        "reserved_paths",
        "proof_commands",
        "budget_minutes",
        "stop_when",
    ],
)
def test_dispatch_refuses_each_missing_bound(field: str) -> None:
    payload = json.loads(json.dumps(asdict(_BOUNDS)))
    del payload[field]
    with pytest.raises((TypeError, ValueError)):
        dp.DispatchBounds.from_payload(payload)


@pytest.mark.parametrize("budget", [0, -1, True, 1.5, "5"])
def test_dispatch_refuses_invalid_budget(budget) -> None:
    payload = json.loads(json.dumps(asdict(_BOUNDS)))
    payload["budget_minutes"] = budget
    with pytest.raises(ValueError, match="positive integer"):
        dp.DispatchBounds.from_payload(payload)


@pytest.mark.parametrize(
    "field",
    [
        "outcome",
        "deliverable",
        "integration_owner",
        "proof_owner",
        "stop_when",
    ],
)
def test_dispatch_refuses_empty_assignment(field: str) -> None:
    payload = json.loads(json.dumps(asdict(_BOUNDS)))
    payload[field] = " "
    with pytest.raises(ValueError, match=field):
        dp.DispatchBounds.from_payload(payload)


def test_bounds_are_required_even_without_routing_sections() -> None:
    with pytest.raises(TypeError, match="bounds"):
        dp.compose("Inspect retained profile", sections=[])
    prompt = dp.compose("Inspect retained profile", bounds=_BOUNDS, sections=[])
    assert "Starting aperture: read only" in prompt
    assert "This read-only assignment authorizes no source changes" in prompt
    assert "Additional proof commands: []" in prompt
    assert "Proof owner: parent" in prompt
    assert "Work budget: 5 minutes" in prompt
    assert "Return only for a reserved path" in prompt
    assert "AGENTS.md" in prompt


def test_cli_requires_and_renders_explicit_bounds(tmp_path, capsys) -> None:
    with pytest.raises(SystemExit) as error:
        dp.main(["--task-text", "Inspect retained profile"])
    assert error.value.code == 2
    path = tmp_path / "bounds.json"
    path.write_text(json.dumps(asdict(_BOUNDS)), encoding="utf-8")
    assert (
        dp.main(["--task-text", "Inspect retained profile", "--bounds", str(path)]) == 0
    )
    assert (
        "Assigned deliverable: Identify the dominant repeated harness cost"
        in capsys.readouterr().out
    )


@pytest.mark.parametrize("sections", [None, []])
def test_implementation_brief_extends_to_canonical_owners(sections) -> None:
    bounds = replace(
        _BOUNDS,
        owned_paths=("tools/dispatch_prompt.py",),
        reserved_paths=("runtime/molt-runtime/src/arena.rs",),
    )
    prompt = dp.compose("Fix dispatch scope", bounds=bounds, sections=sections)
    assert 'Starting aperture: ["tools/dispatch_prompt.py"]' in prompt
    assert (
        'Reserved paths (held by other lanes): ["runtime/molt-runtime/src/arena.rs"]'
        in prompt
    )
    assert "Reserved paths remain excluded, including their descendants" in prompt
    assert "Extend to the canonical owners of each invariant you change" in prompt
    assert "tests and generated outputs" in prompt
    assert "list each extension in the report" in prompt
    assert (
        "Return only for a reserved path, an unsettled semantic decision or the budget"
        in prompt
    )
    assert "Class sweep" in prompt
    assert (
        "mechanism of each defect fixed, siblings found by mechanism, and paths"
        in prompt
    )
    for contradiction in (
        "Return new scope",
        "Permitted proof commands",
        "parent owns integration and scope expansion",
        "conflicts or new scope",
    ):
        assert contradiction not in prompt


@pytest.mark.parametrize(
    "extras", [(), ("python -B tools/check_subagent_contract.py",)]
)
def test_standard_proof_families_do_not_depend_on_extras(extras) -> None:
    bounds = replace(_BOUNDS, owned_paths=("tools",), proof_commands=extras)
    prompt = dp.compose("Fix dispatch scope", bounds=bounds, sections=[])
    for family in (
        "land --repo <checkout> --precheck-only",
        "pytest of changed readers at -n 4 or lower",
        "owning differentials on native and WASM",
        "cargo build/test/clippy for touched crates",
        "generators write/check",
        "/usr/bin/time -l",
    ):
        assert family in prompt
    assert f"Additional proof commands: {json.dumps(extras)}" in prompt
    assert "not an allowlist" in prompt
    assert "Only the named proof owner may launch proof commands" in prompt


@pytest.mark.parametrize("invalid", [None, "runtime/", [""], [" "], [3]])
def test_dispatch_refuses_invalid_reserved_paths(invalid) -> None:
    payload = json.loads(json.dumps(asdict(_BOUNDS)))
    payload["reserved_paths"] = invalid
    with pytest.raises(ValueError, match="reserved_paths"):
        dp.DispatchBounds.from_payload(payload)


def test_cli_refuses_missing_reserved_paths_with_named_error(tmp_path, capsys) -> None:
    payload = json.loads(json.dumps(asdict(_BOUNDS)))
    payload.pop("reserved_paths", None)
    path = tmp_path / "bounds.json"
    path.write_text(json.dumps(payload), encoding="utf-8")
    brief = tmp_path / "brief.md"
    brief.write_text("preserved brief", encoding="utf-8")
    with pytest.raises(SystemExit) as error:
        dp.main(
            [
                "--task-text",
                "Fix dispatch scope",
                "--bounds",
                str(path),
                "--out",
                str(brief),
            ]
        )
    assert error.value.code == 2
    assert "reserved_paths must be an explicit JSON array" in capsys.readouterr().err
    assert brief.read_text(encoding="utf-8") == "preserved brief"


def test_cli_out_writes_a_live_brief_and_prints_only_its_pointer(
    tmp_path, capsys
) -> None:
    bounds = tmp_path / "bounds.json"
    bounds.write_text(json.dumps(asdict(_BOUNDS)), encoding="utf-8")
    brief = tmp_path / "task.md"
    assert (
        dp.main(
            [
                "--task-text",
                "Inspect retained profile",
                "--bounds",
                str(bounds),
                "--out",
                str(brief),
            ]
        )
        == 0
    )
    text = brief.read_text(encoding="utf-8")
    assert text.startswith(sc.PROJECT_IDENTITY + "\n\n" + dp.BRIEF_HEADER + "\n\n")
    assert "Assigned deliverable: Identify the dominant repeated harness cost" in text
    out = capsys.readouterr().out
    assert out.strip() == dp.launch_prompt(brief)
    assert "Assigned deliverable" not in out


# --- (module invariants) -------------------------------------------------------------------


@pytest.mark.parametrize("name", sc.CONTRACT_CONSTANT_NAMES)
def test_constant_non_empty_and_keeps_key_phrase(name: str) -> None:
    value = getattr(sc, name)
    assert isinstance(value, str) and value.strip(), (
        f"{name} must be a non-empty string"
    )
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
    prompt = dp.compose("TASK BODY", bounds=_BOUNDS, sections=None)
    assert prompt.startswith(sc.PROJECT_IDENTITY + "\n\nTASK BODY")
    for name in sc.CONTRACT_CONSTANT_NAMES:
        assert getattr(sc, name) in prompt, f"compose(all) missing {name}"


def test_compose_subset_contains_only_requested_blocks() -> None:
    prompt = dp.compose(
        "TASK BODY", bounds=_BOUNDS, sections=["landing", "web", "git", "verify"]
    )
    # 'verify' expands to two constants; all four keys -> five blocks.
    for name in (
        "LANDING_CONTRACT",
        "WEB_AUTHORITY",
        "GIT_CUSTODY",
        "VERIFY_CHANGED_CONTRACT",
        "EVIDENCE_REVIEW",
    ):
        assert getattr(sc, name) in prompt, f"requested block {name} missing"
    for name in (
        "NO_FAKES",
        "MODEL_TIER",
        "DISTILL",
        "BUILD_ENV",
        "GROUNDED_PROGRESS",
        "NO_ENDING_ON_PROMISES",
        "TRIALITY_WIRING",
    ):
        assert getattr(sc, name) not in prompt, f"unrequested block {name} leaked in"


def test_compose_is_order_stable_regardless_of_key_order() -> None:
    a = dp.compose("T", bounds=_BOUNDS, sections=["git", "web", "landing"])
    b = dp.compose("T", bounds=_BOUNDS, sections=["landing", "web", "git"])
    assert a == b, "composed prompt must be canonical-ordered, not typed-order"


def test_compose_verify_reuses_evidence_and_bounds_proofs() -> None:
    # The exact clause a recent lane was missing must be present when 'verify' is selected.
    prompt = dp.compose("T", bounds=_BOUNDS, sections=["verify"])
    assert "Reuse matching evidence" in prompt
    assert "no automatic full-suite run" in prompt


def test_compose_unknown_section_fails_loud() -> None:
    with pytest.raises(KeyError):
        dp.compose("T", bounds=_BOUNDS, sections=["landing", "not-a-section"])


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
    delattr(ns, "VERIFY_CHANGED_CONTRACT")  # synthetic removal of the recent-miss block
    result = gate.audit(ns)
    assert not result.ok
    assert any(
        "VERIFY_CHANGED_CONTRACT" in f and "MISSING" in f for f in result.failures
    )


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
        capture_output=True,
        text=True,
        cwd=str(_REPO),
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
    prompt = dp.compose(
        task,
        bounds=_BOUNDS,
        sections=["verify", "fakes", "git", "build", "landing", "triality"],
    )
    # Mandatory identity first, then task and selected routing blocks.
    assert prompt.startswith(sc.PROJECT_IDENTITY + "\n\n" + task)
    assert "\n\n" in prompt
    # The lane inherits the exact clauses it needs, mechanically:
    assert "Reuse matching evidence" in prompt  # VERIFY_CHANGED_CONTRACT (M05)
    assert "zero stubs, zero theater" in prompt  # NO_FAKES (M05)
    assert "NEVER `git add -A`" in prompt  # GIT_CUSTODY
    assert "never change source, seal, worktree, or toolchain custody" in prompt
    assert "integration owner" in prompt  # LANDING_CONTRACT (M12)
    assert "all three legs" in prompt  # TRIALITY_WIRING (M63)
    # And does NOT carry the blocks it did not request.
    assert (
        "Respect operator concurrency limits" not in prompt
    )  # MODEL_TIER not selected


@pytest.mark.parametrize("sections", [None, [], ["all"], ["web"], ["git", "verify"]])
def test_project_identity_is_verbatim_first_and_unselectable(sections) -> None:
    expected = """\
Molt is a world-class, early-stage ahead-of-time (AOT) compiler for Python, not an interpreter.
Build it the way leading compilers are built (LLVM and MLIR, Cranelift, rustc, GraalVM Native Image,
mypyc, Codon). Prove facts at compile and build time in the closed world (spec 0216); compiled code
calls the runtime only for work it cannot do itself.

Typing is our friend. Identity is typed, never spelled: runtime names are `NameId`; op kinds,
exceptions and intrinsics are generated closed types; facts live on typed SSA values. Never compare,
intern, allocate or look up a name that the compiler or the runtime can know: no
`attr_name_bits_from_bytes`, no byte or str literal of a registered name, no string-keyed fact. When
the typed form does not exist yet, add it at its one authority (for example the runtime name table)
and use it; do not spell the name.

Design data first. Prefer flat arrays indexed by dense typed ids over hash tables, indices over
pointers, structures of arrays over arrays of structures, and hot bits packed into the index or the
value word. A cache is a sign of a slow design: do not add one; redesign so the work is done once at
build time or is cheap enough to repeat. (Content-addressed build caching of whole artifacts is not
such a cache.)

On a hot path, measure instructions retired and runtime calls per iteration before and after."""
    assert sc.PROJECT_IDENTITY == expected
    prompt = dp.compose("TASK BODY", bounds=_BOUNDS, sections=sections)
    assert prompt.startswith(expected + "\n\nTASK BODY")
    assert prompt.count(expected) == 1
    assert "PROJECT_IDENTITY" not in sc.CONTRACT_CONSTANT_NAMES
    assert all("PROJECT_IDENTITY" not in names for names in dp.SECTIONS.values())


@pytest.mark.parametrize("sections", ["", "web", "all"])
def test_cli_sections_cannot_drop_project_identity(tmp_path, capsys, sections) -> None:
    bounds = tmp_path / "bounds.json"
    bounds.write_text(json.dumps(asdict(_BOUNDS)), encoding="utf-8")
    assert (
        dp.main(
            [
                "--task-text",
                "TASK BODY",
                "--bounds",
                str(bounds),
                "--sections",
                sections,
            ]
        )
        == 0
    )
    prompt = capsys.readouterr().out
    assert prompt.startswith(sc.PROJECT_IDENTITY + "\n\nTASK BODY")
    assert prompt.count(sc.PROJECT_IDENTITY) == 1
