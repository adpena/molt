from __future__ import annotations

from pathlib import Path

from tools import check_test_ownership as ownership


def _repo(root: Path, *, argv: list[str], tests: list[str]) -> Path:
    (root / "tools").mkdir(parents=True)
    quoted = ", ".join(f'"{item}"' for item in argv)
    (root / "tools" / "proof_plan.toml").write_text(
        f'[[command]]\nid = "unit"\nargv = ["uv", "run", "pytest", {quoted}]\n',
        encoding="utf-8",
    )
    for test in tests:
        path = root / test
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("def test_ok():\n    pass\n", encoding="utf-8")
    return root


def test_files_named_by_path_or_directory_are_owned_but_a_node_is_not(
    tmp_path: Path,
) -> None:
    root = _repo(
        tmp_path,
        argv=["tests/test_a.py", "tests/test_b.py::test_ok", "tests/model"],
        tests=[
            "tests/test_a.py",
            "tests/test_b.py",
            "tests/model/test_c.py",
            "tests/test_d.py",
            "tests/differential/test_e.py",
            "tests/cli/fixtures/test_f.py",
        ],
    )

    assert ownership.unowned(root) == {"tests/test_b.py", "tests/test_d.py"}


def test_only_a_file_that_pytest_selects_has_an_owner(tmp_path: Path) -> None:
    # pytest's own parser reads each command (HF-179). pytest runs neither a
    # file it is told to --ignore nor one named only as a --deselect value,
    # and a tool that is not pytest runs no pytest file, though all three
    # spell a tests/ path.
    (tmp_path / "tools").mkdir()
    (tmp_path / "tools" / "proof_plan.toml").write_text(
        '[[command]]\nid = "unit"\nargv = ["uv", "run", "pytest", "--ignore", '
        '"tests/test_ignored.py", "--deselect", "tests/test_deselected.py", '
        '"tests/test_a.py"]\n'
        '[[command]]\nid = "tool"\nargv = ["python", "tools/runner.py", '
        '"tests/test_operand.py"]\n',
        encoding="utf-8",
    )
    (tmp_path / "tests").mkdir()
    for name in ("a", "ignored", "deselected", "operand"):
        (tmp_path / "tests" / f"test_{name}.py").write_text(
            "def test_ok():\n    pass\n", encoding="utf-8"
        )

    assert ownership.unowned(tmp_path) == {
        "tests/test_ignored.py",
        "tests/test_deselected.py",
        "tests/test_operand.py",
    }


def test_a_new_unowned_file_fails_and_cannot_be_baselined(tmp_path: Path) -> None:
    root = _repo(tmp_path, argv=["tests/test_a.py"], tests=["tests/test_a.py"])
    assert ownership.main(["--root", str(root), "--update"]) == 0
    (root / "tests" / "test_new.py").write_text("", encoding="utf-8")

    assert ownership.check(root) == [
        "tests/test_new.py: no proof-plan command runs this test file; add it to "
        "the command that owns its subject in tools/proof_plan.toml"
    ]
    assert ownership.main(["--root", str(root), "--update"]) == 1


def test_a_file_that_gains_an_owner_must_leave_the_baseline(tmp_path: Path) -> None:
    root = _repo(
        tmp_path, argv=["tests/test_a.py"], tests=["tests/test_a.py", "tests/test_b.py"]
    )
    assert ownership.main(["--root", str(root), "--update"]) == 0
    assert ownership.read_baseline(root) == {"tests/test_b.py"}
    plan = root / "tools" / "proof_plan.toml"
    plan.write_text(
        plan.read_text(encoding="utf-8").replace(
            '"tests/test_a.py"', '"tests/test_a.py", "tests/test_b.py"'
        ),
        encoding="utf-8",
    )

    assert ownership.check(root) == [
        "tests/test_b.py: now owned or deleted; remove it from "
        "tools/test_ownership_baseline.json with --update"
    ]
    assert ownership.main(["--root", str(root), "--update"]) == 0
    assert ownership.check(root) == []
