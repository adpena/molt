"""Consumer tests grant no real evidence admission or release readiness."""

from pathlib import Path

import pytest

from tools import release_matrix_acceptance as matrix
from tools.release import release_evidence


def test_stable_consumer_derives_matrix_and_cannot_supply_accepted_ids(
    monkeypatch, tmp_path
):
    expected = matrix.ReleaseMatrix("a" * 40, "b" * 64, (), (), ())
    observed = {}

    def required(*, source_sha, root):
        assert source_sha == expected.source_sha and root == tmp_path
        return expected

    def admission(value, shards, **kwargs):
        observed.update(kwargs)
        assert value is expected and shards == ()
        return ["actual full-matrix toolchain admission missing"]

    monkeypatch.setattr(matrix, "required_matrix", required)
    monkeypatch.setattr(matrix, "full_release_problems", admission)
    manifest = tmp_path / "release-exit.json"
    with pytest.raises(
        ValueError, match="full v1 release acceptance.*toolchain admission"
    ):
        release_evidence.verify_full_release_acceptance(
            manifest, source_sha="a" * 40, repo_root=tmp_path
        )
    assert observed == {
        "toolchain_identities": {},
        "semantic_bundle_manifest": manifest,
        "root": tmp_path,
    }


def test_matrix_admission_rejects_caller_authored_semantic_acceptance():
    with pytest.raises(TypeError, match="accepted_semantic_ids"):
        matrix.full_release_problems(
            matrix.ReleaseMatrix("a" * 40, "b" * 64, (), (), ()),
            (),
            toolchain_identities={},
            accepted_semantic_ids=[],
        )


def test_full_matrix_has_no_self_authored_toolchain_success_path(monkeypatch, tmp_path):
    from tools import release_exit_gate

    monkeypatch.setattr(matrix, "source_admission_problems", lambda **_: [])
    monkeypatch.setattr(
        release_exit_gate,
        "verify_release_bundle",
        lambda *_, **__: release_exit_gate.ReleaseGateReport(
            "a" * 40, "PASS", True, ()
        ),
    )
    root = Path(__file__).resolve().parents[2]
    required = matrix.required_matrix(source_sha="a" * 40, root=root)
    problems = matrix.full_release_problems(
        required,
        (),
        toolchain_identities={},
        semantic_bundle_manifest=tmp_path / "synthetic.json",
        root=root,
    )
    assert any("used-byte admission producer unavailable" in p for p in problems)
    assert any("missing" in p and "performance cells" in p for p in problems)
