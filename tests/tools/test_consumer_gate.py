from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from tools import consumer_gate
from tools.test_dependency_graph import DirectReaders


def _git(root: Path, *args: str) -> None:
    subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True)


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
        def direct_readers(self, changed):
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
    recorded = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD^{tree}"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    assert tree == recorded
