from pathlib import Path
import pytest

from molt.build_state_layout import build_state_root
from molt.backend_daemon_custody import backend_daemon_build_state_root_from_env
from molt.cli.runtime_paths import _build_state_root_cached
from tools.build_control_path import build_control_output
from tools.harness_memory_guard import canonical_harness_env


@pytest.mark.parametrize("target", ["absolute", "relative", "default"])
@pytest.mark.parametrize("explicit_state", [False, True])
def test_ci_build_control_output_uses_admitted_consumer_root(
    tmp_path: Path, target: str, explicit_state: bool
):
    repo = Path(__file__).resolve().parents[1]
    env = {"MOLT_EXT_ROOT": str(tmp_path / "canonical")}
    if target != "default":
        env["CARGO_TARGET_DIR"] = (
            str(tmp_path / "payload")
            if target == "absolute"
            else "target/ci-control-probe"
        )
    if explicit_state:
        env["MOLT_BUILD_STATE_DIR"] = str(tmp_path / "canonical" / "operator-state")
    admitted = canonical_harness_env(env, repo_root=repo)
    expected = backend_daemon_build_state_root_from_env(admitted, project_root=repo)
    assert build_control_output(env, repo_root=repo) == f"root={expected}"
    assert expected.is_relative_to(tmp_path / "canonical")
    assert not expected.exists(), "path projection must not create control directories"


def test_same_target_shares_canonical_control_across_receipt_roots(tmp_path: Path):
    repo = tmp_path / "repo"
    artifact = tmp_path / "canonical"
    target = tmp_path / "output" / "cargo-target"
    first = {
        "MOLT_EXT_ROOT": str(artifact),
        "CARGO_TARGET_DIR": str(target),
        "MOLT_DIFF_ROOT": str(artifact / "receipt-one"),
    }
    second = {**first, "MOLT_DIFF_ROOT": str(artifact / "receipt-two")}
    expected = build_state_root(
        project_root=repo, cargo_target=target, environment=first
    )
    assert expected == build_state_root(
        project_root=repo, cargo_target=target, environment=second
    )
    assert expected == backend_daemon_build_state_root_from_env(
        first, project_root=repo
    )
    assert expected == _build_state_root_cached(
        str(repo), None, str(target), str(repo), None, str(artifact)
    )
    assert expected != build_state_root(
        project_root=repo,
        cargo_target=target.parent / "other",
        environment=first,
    )
    assert expected.is_relative_to(artifact / "tmp" / "build-control")


def test_explicit_build_state_override_is_preserved(tmp_path: Path):
    repo = tmp_path / "repo"
    target = tmp_path / "target"
    env = {
        "MOLT_EXT_ROOT": str(tmp_path / "canonical"),
        "CARGO_TARGET_DIR": str(target),
        "MOLT_BUILD_STATE_DIR": "operator-state",
    }
    expected = repo / "operator-state"
    assert (
        build_state_root(project_root=repo, cargo_target=target, environment=env)
        == expected
    )
    assert backend_daemon_build_state_root_from_env(env, project_root=repo) == expected
