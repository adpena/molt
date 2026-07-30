from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import subprocess

import pytest

from tools import nightly_sharding


SOURCE_COMMIT = "a" * 40
CPYTHON_COMMIT = "b" * 40


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    _write(
        root / "config/cpython_regrtest_sources.toml",
        "\n".join(
            (
                'schema = "molt.cpython-regrtest-sources.v1"',
                "",
                "[[source]]",
                'python = "3.12"',
                f'revision = "{CPYTHON_COMMIT}"',
                'tag = "v3.12.13"',
                'git_url = "https://github.com/python/cpython.git"',
            )
        )
        + "\n",
    )
    for relative in nightly_sharding.AUTHORITY_INPUTS:
        path = root / relative
        if not path.exists():
            _write(path, f"authority input: {relative}\n")
    for index in range(8):
        _write(
            root / "tests/harness/corpus/monty_compat" / f"conformance_{index:02d}.py",
            "x = " + "1" * (index + 1) + "\n",
        )
    for index in range(16):
        family = "basic" if index % 2 == 0 else "stdlib"
        _write(
            root / "tests/differential" / family / f"differential_{index:02d}.py",
            "x = " + "1" * (index + 1) + "\n",
        )
    for index in range(4):
        _write(
            root / "third_party/cpython/Lib/test" / f"test_module_{index:02d}.py",
            "x = " + "1" * (index + 1) + "\n",
        )
    return root


def _plan(root: Path, *, runtime_manifest: Path | None = None) -> dict:
    return nightly_sharding.build_plan(
        root,
        source_commit=SOURCE_COMMIT,
        cpython_commit=CPYTHON_COMMIT,
        runtime_artifact_manifest=runtime_manifest,
    )


