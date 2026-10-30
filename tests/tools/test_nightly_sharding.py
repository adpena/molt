from __future__ import annotations

import copy
from functools import lru_cache
import hashlib
import json
import sys
from pathlib import Path
import shutil
import subprocess

import pytest

from tests.process_guard_common import HUNG_CHILD_SOURCE, run_guarded_test_process

from tools import nightly_shard_profile, nightly_sharding


SOURCE_COMMIT = "a" * 40
CPYTHON_COMMIT = "b" * 40


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


@lru_cache(maxsize=1)
def _discovery_fixture_sources() -> tuple[tuple[str, str], ...]:
    from molt.cli.python_source_closure import local_python_import_closure

    root = nightly_sharding.ROOT
    closure = local_python_import_closure(
        root, (root / relative for relative in nightly_sharding.DISCOVERY_SOURCE_SEEDS)
    )
    # Storage/tool discovery can reach the CLI facade. Its declared manifest
    # is part of the real closure too; omitting it changes import semantics
    # and can turn unrelated corpus files into authority inputs.
    return tuple(
        (path.relative_to(root).as_posix(), path.read_text(encoding="utf-8"))
        for path in closure.paths
    )


def _repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    _write(
        root / "config/nightly_shard_profile.json",
        json.dumps(
            {
                "schema": nightly_shard_profile.PROFILE_SCHEMA,
                "policy": {
                    "algorithm": nightly_shard_profile.PROFILE_ALGORITHM,
                    "bucket_count": nightly_shard_profile.BUCKET_COUNT,
                    "duration_unit": "microseconds",
                    "max_overrides_per_program": nightly_shard_profile.MAX_OVERRIDES,
                    "max_serialized_bytes": nightly_shard_profile.MAX_SERIALIZED_BYTES,
                },
                "programs": {
                    program: {"model": None}
                    for program in nightly_sharding.SHARD_COUNTS
                },
            },
            sort_keys=True,
        ),
    )
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
    _write(
        root / "config/verified_subset.toml",
        (nightly_sharding.ROOT / "config/verified_subset.toml").read_text(
            encoding="utf-8"
        ),
    )
    _write(
        root / "tests/differential/basic/generated_from_proofs/__init__.py",
        "# MOLT_META: source_role=fixture\n",
    )
    for relative, content in _discovery_fixture_sources():
        _write(root / relative, content)
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
    )


