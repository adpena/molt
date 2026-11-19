from __future__ import annotations

import json
import tempfile
import os

from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process
from tools import consumer_gate
from tools.test_dependency_graph import DirectReaders
from tools.molt_dev_policy import SelectedGate


@pytest.fixture(autouse=True)
def _private_host_work_slot(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """A gate under test takes a slot of its own, not this host's (HF-623)."""
    from molt import host_capacity

    monkeypatch.setattr(
        host_capacity, "host_work_dir", lambda _repo: tmp_path / "host-work"
    )


def _git(root: Path, *args: str) -> str:
    return run_guarded_test_process(["git", "-C", str(root), *args], check=True).stdout


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    root = tmp_path / "repo"
    (root / "tools").mkdir(parents=True)
    (root / "tests").mkdir()
    (root / "tools/generator_manifest.toml").write_text(
        '[[generator]]\ntool = "tools/gen.py"\noutputs = ["generated.txt"]\n',
        encoding="utf-8",
    )
    (root / "lib.py").write_text("VALUE = 1\n", encoding="utf-8")
    (root / "generated.txt").write_text("1\n", encoding="utf-8")
    (root / "tests/test_lib.py").write_text(
        "def test_value():\n    assert True\n", encoding="utf-8"
    )
    _git(root, "init", "-q")
    _git(root, "add", "-A")
    _git(
        root,
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.invalid",
        "commit",
        "-q",
        "--no-verify",
        "-m",
        "base",
    )
    return root


def _graph(monkeypatch, importers: tuple[str, ...]) -> list[list[str]]:
    from tools import test_dependency_graph

    seen: list[list[str]] = []

    class Graph:
        def direct_readers(
            self, changed: list[str], *, before: object = None
        ) -> DirectReaders:
            seen.append(list(changed))
            return DirectReaders(importers=importers, data_readers=())

    monkeypatch.setattr(test_dependency_graph, "load_graph", lambda root: Graph())
    return seen


def test_run_writes_a_receipt_that_check_accepts_until_the_change_moves(
    repo, monkeypatch, capsys
):
    seen = _graph(monkeypatch, ("tests/test_lib.py",))
    (repo / "lib.py").write_text("VALUE = 2\n", encoding="utf-8")

    assert consumer_gate.check(repo, ["lib.py"]) == 1
    assert "tools/consumer_gate.py run -- lib.py" in capsys.readouterr().err
    assert (
        consumer_gate.run(repo, ["lib.py"], pytest_args=["-p", "no:cacheprovider"]) == 0
    )
    assert seen == [["lib.py"]]
    assert consumer_gate.check(repo, ["lib.py"]) == 0

    # A later edit is a different tree: the old receipt does not cover it.
    (repo / "lib.py").write_text("VALUE = 3\n", encoding="utf-8")
    assert consumer_gate.check(repo, ["lib.py"]) == 1


def test_a_failing_importer_test_leaves_no_receipt(repo, monkeypatch):
    _graph(monkeypatch, ("tests/test_lib.py",))
    (repo / "tests/test_lib.py").write_text(
        "def test_value():\n    assert False\n", encoding="utf-8"
    )
    paths = ["tests/test_lib.py"]
    assert consumer_gate.run(repo, paths, pytest_args=["-p", "no:cacheprovider"]) == 1
    assert consumer_gate.check(repo, paths) == 1


def test_a_hung_importer_run_is_stopped_and_leaves_no_receipt(
    repo: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    # 2026-11-17: one hung importer run held its host slot for 37 minutes at
    # 0% CPU and every other lane's gate queued behind it. The run is bounded.
    import time

    _graph(monkeypatch, ("tests/test_lib.py",))
    (repo / "tests/test_lib.py").write_text(
        "import time\n\ndef test_value():\n    time.sleep(120)\n", encoding="utf-8"
    )
    monkeypatch.setattr(consumer_gate, "IMPORTER_RUN_TIMEOUT_S", 5)
    paths = ["tests/test_lib.py"]
    started = time.monotonic()
    assert consumer_gate.run(repo, paths, pytest_args=["-p", "no:cacheprovider"]) == 1
    assert time.monotonic() - started < 60
    assert "ran past 5 s and were stopped" in capsys.readouterr().err
    assert consumer_gate.check(repo, paths) == 1


def test_a_test_that_fails_under_load_and_passes_alone_is_rerun_and_recorded(
    repo: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # 2026-11-13: three tests with wall-clock liveness deadlines failed beside
    # the suite's load and passed alone; each cost a full gate rerun. The gate
    # now reruns the failures alone once and records them, as the landing does.
    _graph(monkeypatch, ("tests/test_lib.py",))
    marker = repo / "first-run-done"
    (repo / "tests/test_lib.py").write_text(
        "from pathlib import Path\n\n"
        "class TestUnderLoad:\n"
        "    def test_value(self):\n"
        f"        marker = Path({str(marker)!r})\n"
        "        if not marker.exists():\n"
        "            marker.write_text('x')\n"
        "            raise AssertionError('deadline missed under load')\n",
        encoding="utf-8",
    )
    paths = ["tests/test_lib.py"]
    assert consumer_gate.run(repo, paths, pytest_args=["-p", "no:cacheprovider"]) == 0
    receipt = json.loads(
        consumer_gate.receipt_path(
            repo, consumer_gate.commit_tree(repo, paths)
        ).read_text(encoding="utf-8")
    )
    assert receipt["flaky_reruns"] == ["tests/test_lib.py::TestUnderLoad::test_value"]


def test_generator_outputs_alone_need_no_receipt(repo):
    (repo / "generated.txt").write_text("2\n", encoding="utf-8")
    assert consumer_gate.check(repo, ["generated.txt"]) == 0
    (repo / "lib.py").write_text("VALUE = 2\n", encoding="utf-8")
    assert consumer_gate.check(repo, ["generated.txt", "lib.py"]) == 1


def test_commit_tree_matches_what_a_pathspec_commit_records(repo):
    (repo / "lib.py").write_text("VALUE = 2\n", encoding="utf-8")
    (repo / "generated.txt").write_text("unstaged\n", encoding="utf-8")
    tree = consumer_gate.commit_tree(repo, ["lib.py"])
    _git(
        repo,
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.invalid",
        "commit",
        "-q",
        "--no-verify",
        "-m",
        "change",
        "--",
        "lib.py",
    )
    recorded = _git(repo, "rev-parse", "HEAD^{tree}").strip()
    assert tree == recorded


def _commit(repo: Path, message: str) -> None:
    _git(
        repo,
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.invalid",
        "commit",
        "-q",
        "--no-verify",
        "-am",
        message,
    )


def test_a_committed_deletion_names_a_path_the_tree_already_lacks(repo: Path) -> None:
    """2026-11-14: gating a rebased stack that deleted files raised in `git
    add` (the path is in neither the worktree nor HEAD); it changes nothing."""
    (repo / "lib.py").unlink()
    _commit(repo, "delete lib")

    tree = consumer_gate.commit_tree(repo, ["lib.py"])

    assert tree == _git(repo, "rev-parse", "HEAD^{tree}").strip()


def test_run_against_a_base_gates_a_committed_stack(
    repo: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Measured from HEAD, committed work is no change and runs nothing; a
    stack names the main it lands on and runs the tests its commits reach."""
    seen = _graph(monkeypatch, ("tests/test_lib.py",))
    monkeypatch.setattr(consumer_gate, "static_gates", lambda root, tree, base: [])
    main = _git(repo, "rev-parse", "HEAD").strip()
    (repo / "lib.py").write_text("VALUE = 2\n", encoding="utf-8")
    _commit(repo, "change lib")

    assert (
        consumer_gate.run(repo, ["lib.py"], pytest_args=["-p", "no:cacheprovider"]) == 0
    )
    assert seen == [[]]
    assert (
        consumer_gate.run(
            repo, ["lib.py"], pytest_args=["-p", "no:cacheprovider"], base=main
        )
        == 0
    )
    assert seen[-1] == ["lib.py"]
    receipt = json.loads(
        consumer_gate.receipt_path(
            repo, consumer_gate.commit_tree(repo, ["lib.py"])
        ).read_text(encoding="utf-8")
    )
    assert receipt["base"] == main and receipt["importers"] == ["tests/test_lib.py"]


def test_a_failing_static_gate_blocks_the_receipt_before_any_test(repo, monkeypatch):
    seen = _graph(monkeypatch, ("tests/test_lib.py",))
    failing = 'python -c "import sys; sys.exit(3)"'
    monkeypatch.setattr(
        consumer_gate, "static_gates", lambda root, tree, base="HEAD": [failing]
    )
    (repo / "lib.py").write_text("VALUE = 2\n", encoding="utf-8")

    assert consumer_gate.run(repo, ["lib.py"], pytest_args=[]) == 1
    assert seen == []  # no importer test ran
    assert consumer_gate.check(repo, ["lib.py"]) == 1


def test_a_passing_run_records_its_static_gates(repo, monkeypatch):
    _graph(monkeypatch, ())
    passing = 'python -c "pass"'
    monkeypatch.setattr(
        consumer_gate, "static_gates", lambda root, tree, base="HEAD": [passing]
    )
    (repo / "lib.py").write_text("VALUE = 2\n", encoding="utf-8")

    assert consumer_gate.run(repo, ["lib.py"], pytest_args=[]) == 0
    tree = consumer_gate.commit_tree(repo, ["lib.py"])
    receipt = json.loads(
        consumer_gate.receipt_path(repo, tree).read_text(encoding="utf-8")
    )
    assert receipt["static_gates"] == [passing]


def test_a_tool_change_selects_the_repository_policy_gates():
    # The checks that cost landings 42-44 on 2026-11-07: the landing selects
    # them for any tool change, and now the gate runs them before commit.
    root = consumer_gate.ROOT
    # A tree that changes a tool whatever the working tree holds: the landing
    # checkout already contains every change, so commit_tree would equal HEAD.
    with tempfile.TemporaryDirectory() as scratch:
        env = dict(os.environ, GIT_INDEX_FILE=str(Path(scratch) / "index"))
        changed = (root / "tools/consumer_gate.py").read_bytes() + b"\n# probe\n"
        blob = (
            run_guarded_test_process(
                ["git", "-C", str(root), "hash-object", "-w", "--stdin"],
                input=changed,
                text=False,
                check=True,
            )
            .stdout.decode()
            .strip()
        )
        for argv in (
            ["read-tree", "HEAD"],
            ["update-index", "--cacheinfo", f"100644,{blob},tools/consumer_gate.py"],
        ):
            run_guarded_test_process(
                ["git", "-C", str(root), *argv], env=env, check=True
            )
        tree = run_guarded_test_process(
            ["git", "-C", str(root), "write-tree"], env=env, check=True
        ).stdout.strip()
    selected = consumer_gate.static_gates(root, tree)
    # Which policy checks a tool change selects is the landing policy's
    # contract (tests/test_committed_gate_policy.py); the gate's contract is
    # that it runs that non-empty static selection before any test.
    assert selected
    assert all(isinstance(command, str) and command for command in selected)


def test_importers_needing_landing_built_artifacts_are_left_to_the_landing() -> None:
    # The real manifest: the WASM export-signature test consumes the runtime
    # pair that the compiler build wasm.build.shared-runtime makes from the
    # committed tree.
    from tools.consumer_gate import ROOT

    deps = consumer_gate.landing_built_tests(
        ROOT,
        [
            "tests/test_wasm_runtime_call_signature_authority.py",
            "tests/tools/test_consumer_gate.py",
            # repository.docs-tests orders after repository checks, which build
            # nothing: the generator test still runs before the commit.
            "tests/test_gen_wasm_abi.py",
        ],
    )
    assert deps == {
        "tests/test_wasm_runtime_call_signature_authority.py": (
            "wasm.build.shared-runtime",
        )
    }


def test_the_default_importer_run_is_presubmits() -> None:
    """Without --pytest-arg the gate runs importers as presubmit does: in
    parallel, under their declared worker memory, and without `slow` tests,
    which postsubmit owns (AGENTS.md; tests/conftest.py --skip-slow). A
    serial run with slow tests took over an hour and failed on a test the
    landing never runs."""
    root = Path(consumer_gate.__file__).resolve().parents[1]
    argv = consumer_gate.presubmit_pytest_argv(
        root, ["tests/tools/test_consumer_gate.py"]
    )
    assert argv[0] == consumer_gate.sys.executable
    assert argv[1:3] == ["-m", "pytest"]
    assert argv[argv.index("-n") + 1] == "auto"
    assert "--skip-slow" in argv
    assert any(arg.startswith("--molt-worker-memory-gb=") for arg in argv)
    assert argv[-1] == "tests/tools/test_consumer_gate.py"


def _pytest_gate(*args: str) -> SelectedGate:
    command = ("python", "-m", "pytest", "-q", "-n", "auto", *args)
    return SelectedGate(command=command, toolchains=(), pytest=True, try_run=True)


def test_a_receipt_proves_the_gates_it_ran_and_no_other(
    repo: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _graph(monkeypatch, ("tests/test_lib.py",))
    passing = 'python -c "pass"'
    monkeypatch.setattr(
        consumer_gate, "static_gates", lambda root, tree, base="HEAD": [passing]
    )
    (repo / "lib.py").write_text("VALUE = 2\n", encoding="utf-8")
    tree = consumer_gate.commit_tree(repo, ["lib.py"])
    assert consumer_gate.proven_gates(repo, tree) is None

    assert (
        consumer_gate.run(repo, ["lib.py"], pytest_args=["-p", "no:cacheprovider"]) == 0
    )
    proven = consumer_gate.proven_gates(repo, tree)
    assert proven is not None
    # The receipt ran these: the static command, and the importer under
    # presubmit's --skip-slow (a -k selection only narrows the file).
    assert proven(SelectedGate(command=passing, toolchains=()))
    assert proven(_pytest_gate("--maxfail=1", "--skip-slow", "tests/test_lib.py"))
    assert proven(
        _pytest_gate("-k", "value", "--skip-slow", "tests/test_lib.py::test_value")
    )
    # It did not run these: another static command, a cohort that runs slow
    # tests, a file it never ran, or a directory of files.
    assert not proven(SelectedGate(command='python -c "1"', toolchains=()))
    assert not proven(_pytest_gate("tests/test_lib.py"))
    assert not proven(_pytest_gate("--skip-slow", "tests/test_lib.py", "tests/t.py"))
    assert not proven(_pytest_gate("--skip-slow", "tests"))
    # A different tree has no receipt.
    (repo / "lib.py").write_text("VALUE = 3\n", encoding="utf-8")
    assert (
        consumer_gate.proven_gates(repo, consumer_gate.commit_tree(repo, ["lib.py"]))
        is None
    )


def test_the_batch_reuses_only_a_receipt_run_on_exactly_the_tree(
    repo: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _graph(monkeypatch, ("tests/test_lib.py",))
    monkeypatch.setattr(
        consumer_gate, "static_gates", lambda root, tree, base="HEAD": []
    )
    (repo / ".gitignore").write_text("__pycache__/\n", encoding="utf-8")
    (repo / "lib.py").write_text("VALUE = 2\n", encoding="utf-8")
    paths = [".gitignore", "lib.py"]
    gate = _pytest_gate("--skip-slow", "tests/test_lib.py")

    assert consumer_gate.run(repo, paths, pytest_args=["-p", "no:cacheprovider"]) == 0
    tree = consumer_gate.commit_tree(repo, paths)
    exact = consumer_gate.proven_gates(repo, tree, exact=True)
    assert exact is not None and exact(gate)

    # An untracked file the tree lacks could have made the tests pass: the
    # lane precheck may still skip the gate, the batch may not.
    (repo / "helper.py").write_text("X = 1\n", encoding="utf-8")
    assert consumer_gate.run(repo, paths, pytest_args=["-p", "no:cacheprovider"]) == 0
    assert consumer_gate.proven_gates(repo, tree, exact=True) is None
    lane = consumer_gate.proven_gates(repo, tree)
    assert lane is not None and lane(gate)


def test_a_test_failing_on_the_base_too_is_main_red_not_the_changes(
    repo: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # 2026-11-15: tests that failed on clean main blocked the receipt of every
    # change broad enough to select them, so nothing touching dx.py could be
    # committed. Chromium's "without patch" retry: a failure the base commit
    # reproduces is main's defect, recorded, and does not block the change.
    (repo / "tests/test_lib.py").write_text(
        "def test_value():\n    assert False, 'red on main'\n", encoding="utf-8"
    )
    _commit(repo, "main is red")
    _graph(monkeypatch, ("tests/test_lib.py",))
    (repo / "lib.py").write_text("VALUE = 2\n", encoding="utf-8")
    paths = ["lib.py"]
    assert consumer_gate.run(repo, paths, pytest_args=["-p", "no:cacheprovider"]) == 0
    receipt = json.loads(
        consumer_gate.receipt_path(
            repo, consumer_gate.commit_tree(repo, paths)
        ).read_text(encoding="utf-8")
    )
    assert receipt["preexisting_failures"] == ["tests/test_lib.py::test_value"]
    assert receipt["flaky_reruns"] is None
    # The temporary base worktree is gone.
    assert len(_git(repo, "worktree", "list").splitlines()) == 1


def test_a_hung_base_rerun_is_stopped_and_proves_no_main_defect(
    repo: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    # 2026-11-18: the base rerun's backend daemon never exited and held the
    # host slot for 41 minutes; every other lane's gate queued behind it. The
    # rerun is bounded, and a hang on the base is not main's defect.
    import time

    (repo / "tests/test_lib.py").write_text(
        "import time\n\nimport lib\n\n\ndef test_value():\n"
        "    if lib.VALUE == 1:\n        time.sleep(120)\n    assert False\n",
        encoding="utf-8",
    )
    _commit(repo, "a test that hangs on the base")
    _graph(monkeypatch, ("tests/test_lib.py",))
    (repo / "lib.py").write_text("VALUE = 2\n", encoding="utf-8")
    monkeypatch.setattr(consumer_gate, "RERUN_TIMEOUT_S", 5)
    paths = ["lib.py"]
    started = time.monotonic()
    assert consumer_gate.run(repo, paths, pytest_args=["-p", "no:cacheprovider"]) == 1
    assert time.monotonic() - started < 60
    assert "base rerun ran past 5 s and was stopped" in capsys.readouterr().err
    assert consumer_gate.check(repo, paths) == 1
    assert len(_git(repo, "worktree", "list").splitlines()) == 1


def test_the_owners_bypass_runs_static_gates_and_own_tests_and_records_the_rest(
    repo: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Owner direction 2026-11-15: velocity work lands directly, bypassing the
    # expensive importer suites; the receipt says so and names what it skipped.
    (repo / "tests/test_other.py").write_text(
        "def test_other():\n    assert False, 'an importer the bypass skips'\n",
        encoding="utf-8",
    )
    _commit_all(repo, "an importer that fails")
    _graph(monkeypatch, ("tests/test_lib.py", "tests/test_other.py"))
    (repo / "lib.py").write_text("VALUE = 2\n", encoding="utf-8")
    (repo / "tests/test_lib.py").write_text(
        "def test_value():\n    assert True\n\ndef test_new():\n    assert True\n",
        encoding="utf-8",
    )
    paths = ["lib.py", "tests/test_lib.py"]
    assert (
        consumer_gate.run(
            repo,
            paths,
            pytest_args=["-p", "no:cacheprovider"],
            bypass_importers="unblock landings",
        )
        == 0
    )
    receipt = json.loads(
        consumer_gate.receipt_path(
            repo, consumer_gate.commit_tree(repo, paths)
        ).read_text(encoding="utf-8")
    )
    assert receipt["bypass"] == "unblock landings"
    assert receipt["importers"] == ["tests/test_lib.py"]
    assert receipt["bypassed_importers"] == ["tests/test_other.py"]
    assert consumer_gate.check(repo, paths) == 0


def _commit_all(repo: Path, message: str) -> None:
    _git(repo, "add", "-A")
    _commit(repo, message)


def test_an_importer_run_asks_one_worker_per_file_at_its_declared_memory(
    tmp_path: Path,
) -> None:
    # The host grants shares by this demand (molt.host_capacity): a 13-file
    # gate no longer holds the whole development class.
    from molt import host_capacity

    argv = consumer_gate.presubmit_pytest_argv(
        Path(consumer_gate.__file__).resolve().parents[1],
        ["tests/tools/test_consumer_gate.py"] * 13,
    )
    demand = consumer_gate._importer_demand(argv, 13)
    unit = next(
        float(word.split("=", 1)[1])
        for word in argv
        if word.startswith("--molt-worker-memory-gb=")
    )
    half = (host_capacity.cpu_count() + 1) // 2
    assert demand == host_capacity.HostWorkDemand(max(13, half), int(unit * 1024**3))
    # One file still asks for half the host: it may hold thousands of tests.
    assert consumer_gate._importer_demand(argv, 1).workers == half
