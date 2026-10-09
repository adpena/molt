from pathlib import Path
from types import SimpleNamespace

import pytest

from tools import nightly_prepare


def test_prepare_owns_runtime_cpython_plan_and_matrix_projection(
    tmp_path: Path, monkeypatch
) -> None:
    output_root = tmp_path / "proof-results" / "nightly" / "prepare"
    cpython_dir = tmp_path / "third_party" / "cpython"
    target_root = tmp_path / "target"
    github_output = tmp_path / "github-output.txt"
    source = SimpleNamespace(revision="b" * 40)
    seen: dict[str, object] = {}

    monkeypatch.setattr(
        nightly_prepare.cpython_regrtest,
        "load_cpython_sources",
        lambda: {"3.12": source},
    )

    def fake_ensure(path, selected, **kwargs):
        seen["cpython"] = (path, selected, kwargs)

    monkeypatch.setattr(
        nightly_prepare.cpython_regrtest,
        "ensure_cpython_checkout",
        fake_ensure,
    )

    runs: list[tuple[list[str], dict[str, object]]] = []

    def fake_run(argv, **kwargs):
        runs.append((list(argv), kwargs))
        if "pack" in argv:
            Path(argv[argv.index("--output") + 1]).write_bytes(b"bundle")
            Path(argv[argv.index("--manifest-out") + 1]).write_text(
                '{"identity":{"source_commit":"' + "a" * 40 + '"}}',
                encoding="utf-8",
            )
        else:
            Path(argv[argv.index("--output") + 1]).write_bytes(b"native-smoke")
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(nightly_prepare, "COMMANDS", SimpleNamespace(run=fake_run))
    plan = {
        "cpython_commit": "b" * 40,
        "plan_sha256": "c" * 64,
        "authority": {
            "weight_profile": {"profile_sha256": "d" * 64},
        },
        "programs": {
            program: {"shards": [{"id": index} for index in range(count)]}
            for program, count in nightly_prepare.nightly_sharding.SHARD_COUNTS.items()
        },
    }
    monkeypatch.setattr(
        nightly_prepare.nightly_sharding, "build_plan", lambda *_args, **_kwargs: plan
    )

    build_env = {"CARGO_TARGET_DIR": str(target_root), "MOLT_SESSION_ID": "nightly-1"}
    summary = nightly_prepare.prepare(
        output_root=output_root,
        cpython_dir=cpython_dir,
        build_env=build_env,
        github_output=github_output,
    )

    (build_argv, build_kwargs), (pack_argv, pack_kwargs) = runs
    # The bundle exports under the smoke build's own environment, so it reads
    # the runtime generation and backend that build admitted.
    assert build_kwargs["env"] is build_env
    assert pack_kwargs["env"] is build_env
    assert pack_argv[1:3] == ["tools/nightly_runtime_bundle.py", "pack"]
    assert build_argv[build_argv.index("--stdlib-profile") + 1] == "full"
    assert build_argv[build_argv.index("--build-profile") + 1] == "dev"
    assert summary["source_commit"] == "a" * 40
    assert (output_root / "shard-plan.json").is_file()
    assert github_output.read_text(encoding="utf-8").splitlines() == [
        'conformance_matrix={"shard":[0,1,2,3,4,5,6,7]}',
        'differential_matrix={"shard":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15]}',
        'regrtest_matrix={"shard":[0,1,2,3]}',
    ]


def test_main_bundles_from_the_dx_resolved_target(tmp_path: Path, monkeypatch) -> None:
    resolved_target = tmp_path / "custody" / "cargo-target"
    seen: dict[str, object] = {}

    def fake_env(root, **kwargs):
        seen["session_prefix"] = kwargs.get("session_prefix")
        return {"CARGO_TARGET_DIR": str(resolved_target)}

    def fake_prepare(**kwargs):
        seen["build_env"] = kwargs["build_env"]
        return {"schema": "molt.nightly-prepare.v1"}

    monkeypatch.setattr(nightly_prepare, "development_artifact_env", fake_env)
    monkeypatch.setattr(nightly_prepare, "prepare", fake_prepare)
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)

    assert nightly_prepare.main(["--output-root", str(tmp_path / "out")]) == 0
    assert seen["session_prefix"] == "nightly"
    assert seen["build_env"]["CARGO_TARGET_DIR"] == str(resolved_target)

    override = tmp_path / "explicit-target"
    assert (
        nightly_prepare.main(
            ["--output-root", str(tmp_path / "out"), "--target-root", str(override)]
        )
        == 0
    )
    assert seen["build_env"]["CARGO_TARGET_DIR"] == str(override.resolve())


def test_prepare_reports_why_packing_failed(tmp_path: Path, monkeypatch) -> None:
    source = SimpleNamespace(revision="b" * 40)
    monkeypatch.setattr(
        nightly_prepare.cpython_regrtest,
        "load_cpython_sources",
        lambda: {"3.12": source},
    )
    monkeypatch.setattr(
        nightly_prepare.cpython_regrtest,
        "ensure_cpython_checkout",
        lambda *_args, **_kwargs: None,
    )

    def fake_run(argv, **kwargs):
        if "pack" in argv:
            return SimpleNamespace(
                returncode=1,
                stdout="",
                stderr="nightly-runtime-bundle: no backend compiler admitted\n",
            )
        Path(argv[argv.index("--output") + 1]).write_bytes(b"native-smoke")
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(nightly_prepare, "COMMANDS", SimpleNamespace(run=fake_run))
    with pytest.raises(RuntimeError, match="(?s)exit 1.*no backend compiler admitted"):
        nightly_prepare.prepare(
            output_root=tmp_path / "out",
            cpython_dir=tmp_path / "cpython",
            build_env={"CARGO_TARGET_DIR": str(tmp_path / "target")},
            github_output=None,
        )
