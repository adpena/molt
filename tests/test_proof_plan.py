from __future__ import annotations

import json
from pathlib import Path

from tools import proof_plan


PLAN = proof_plan.ProofPlan.load()


def _classes(*paths: str) -> dict[str, bool]:
    selected = {family.name for family in PLAN.select(paths).selected}
    return {family.name: family.name in selected for family in PLAN.families}


def test_manifest_is_complete_and_single_authority() -> None:
    assert PLAN.path.name == "proof_plan.toml"
    assert len(PLAN.families) == 6
    assert len(PLAN.local_rules) >= 30
    assert not (proof_plan.ROOT / "tools" / "molt_dev_gates.toml").exists()
    assert not (proof_plan.ROOT / "tools" / "ci_changed_paths.py").exists()


def test_docs_only_change_skips_compiler_proofs() -> None:
    assert not any(_classes("docs/agent/INDEX.md").values())


def test_python_source_change_runs_python_smoke_only() -> None:
    classes = _classes("src/molt/cli/runtime_wasm_cache.py")
    assert classes["python_tooling"] is True
    assert classes["rust"] is False
    assert classes["llvm"] is False
    assert classes["python_security"] is False
    assert classes["rust_security"] is False
    assert classes["formal"] is False


def test_runtime_leaf_change_runs_rust_without_llvm_or_formal() -> None:
    classes = _classes("runtime/molt-stdlib-text/src/tokenize.rs")
    assert classes["rust"] is True
    assert classes["llvm"] is False
    assert classes["formal"] is False


def test_midend_change_runs_complete_rust_llvm_formal_family() -> None:
    classes = _classes("runtime/molt-passes/src/tir/value_range.rs")
    assert classes["rust"] is True
    assert classes["llvm"] is True
    assert classes["formal"] is True


def test_luau_change_selects_formal_without_stale_backend_path() -> None:
    classes = _classes("runtime/molt-backend-luau/src/luau.rs")
    assert classes["formal"] is True
    assert classes["rust"] is True


def test_llvm_control_plane_changes_run_llvm_stack() -> None:
    for path in (
        "src/molt/llvm_toolchain.py",
        "config/llvm_toolchain_arches.toml",
        ".github/actions/setup-llvm/action.yml",
        "tools/bootstrap_llvm.py",
    ):
        assert _classes(path)["llvm"] is True, path


def test_lockfiles_select_security_and_build_classes() -> None:
    cargo = _classes("Cargo.lock")
    uv = _classes("uv.lock")
    assert cargo["rust"] and cargo["llvm"] and cargo["rust_security"]
    assert not cargo["python_tooling"]
    assert uv["python_tooling"] and uv["python_security"]
    assert not uv["rust"] and not uv["rust_security"]


def test_workflow_mechanics_select_only_owned_families() -> None:
    ci = _classes(".github/workflows/ci.yml")
    security = _classes(".github/workflows/security_hardening.yml")
    formal = _classes(".github/workflows/formal.yml")
    assert ci["python_tooling"] and ci["rust"] and ci["llvm"]
    assert security["python_security"] and security["rust_security"]
    assert formal["formal"]


def test_authority_change_fails_closed_to_every_family() -> None:
    classes = _classes("tools/proof_plan.toml")
    assert all(classes.values())


def test_push_uses_before_after_instead_of_unconditionally_selecting_all(
    monkeypatch,
) -> None:
    calls: list[tuple[str, str]] = []

    def fake_diff(base: str, head: str, *, three_dot: bool = False) -> list[str]:
        assert three_dot is False
        calls.append((base, head))
        return ["src/molt/frontend/diagnostics.py"]

    monkeypatch.setattr(proof_plan, "_diff_paths", fake_diff)
    selection = proof_plan.selection_for_event(
        PLAN,
        event_name="push",
        base_ref="",
        event_path="",
        before="1" * 40,
        after="2" * 40,
    )
    assert calls == [("1" * 40, "2" * 40)]
    assert {family.name for family in selection.selected} == {"python_tooling"}


def test_forced_or_null_push_fails_closed(tmp_path: Path) -> None:
    event = tmp_path / "event.json"
    event.write_text(json.dumps({"forced": True}), encoding="utf-8")
    selection = proof_plan.selection_for_event(
        PLAN,
        event_name="push",
        base_ref="",
        event_path=str(event),
        before=proof_plan.NULL_SHA,
        after="2" * 40,
    )
    assert selection.selected == PLAN.families
    assert selection.fail_closed_reason is not None


def test_generated_matrix_records_selection_reason() -> None:
    selection = PLAN.select(["Cargo.lock"])
    outputs = proof_plan.family_outputs(PLAN, selection)
    matrix = json.loads(outputs["matrix"])["include"]
    by_name = {entry["name"]: entry for entry in matrix}
    assert by_name["rust"]["selected_by"] == ["Cargo.lock"]
    assert by_name["rust"]["resource_class"] == "compiler-build-resource"
    assert by_name["rust_security"]["cache_domain"] == "rust-security"


def test_replay_quantifies_avoided_launches(monkeypatch) -> None:
    monkeypatch.setattr(
        proof_plan,
        "_run_git",
        lambda _args: "a\nb\n",
    )
    monkeypatch.setattr(
        proof_plan,
        "_diff_paths",
        lambda base, head: (
            ["src/molt/frontend/diagnostics.py"]
            if head == "a"
            else ["runtime/molt-runtime/src/lib.rs"]
        ),
    )
    replay = proof_plan.replay_recent_commits(PLAN, 2)
    assert replay["families"]["python_tooling"]["selected"] == 1
    assert replay["families"]["rust"]["selected"] == 1
    assert replay["families"]["rust_security"]["selected"] == 0
    assert replay["families"]["rust_security"]["avoidable_percent"] == 100.0
