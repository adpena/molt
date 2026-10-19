import pytest

from tools import lane_maturity as lm


def test_wasm_refused_below_l1():
    decision = lm.decide(maturity="L0", resource_family="wasm-browser")
    assert not decision.allow and decision.required == "L1"


def test_cross_lane_requires_exact_verified():
    assert not lm.decide(maturity="L3", resource_family="python", cross_lane=True).allow
    assert lm.decide(maturity="L4", resource_family="python", cross_lane=True).allow


def test_l7_transition_requires_paired_authorities_and_matrix():
    evidence = list(lm.REQUIRED_EVIDENCE["L7"])
    assert lm.decide_transition(target="L7", evidence=evidence).allow
    evidence.remove("windows_wasm_parity")
    assert not lm.decide_transition(target="L7", evidence=evidence).allow


def test_registry_failure_is_loud_fail_open(monkeypatch, tmp_path, capsys):
    monkeypatch.setattr(
        lm, "read_registry", lambda root: (_ for _ in ()).throw(OSError("boom"))
    )
    assert lm.admission_check(
        repo_root=tmp_path, lane_id="x", resource_family="wasm"
    ).allow
    assert "LOUD fail-open" in capsys.readouterr().err


def test_real_linked_worktrees_share_the_conventional_main_registry(
    tmp_path, monkeypatch
):
    from tests.tools.test_disk_guard import _registered_worktree_scope

    _, main, linked = _registered_worktree_scope(tmp_path)
    monkeypatch.setenv("PATH", str(tmp_path / "no-git-tools"))
    records = {"lane": {"maturity": "L1", "worktree": str(linked)}}
    lm.write_registry(linked, records)

    assert lm.registry_owner_root(linked) == main
    assert lm.registry_path(linked) == main / ".molt" / "state" / "lane_registry.json"
    assert lm.read_registry(main) == records
    assert not (linked / ".molt").exists()


def test_separate_git_directory_cannot_guess_a_shared_registry_owner(tmp_path):
    from tests.tools.test_disk_guard import _registered_worktree_scope

    common = tmp_path / "metadata-α é"
    _, main, linked = _registered_worktree_scope(tmp_path, separate_git_dir=common)
    assert lm.registry_owner_root(main) == main
    assert lm.registry_owner_root(linked) == linked
    lm.write_registry(linked, {"lane": {"maturity": "L1"}})
    assert not (tmp_path / ".molt").exists()
    assert not (main / ".molt").exists()


def test_missing_git_keeps_the_plain_tree_registry_owner(tmp_path):
    assert lm.registry_owner_root(tmp_path) == tmp_path
    lm.write_registry(tmp_path, {"plain": {"maturity": "L1"}})
    assert lm.read_registry(tmp_path) == {"plain": {"maturity": "L1"}}


def test_bad_git_metadata_refuses_registry_write_and_preserves_sources(tmp_path):
    from tests.tools.test_disk_guard import _registered_worktree_scope

    _, main, linked = _registered_worktree_scope(tmp_path)
    records = {"lane": {"maturity": "L1"}}
    lm.write_registry(main, records)
    manifest = lm.registry_path(main)
    before = manifest.read_bytes()
    pointer = linked / ".git"
    pointer.write_bytes(pointer.read_bytes().replace(b"gitdir:", b"GITDIR:", 1))

    with pytest.raises(ValueError, match="malformed Git checkout metadata"):
        lm.write_registry(linked, {"lane": {"maturity": "L7"}})
    assert manifest.read_bytes() == before
    assert not (linked / ".molt").exists()
    assert (linked / "tracked").read_text(encoding="utf-8") == "source\n"
