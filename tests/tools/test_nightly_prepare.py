from pathlib import Path
from types import SimpleNamespace

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

    def fake_run(argv, **kwargs):
        seen["build"] = (argv, kwargs)
        output = Path(argv[argv.index("--output") + 1])
        output.write_bytes(b"native-smoke")
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(nightly_prepare, "COMMANDS", SimpleNamespace(run=fake_run))
    identity = SimpleNamespace(source_commit="a" * 40)
    monkeypatch.setattr(
        nightly_prepare.nightly_runtime_bundle,
        "collect_bundle_identity",
        lambda _root: identity,
    )

    def fake_pack(**kwargs):
        kwargs["output"].write_bytes(b"bundle")
        kwargs["manifest_output"].write_text("{}", encoding="utf-8")
        return {"schema_version": 1}

    monkeypatch.setattr(
        nightly_prepare.nightly_runtime_bundle, "pack_bundle", fake_pack
    )
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

    summary = nightly_prepare.prepare(
        output_root=output_root,
        cpython_dir=cpython_dir,
        target_root=target_root,
        github_output=github_output,
    )

    build_argv = seen["build"][0]
    assert build_argv[build_argv.index("--stdlib-profile") + 1] == "full"
    assert build_argv[build_argv.index("--build-profile") + 1] == "dev"
    assert summary["source_commit"] == "a" * 40
    assert (output_root / "shard-plan.json").is_file()
    assert github_output.read_text(encoding="utf-8").splitlines() == [
        'conformance_matrix={"shard":[0,1,2,3,4,5,6,7]}',
        'differential_matrix={"shard":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15]}',
        'regrtest_matrix={"shard":[0,1,2,3]}',
    ]