def test_runtime_plan_is_deterministic_lpt_and_digest_bound(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    runtime_manifest = root / "proof-results/runtime-manifest.json"
    _write(runtime_manifest, '{"runtime":"sha256:123"}\n')

    first = _plan(root, runtime_manifest=runtime_manifest)
    second = _plan(root, runtime_manifest=runtime_manifest)

    assert first == second
    assert first["runtime_artifact_manifest"] == {
        "path": "proof-results/runtime-manifest.json",
        "sha256": hashlib.sha256(runtime_manifest.read_bytes()).hexdigest(),
    }
    assert {
        name: len(first["programs"][name]["shards"]) for name in first["programs"]
    } == {
        "conformance": 8,
        "differential": 16,
        "regrtest": 4,
    }
    for program in first["programs"].values():
        projected = [path for shard in program["shards"] for path in shard["entries"]]
        expected = [entry["path"] for entry in program["entries"]]
        assert sorted(projected) == expected
        assert len(projected) == len(set(projected))
    nightly_sharding.validate_plan(
        first,
        root,
        expected_source_commit=SOURCE_COMMIT,
        expected_cpython_commit=CPYTHON_COMMIT,
    )


def test_plan_rejects_source_runtime_and_pinned_revision_drift(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    runtime_manifest = root / "runtime.json"
    _write(runtime_manifest, "runtime\n")
    plan = _plan(root, runtime_manifest=runtime_manifest)

    source = root / plan["programs"]["conformance"]["entries"][0]["path"]
    source.write_text("changed\n", encoding="utf-8")
    with pytest.raises(ValueError, match="source digest mismatch"):
        nightly_sharding.validate_plan(plan, root)
    source.write_text("x = 1\n", encoding="utf-8")
    runtime_manifest.write_text("changed\n", encoding="utf-8")
    with pytest.raises(ValueError, match="runtime artifact manifest digest mismatch"):
        nightly_sharding.validate_plan(plan, root)

    fresh = _plan(root, runtime_manifest=runtime_manifest)
    authority = root / "config/cpython_regrtest_sources.toml"
    authority.write_text(
        authority.read_text(encoding="utf-8").replace(CPYTHON_COMMIT, "c" * 40),
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="authority input drift"):
        nightly_sharding.validate_plan(fresh, root)


def test_sparse_weights_are_bounded_and_feed_deterministic_lpt(tmp_path: Path) -> None:
    root = _repo(tmp_path)
    path = "tests/differential/basic/differential_00.py"
    plan = nightly_sharding.build_plan(
        root,
        source_commit=SOURCE_COMMIT,
        cpython_commit=CPYTHON_COMMIT,
        sparse_weights={"differential": {path: 1_000_000}},
    )
    assert plan["programs"]["differential"]["shards"][0]["entries"] == [path]

    with pytest.raises(ValueError, match="unknown weight paths"):
        nightly_sharding.build_plan(
            root,
            source_commit=SOURCE_COMMIT,
            cpython_commit=CPYTHON_COMMIT,
            sparse_weights={"differential": {"not-in-corpus.py": 1}},
        )


def test_command_construction_uses_exact_file_lists_and_regrtest_no_diff(
    tmp_path: Path,
) -> None:
    selection = tmp_path / "selection.txt"
    summary = tmp_path / "summary.json"
    artifact_root = tmp_path / "artifacts"
    conformance = nightly_sharding.build_shard_command(
        tmp_path, "conformance", selection, summary
    )
    differential = nightly_sharding.build_shard_command(
        tmp_path, "differential", selection, summary
    )
    regrtest = nightly_sharding.build_shard_command(
        tmp_path, "regrtest", selection, summary, artifact_root
    )

    assert conformance[conformance.index("--files-from") + 1] == str(selection)
    assert differential[differential.index("--files-from") + 1] == str(selection)
    assert regrtest[regrtest.index("--tests-from") + 1] == str(selection)
    assert "--no-diff" in regrtest
    assert "--core-only" not in regrtest


def test_item_telemetry_normalizes_harness_failure_classes() -> None:
    rows = nightly_sharding._item_results(
        {
            "item_results": [
                {"path": "a.py", "status": "compile_error", "duration_s": 0.1},
                {"path": "b.py", "status": "oom", "duration_s": 0.2},
                {"path": "c.py", "status": "timeout", "duration_s": 0.3},
                {"path": "d.py", "status": "pass", "duration_s": 0.4},
            ]
        },
        ["a.py", "b.py", "c.py", "d.py"],
    )
    assert [row["status"] for row in rows] == [
        "errors",
        "errors",
        "errors",
        "passed",
    ]


class _FakeExecutor:
    def __init__(self, returncode: int = 0) -> None:
        self.returncode = returncode
        self.selections: list[list[str]] = []

    def run(self, argv: list[str], **_: object) -> subprocess.CompletedProcess[str]:
        selection = Path(argv[argv.index("--selection") + 1])
        summary = Path(argv[argv.index("--summary") + 1])
        entries = selection.read_text(encoding="utf-8").splitlines()
        self.selections.append(entries)
        summary.write_text(
            json.dumps(
                {
                    "total": len(entries),
                    "passed": len(entries) if self.returncode == 0 else 0,
                    "failed": 0 if self.returncode == 0 else len(entries),
                    "errors": 0,
                    "skipped": 0,
                    "item_results": [
                        {
                            "path": path,
                            "status": "passed" if self.returncode == 0 else "failed",
                            "duration_s": 0.01,
                        }
                        for path in entries
                    ],
                }
            ),
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(
            argv, self.returncode, stdout="stdout", stderr="stderr"
        )


def _run_program(
    root: Path,
    plan: dict,
    evidence_root: Path,
    program: str,
    monkeypatch: pytest.MonkeyPatch,
    *,
    failed_shard: int | None = None,
) -> None:
    for shard_id in range(nightly_sharding.SHARD_COUNTS[program]):
        executor = _FakeExecutor(7 if shard_id == failed_shard else 0)
        monkeypatch.setattr(nightly_sharding, "EXECUTOR", executor)
        artifact_root = None
        artifact_out = None
        if program == "regrtest":
            artifact_root = evidence_root / f"artifact-tree-{shard_id:02d}"
            _write(artifact_root / "summary.json", json.dumps({"ok": True}))
            artifact_out = evidence_root / f"shard-{shard_id:02d}.artifacts.zip"
        nightly_sharding.run_shard(
            plan,
            root=root,
            program=program,
            shard_id=shard_id,
            raw_out=evidence_root / f"shard-{shard_id:02d}.raw.json",
            checkpoint_out=evidence_root / f"shard-{shard_id:02d}.checkpoint.json",
            artifact_root=artifact_root,
            artifact_out=artifact_out,
            command=[
                "fake",
                "--selection",
                "{selection}",
                "--summary",
                "{summary}",
            ],
        )
        assert executor.selections == [
            nightly_sharding.shard_entries(plan, program, shard_id)
        ]


def test_failed_shard_preserves_evidence_and_aggregate_owns_verdict(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _repo(tmp_path)
    plan = _plan(root)
    evidence = tmp_path / "evidence"
    _run_program(root, plan, evidence, "conformance", monkeypatch, failed_shard=3)

    raw = json.loads((evidence / "shard-03.raw.json").read_text(encoding="utf-8"))
    checkpoint = json.loads(
        (evidence / "shard-03.checkpoint.json").read_text(encoding="utf-8")
    )
    assert raw["returncode"] == checkpoint["returncode"] == 7
    assert raw["failed"] == 1
    aggregate = nightly_sharding.aggregate(
        plan, root=root, program="conformance", evidence_root=evidence
    )
    assert aggregate["ok"] is False
    assert aggregate["failed"] == 1
    with pytest.raises(RuntimeError, match="nightly conformance failed"):
        nightly_sharding.validate_aggregate(plan, aggregate)


def test_aggregate_requires_exact_digest_and_corpus_closure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _repo(tmp_path)
    plan = _plan(root)
    evidence = tmp_path / "evidence"
    _run_program(root, plan, evidence, "differential", monkeypatch)

    aggregate = nightly_sharding.aggregate(
        plan, root=root, program="differential", evidence_root=evidence
    )
    nightly_sharding.validate_aggregate(plan, aggregate)
    assert aggregate["selected"] == 16

    raw_path = evidence / "shard-00.raw.json"
    tampered = json.loads(raw_path.read_text(encoding="utf-8"))
    tampered["entries"] = copy.deepcopy(
        json.loads((evidence / "shard-01.raw.json").read_text(encoding="utf-8"))[
            "entries"
        ]
    )
    raw_path.write_text(json.dumps(tampered), encoding="utf-8")
    broken = nightly_sharding.aggregate(
        plan, root=root, program="differential", evidence_root=evidence
    )
    assert broken["ok"] is False
    assert any("entries mismatch" in error for error in broken["integrity_errors"])
    assert any("raw_sha256 mismatch" in error for error in broken["integrity_errors"])


def test_regrtest_aggregate_requires_digest_bound_artifact_custody(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _repo(tmp_path)
    plan = _plan(root)
    evidence = tmp_path / "evidence"
    _run_program(root, plan, evidence, "regrtest", monkeypatch)
    aggregate = nightly_sharding.aggregate(
        plan, root=root, program="regrtest", evidence_root=evidence
    )
    nightly_sharding.validate_aggregate(plan, aggregate)

    archive = evidence / "shard-00.artifacts.zip"
    archive.write_bytes(archive.read_bytes() + b"tamper")
    broken = nightly_sharding.aggregate(
        plan, root=root, program="regrtest", evidence_root=evidence
    )
    assert broken["ok"] is False
    assert any(
        "artifact digest mismatch" in error for error in broken["integrity_errors"]
    )


@pytest.mark.parametrize("member", ("../escape", "/absolute", "C:/escape"))
def test_artifact_archive_rejects_unsafe_members(tmp_path: Path, member: str) -> None:
    archive = tmp_path / "artifact.zip"
    import zipfile

    with zipfile.ZipFile(archive, "w") as output:
        info = zipfile.ZipInfo(member)
        info.external_attr = 0o100644 << 16
        output.writestr(info, b"x")
    rows = [{"path": member, "size": 1, "sha256": hashlib.sha256(b"x").hexdigest()}]
    with pytest.raises(ValueError, match="unsafe artifact member path"):
        nightly_sharding._validate_artifact_archive(archive, rows)