def test_transported_plan_envelope_does_not_require_external_cpython_tree(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    plan = _plan(root)
    shutil.rmtree(root / "third_party/cpython")

    nightly_sharding.validate_plan_envelope(plan, root)
    with pytest.raises((FileNotFoundError, ValueError)):
        nightly_sharding.validate_plan(plan, root)


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
    with pytest.raises(ValueError, match="authority inputs drift"):
        nightly_sharding.validate_plan(fresh, root)


def test_checked_in_profile_is_bound_and_feeds_deterministic_lpt(
    tmp_path: Path,
) -> None:
    root = _repo(tmp_path)
    path = "tests/differential/basic/differential_00.py"
    profile_path = root / "config/nightly_shard_profile.json"
    profile = json.loads(profile_path.read_text(encoding="utf-8"))
    source = root / path
    contract_digest = nightly_sharding._measurement_contract_digest(
        nightly_sharding._authority_inputs(root)
    )
    profile["programs"]["differential"]["model"] = {
        "baseline": {
            "intercept_us": 1,
            "slope_denominator": 1,
            "slope_numerator": 0,
        },
        "overrides": [
            {
                "duration_us": 1_000_000,
                "path": path,
                "sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
            }
        ],
        "training": {
            "aggregate_sha256": "c" * 64,
            "cell": "linux-x86_64-py312-native-dev",
            "corpus_sha256": "d" * 64,
            "cpython_commit": CPYTHON_COMMIT,
            "measurement_contract_sha256": contract_digest,
            "plan_sha256": "f" * 64,
            "samples": 16,
            "source_commit": "a" * 40,
        },
    }
    profile_path.write_text(json.dumps(profile, sort_keys=True), encoding="utf-8")
    plan = _plan(root)
    assert plan["programs"]["differential"]["shards"][0]["entries"] == [path]
    assert (
        plan["authority"]["weight_profile"]["programs"]["differential"][
            "applied_overrides"
        ]
        == 1
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
    plan_file = tmp_path / "shard-plan.json"
    regrtest = nightly_sharding.build_shard_command(
        tmp_path, "regrtest", selection, summary, artifact_root, plan_file
    )

    assert conformance[conformance.index("--files-from") + 1] == str(selection)
    assert differential[differential.index("--files-from") + 1] == str(selection)
    assert regrtest[regrtest.index("--tests-from") + 1] == str(selection)
    assert "--no-diff" in regrtest
    assert "--core-only" not in regrtest
    # The transported tree has no Git metadata; its custody is the plan.
    assert regrtest[regrtest.index("--cpython-dir") + 1] == "third_party/cpython"
    assert regrtest[regrtest.index("--cpython-custody") + 1] == "shard-plan"
    assert regrtest[regrtest.index("--shard-plan") + 1] == str(plan_file)
    # Admission keeps every regrtest shard inside its deadline, and the worker
    # log keeps what it finished.
    assert regrtest[regrtest.index("--deadline-s") + 1] == str(
        nightly_sharding.SHARD_DEADLINES_S["regrtest"]
        - nightly_sharding.SUMMARY_RESERVE_S
    )
    assert regrtest[regrtest.index("--results-jsonl") + 1] == "{results}"
    with pytest.raises(ValueError, match="requires an artifact root and its plan"):
        nightly_sharding.build_shard_command(
            tmp_path, "regrtest", selection, summary, artifact_root
        )


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


def test_aggregate_rejects_nonfinite_item_duration(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _repo(tmp_path)
    plan = _plan(root)
    evidence = tmp_path / "evidence"
    _run_program(root, plan, evidence, "conformance", monkeypatch)
    aggregate = nightly_sharding.aggregate(
        plan, root=root, program="conformance", evidence_root=evidence
    )
    path = next(iter(aggregate["item_durations_s"]))
    aggregate["item_durations_s"][path] = float("inf")
    with pytest.raises(ValueError, match="item telemetry is invalid"):
        nightly_sharding.validate_aggregate(plan, aggregate)


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


def test_nightly_uses_canonical_program_selection_and_binds_imported_fixtures(tmp_path):
    root = _repo(tmp_path)
    generated = root / "tests/differential/basic/generated_from_proofs/test_theorem.py"
    _write(generated, "print(1)\n")
    helper = root / "tests/differential/basic/pkg/helper.py"
    _write(helper, "VALUE = 1\n")
    programs, fixtures = nightly_sharding._differential_sources(root)
    assert generated in programs
    assert helper in fixtures and helper not in programs
    assert (
        root / "tests/differential/basic/generated_from_proofs/__init__.py" in fixtures
    )
    discovered = [
        entry["path"]
        for entry in nightly_sharding.discover_corpora(root)["differential"]
    ]
    assert discovered == nightly_sharding._current_corpus_paths(root, "differential")
    assert len(discovered) == 17
    inputs = {
        row["path"]: row["sha256"]
        for row in nightly_sharding._authority_inputs(root).files
    }
    relative = helper.relative_to(root).as_posix()
    assert relative in inputs
    _write(helper, "VALUE = 2\n")
    changed = {
        row["path"]: row["sha256"]
        for row in nightly_sharding._authority_inputs(root).files
    }
    assert changed[relative] != inputs[relative]


def test_discovery_custody_dependency_changes_invalidate_plan_authority(tmp_path):
    root = _repo(tmp_path)
    before = _plan(root)
    _write(root / "src/molt/file_publication.py", "# changed custody implementation\n")
    after = _plan(root)
    assert before["authority_sha256"] != after["authority_sha256"]
    assert before["plan_sha256"] != after["plan_sha256"]


def test_new_discovery_import_is_automatically_bound_by_shared_source_closure(tmp_path):
    root = _repo(tmp_path)
    path = root / "src/molt/file_publication.py"
    path.write_text(
        path.read_text(encoding="utf-8") + "\nfrom molt import discovery_probe\n",
        encoding="utf-8",
    )
    extra = root / "src/molt/discovery_probe.py"
    _write(extra, "VALUE = 1\n")
    before = _plan(root)
    inputs = {
        row["path"]: row["sha256"]
        for row in nightly_sharding._authority_inputs(root).files
    }
    assert "src/molt/discovery_probe.py" in inputs
    _write(extra, "VALUE = 2\n")
    after = _plan(root)
    assert before["authority_sha256"] != after["authority_sha256"]


def test_unknown_dynamic_discovery_import_cannot_produce_a_partial_plan(tmp_path):
    root = _repo(tmp_path)
    path = root / "src/molt/file_publication.py"
    path.write_text(
        path.read_text(encoding="utf-8")
        + "\nimport importlib\nimportlib.import_module(module_name)\n",
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="dynamic"):
        _plan(root)


def test_namespace_only_topology_invalidates_plan_and_measurement_contract(tmp_path):
    root = _repo(tmp_path)
    seed = root / nightly_sharding.DISCOVERY_SOURCE_SEEDS[0]
    seed.write_text(
        seed.read_text(encoding="utf-8")
        + "\n__package__ = unknown\nfrom .missing import member\n",
        encoding="utf-8",
    )
    before = nightly_sharding._authority_inputs(root)
    plan = _plan(root)
    (root / "namespace_only").mkdir()
    after = nightly_sharding._authority_inputs(root)
    assert before.files == after.files
    assert before.topology_digest != after.topology_digest
    assert nightly_sharding._measurement_contract_digest(
        before
    ) != nightly_sharding._measurement_contract_digest(after)
    with pytest.raises(ValueError, match="authority inputs drift"):
        nightly_sharding.validate_plan_envelope(plan, root)


def test_shard_loads_a_plan_whose_cpython_tree_has_no_git_metadata(
    tmp_path: Path,
) -> None:
    # A shard job receives third_party/cpython as an artifact; there,
    # `git rev-parse` would report the enclosing checkout's commit.
    root = _repo(tmp_path)
    git = ["git", "-C", str(root), "-c", "user.name=t", "-c", "user.email=t@t"]
    run_guarded_test_process([*git, "init", "-q"], check=True)
    run_guarded_test_process([*git, "add", "-A"], check=True)
    run_guarded_test_process([*git, "commit", "-q", "-m", "fixture"], check=True)
    head = run_guarded_test_process(
        [*git, "rev-parse", "HEAD"], check=True
    ).stdout.strip()
    plan = nightly_sharding.build_plan(
        root, source_commit=head, cpython_commit=CPYTHON_COMMIT
    )
    path = tmp_path / "plan.json"
    path.write_text(json.dumps(plan), encoding="utf-8")

    assert nightly_sharding._load_plan(path, root)["cpython_commit"] == CPYTHON_COMMIT


def test_run_shard_drives_the_real_guarded_executor(tmp_path: Path) -> None:
    # The fake executors above accept any keyword; this one runs the real guard,
    # which requires capture files whenever only an output tail is retained.
    root = _repo(tmp_path)
    plan = _plan(root)
    evidence = tmp_path / "evidence"
    script = (
        "import json, sys\n"
        "entries = open(sys.argv[1], encoding='utf-8').read().split()\n"
        "rows = [{'path': p, 'status': 'passed', 'duration_s': 0.01} for p in entries]\n"
        "summary = {'total': len(entries), 'passed': len(entries), 'failed': 0,\n"
        "           'compile_error': 0, 'timeout': 0, 'skipped': 0, 'item_results': rows}\n"
        "json.dump(summary, open(sys.argv[2], 'w', encoding='utf-8'))\n"
        "print('x' * 40000)\n"
    )

    nightly_sharding.run_shard(
        plan,
        root=root,
        program="conformance",
        shard_id=0,
        raw_out=evidence / "raw.json",
        checkpoint_out=evidence / "checkpoint.json",
        command=[sys.executable, "-c", script, "{selection}", "{summary}"],
    )

    raw = json.loads((evidence / "raw.json").read_text(encoding="utf-8"))
    entries = nightly_sharding.shard_entries(plan, "conformance", 0)
    assert raw["returncode"] == 0
    assert raw["summary_error"] is None
    assert (raw["selected"], raw["passed"]) == (len(entries), len(entries))
    assert 0 < len(raw["stdout_tail"]) <= 16_000


def _differential_repo(tmp_path: Path) -> Path:
    """A fixture with several differential files per shard."""
    root = _repo(tmp_path)
    for index in range(16, 64):
        family = "basic" if index % 2 == 0 else "stdlib"
        _write(
            root / "tests/differential" / family / f"differential_{index:02d}.py",
            "x = " + "1" * (index + 1) + "\n",
        )
    return root


def test_killed_differential_shard_keeps_the_items_it_finished(
    tmp_path: Path,
) -> None:
    # The child appends each finished test to the result log, as molt_diff
    # does, then hangs until the shard deadline kills it.
    root = _differential_repo(tmp_path)
    plan = _plan(root)
    entries = nightly_sharding.shard_entries(plan, "differential", 0)
    assert len(entries) >= 3
    script = (
        "import json, os, sys\n"
        "entries = open(sys.argv[1], encoding='utf-8').read().split()\n"
        "rows = [\n"
        " {'record_type': 'test', 'file': entries[0], 'raw_status': 'pass',\n"
        "  'resolved_status': 'pass', 'reason_tag': None, 'duration_s': 0.5},\n"
        " {'record_type': 'backend', 'file': entries[1], 'raw_status': 'fail',\n"
        "  'build_failed': True, 'timed_out': False, 'detail': 'link failed'},\n"
        " {'record_type': 'test', 'file': entries[1], 'raw_status': 'fail',\n"
        "  'resolved_status': 'fail', 'reason_tag': None, 'duration_s': 0.7},\n"
        "]\n"
        "with open(os.environ['MOLT_DIFF_RESULTS_JSONL'], 'a', encoding='utf-8') as log:\n"
        "    log.write(''.join(json.dumps(row) + '\\n' for row in rows))\n"
    ) + HUNG_CHILD_SOURCE
    evidence = tmp_path / "evidence"
    raw = nightly_sharding.run_shard(
        plan,
        root=root,
        program="differential",
        shard_id=0,
        raw_out=evidence / "shard-00.raw.json",
        checkpoint_out=evidence / "shard-00.checkpoint.json",
        command=[sys.executable, "-c", script, "{selection}", "{summary}"],
        timeout=5,
    )

    assert raw["returncode"] == 124
    rows = {row["path"]: row for row in raw["item_results"]}
    assert rows[entries[0]] == {
        "path": entries[0],
        "status": "passed",
        "duration_s": 0.5,
        "reason": "",
    }
    assert rows[entries[1]]["status"] == "failed"
    assert rows[entries[1]]["reason"] == "build"
    assert rows[entries[1]]["detail"] == "link failed"
    assert {rows[path]["status"] for path in entries[2:]} == {"not_run"}
    assert {rows[path]["reason"] for path in entries[2:]} == {"shard deadline"}
    assert (raw["selected"], raw["passed"], raw["failed"], raw["not_run"]) == (
        len(entries),
        1,
        1,
        len(entries) - 2,
    )
    report = "\n".join(nightly_sharding.shard_report(raw))
    assert f"not_run/shard deadline: {len(entries) - 2}" in report
    assert "failed/build: 1" in report


def _regrtest_repo(tmp_path: Path) -> Path:
    """A fixture with several regrtest modules per shard."""
    root = _repo(tmp_path)
    for index in range(4, 24):
        _write(
            root / "third_party/cpython/Lib/test" / f"test_module_{index:02d}.py",
            "x = " + "1" * (index + 1) + "\n",
        )
    return root


def test_killed_regrtest_shard_keeps_finished_and_refused_tests(
    tmp_path: Path,
) -> None:
    # The worker appends one row per test, including a test it refused at
    # admission; then the run hangs until the shard deadline kills it.
    root = _regrtest_repo(tmp_path)
    plan = _plan(root)
    entries = nightly_sharding.shard_entries(plan, "regrtest", 0)
    assert len(entries) >= 3
    script = (
        "import json, sys\n"
        "entries = open(sys.argv[1], encoding='utf-8').read().split()\n"
        "rows = [\n"
        " {'test': entries[0], 'state': 'PASSED', 'duration_s': 0.5,\n"
        "  'reason': '', 'detail': ''},\n"
        " {'test': entries[1], 'state': 'TIMEOUT', 'duration_s': 600.0,\n"
        "  'reason': 'timeout', 'detail': ''},\n"
        " {'test': entries[2], 'state': 'DID_NOT_RUN', 'duration_s': None,\n"
        "  'reason': 'deadline', 'detail': ''},\n"
        "]\n"
        "with open(sys.argv[2], 'a', encoding='utf-8') as log:\n"
        "    log.write(''.join(json.dumps(row) + '\\n' for row in rows))\n"
    ) + HUNG_CHILD_SOURCE
    evidence = tmp_path / "evidence"
    artifact_root = evidence / "artifact-tree"
    _write(artifact_root / "regrtest.log", "partial\n")
    raw = nightly_sharding.run_shard(
        plan,
        root=root,
        program="regrtest",
        shard_id=0,
        raw_out=evidence / "shard-00.raw.json",
        checkpoint_out=evidence / "shard-00.checkpoint.json",
        artifact_root=artifact_root,
        artifact_out=evidence / "shard-00.artifacts.zip",
        command=[sys.executable, "-c", script, "{selection}", "{results}"],
        timeout=5,
    )

    assert raw["returncode"] == 124
    rows = {row["path"]: row for row in raw["item_results"]}
    assert rows[entries[0]] == {
        "path": entries[0],
        "status": "passed",
        "duration_s": 0.5,
        "reason": "",
    }
    assert (rows[entries[1]]["status"], rows[entries[1]]["reason"]) == (
        "errors",
        "timeout",
    )
    assert rows[entries[2]] == {
        "path": entries[2],
        "status": "not_run",
        "duration_s": None,
        "reason": "deadline",
    }
    assert {rows[path]["reason"] for path in entries[3:]} <= {"shard deadline"}
    assert (raw["passed"], raw["errors"], raw["not_run"]) == (
        1,
        1,
        len(entries) - 2,
    )


def test_skipped_differential_items_count_toward_the_selection(
    tmp_path: Path,
) -> None:
    # molt_diff's summary total leaves skipped files out; the plan selects them.
    root = _differential_repo(tmp_path)
    plan = _plan(root)
    entries = nightly_sharding.shard_entries(plan, "differential", 0)
    script = (
        "import json, sys\n"
        "entries = open(sys.argv[1], encoding='utf-8').read().split()\n"
        "rows = [{'path': p, 'status': 'pass', 'duration_s': 0.1} for p in entries[1:]]\n"
        "rows.append({'path': entries[0], 'status': 'skip', 'duration_s': 0.1})\n"
        "summary = {'total': len(entries) - 1, 'passed': len(entries) - 1,\n"
        "           'failed': 0, 'skipped': 1, 'item_results': rows}\n"
        "json.dump(summary, open(sys.argv[2], 'w', encoding='utf-8'))\n"
    )
    evidence = tmp_path / "evidence"
    raw = nightly_sharding.run_shard(
        plan,
        root=root,
        program="differential",
        shard_id=0,
        raw_out=evidence / "shard-00.raw.json",
        checkpoint_out=evidence / "shard-00.checkpoint.json",
        command=[sys.executable, "-c", script, "{selection}", "{summary}"],
    )
    assert raw["summary_error"] is None
    assert (raw["selected"], raw["passed"], raw["skipped"]) == (
        len(entries),
        len(entries) - 1,
        1,
    )
    skipped = next(row for row in raw["item_results"] if row["status"] == "skipped")
    assert skipped["reason"] == "skip"


def test_differential_shard_bounds_each_test_and_names_unstarted_ones(
    tmp_path: Path,
) -> None:
    # The harness receives the plan's per-test bound and deadline, admits only
    # tests that can finish, and names the rest in a complete summary.
    root = _differential_repo(tmp_path)
    plan = _plan(root)
    entries = nightly_sharding.shard_entries(plan, "differential", 0)
    command = nightly_sharding.build_shard_command(
        root, "differential", Path("{selection}"), Path("{summary}")
    )
    deadline = command[command.index("--deadline-s") + 1]
    assert float(deadline) == (
        plan["programs"]["differential"]["shard_deadline_s"]
        - nightly_sharding.SUMMARY_RESERVE_S
    )
    script = (
        "import json, os, sys\n"
        "assert os.environ['MOLT_DIFF_TIMEOUT'] == '300', os.environ.get('MOLT_DIFF_TIMEOUT')\n"
        "entries = open(sys.argv[1], encoding='utf-8').read().split()\n"
        "rows = [{'path': entries[0], 'status': 'pass', 'duration_s': 0.1}]\n"
        "rows += [{'path': p, 'status': 'not_run', 'duration_s': None,\n"
        "          'reason': 'deadline'} for p in entries[1:]]\n"
        "summary = {'total': 1, 'passed': 1, 'failed': 0, 'skipped': 0,\n"
        "           'not_run': len(entries) - 1, 'item_results': rows}\n"
        "json.dump(summary, open(sys.argv[2], 'w', encoding='utf-8'))\n"
        "sys.exit(1)\n"
    )
    evidence = tmp_path / "evidence"
    raw = nightly_sharding.run_shard(
        plan,
        root=root,
        program="differential",
        shard_id=0,
        raw_out=evidence / "shard-00.raw.json",
        checkpoint_out=evidence / "shard-00.checkpoint.json",
        command=[sys.executable, "-c", script, "{selection}", "{summary}"],
    )
    assert raw["summary_error"] is None
    assert (raw["passed"], raw["not_run"]) == (1, len(entries) - 1)
    unstarted = [row for row in raw["item_results"] if row["status"] == "not_run"]
    assert {(row["duration_s"], row["reason"]) for row in unstarted} == {
        (None, "deadline")
    }


def test_shard_deadline_is_the_proof_plan_budget_less_its_overhead(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import tomllib

    authority = tomllib.loads(
        (nightly_sharding.ROOT / "tools" / "proof_plan.toml").read_text(
            encoding="utf-8"
        )
    )
    declared = {
        family["sharded_program"]: family
        for family in authority["scheduled_family"]
        if "sharded_program" in family
    }
    root = _repo(tmp_path)
    plan = _plan(root)
    for program, family in declared.items():
        deadline = (
            family["shard_timeout_minutes"] * 60 - family["shard_overhead_seconds"]
        )
        assert 0 < deadline < family["shard_timeout_minutes"] * 60
        assert plan["programs"][program]["shard_deadline_s"] == deadline
        assert nightly_sharding.SHARD_COUNTS[program] == family["shard_count"]

    seen: list[object] = []

    class _DeadlineExecutor(_FakeExecutor):
        def run(self, argv: list[str], **kwargs: object):
            seen.append(kwargs.get("timeout"))
            return super().run(argv)

    monkeypatch.setattr(nightly_sharding, "EXECUTOR", _DeadlineExecutor())
    evidence = tmp_path / "evidence"
    nightly_sharding.run_shard(
        plan,
        root=root,
        program="differential",
        shard_id=0,
        raw_out=evidence / "shard-00.raw.json",
        checkpoint_out=evidence / "shard-00.checkpoint.json",
        command=["fake", "--selection", "{selection}", "--summary", "{summary}"],
    )
    assert seen == [float(plan["programs"]["differential"]["shard_deadline_s"])]

    tampered = copy.deepcopy(plan)
    tampered["programs"]["differential"]["shard_deadline_s"] += 1
    # A plan sealed under a different budget, not a corrupted one.
    tampered["plan_sha256"] = nightly_sharding._expected_plan_digest(tampered)
    with pytest.raises(ValueError, match="deadline does not match the proof plan"):
        nightly_sharding.validate_plan_envelope(tampered, root)


def test_missing_shard_evidence_is_a_named_failure_not_a_contract_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _repo(tmp_path)
    plan = _plan(root)
    evidence = tmp_path / "evidence"
    _run_program(root, plan, evidence, "conformance", monkeypatch)
    for suffix in ("raw.json", "checkpoint.json"):
        (evidence / f"shard-02.{suffix}").replace(tmp_path / f"moved-{suffix}")

    aggregate = nightly_sharding.aggregate(
        plan, root=root, program="conformance", evidence_root=evidence
    )
    assert aggregate["ok"] is False
    assert nightly_sharding.aggregate_contract_problems(plan, aggregate) == []
    report = "\n".join(nightly_sharding.aggregate_report(aggregate))
    assert "ok=False" in report
    assert "integrity: aggregate item telemetry closure mismatch" in report
    with pytest.raises(RuntimeError, match="nightly conformance failed"):
        nightly_sharding.validate_aggregate(plan, aggregate)
    with pytest.raises(ValueError, match="item telemetry is incomplete"):
        nightly_sharding.validate_aggregate(plan, aggregate, telemetry_only=True)

    hidden = copy.deepcopy(aggregate)
    hidden["ok"] = True
    hidden["integrity_errors"] = []
    with pytest.raises(ValueError, match="partial item telemetry is not recorded"):
        nightly_sharding.validate_aggregate(plan, hidden)


def test_aggregate_contract_names_each_broken_field(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _repo(tmp_path)
    plan = _plan(root)
    evidence = tmp_path / "evidence"
    _run_program(root, plan, evidence, "conformance", monkeypatch)
    aggregate = nightly_sharding.aggregate(
        plan, root=root, program="conformance", evidence_root=evidence
    )
    broken = copy.deepcopy(aggregate)
    broken["schema"] = "other"
    broken["plan_sha256"] = "0" * 64
    with pytest.raises(ValueError) as error:
        nightly_sharding.validate_aggregate(plan, broken)
    assert "schema does not match the plan" in str(error.value)
    assert "plan_sha256 does not match the plan" in str(error.value)


def test_failed_shard_report_carries_the_child_output() -> None:
    lines = nightly_sharding.shard_report(
        {
            "program": "differential",
            "shard": 3,
            "returncode": 124,
            "selected": 2,
            "errors": 2,
            "duration_s": 5.0,
            "summary_error": "child summary is missing",
            "stderr_tail": "first\nTraceback: boom\n",
            "stdout_tail": "",
        }
    )
    assert lines[0].startswith("nightly-shard: program=differential shard=3")
    assert "returncode=124" in lines[0]
    assert "  summary_error: child summary is missing" in lines
    assert "  stderr_tail: Traceback: boom" in lines
    passing = nightly_sharding.shard_report(
        {"returncode": 0, "duration_s": 1.0, "stderr_tail": "noise"}
    )
    assert passing == [passing[0]]
