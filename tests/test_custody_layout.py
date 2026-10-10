"""Checkout-family layout authority: one rule for where checkout state lives."""

from __future__ import annotations

import tempfile
from pathlib import Path

import pytest

from molt import custody_layout


def test_family_members_resolve_to_the_family_root(tmp_path: Path) -> None:
    root = tmp_path / "Molt"
    checkout = root / "molt-src"
    lane = root / "worktrees" / "lane-a"
    checkout.mkdir(parents=True)
    lane.mkdir(parents=True)

    assert custody_layout.custody_root(checkout) == root.resolve()
    assert custody_layout.custody_root(lane) == root.resolve()
    # The family root owns scratch for every member.
    assert custody_layout.scratch_root(root, lane) == root / "tmp"


def test_a_plain_clone_is_its_own_family_and_keeps_scratch_out_of_its_tree(
    tmp_path: Path,
) -> None:
    clone = tmp_path / "src" / "molt"
    clone.mkdir(parents=True)

    assert custody_layout.custody_root(clone) == clone.resolve()
    scratch = custody_layout.scratch_root(clone, clone)
    assert clone.resolve() not in (scratch, *scratch.parents)
    assert scratch.parent == Path(tempfile.gettempdir()).resolve()


def test_a_child_whose_temp_root_is_the_out_of_tree_scratch_agrees_with_its_parent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    clone = tmp_path / "src" / "molt"
    clone.mkdir(parents=True)
    parent_view = custody_layout.out_of_tree_scratch_root(clone)
    # RunContext exports the scratch root as the child's TMPDIR.
    child_temp = tmp_path / "host-tmp" / parent_view.name / "pytest"
    child_temp.mkdir(parents=True)
    monkeypatch.setattr(tempfile, "tempdir", str(child_temp))

    assert custody_layout.out_of_tree_scratch_root(clone) == (
        child_temp.resolve().parent
    )


def test_memory_scratch_is_one_folder_per_artifact_root_under_the_memory_root(
    tmp_path: Path,
) -> None:
    memory = tmp_path / "ram"
    first = tmp_path / "one"
    second = tmp_path / "two"

    one = custody_layout.scratch_root(first, first / "molt-src", memory_root=memory)
    assert one.parent == memory
    assert one == custody_layout.scratch_root(
        first, first / "worktrees" / "lane", memory_root=memory
    )
    assert one != custody_layout.scratch_root(
        second, second / "molt-src", memory_root=memory
    )


def test_host_scratch_is_stable_per_checkout_and_distinct_across_checkouts(
    tmp_path: Path,
) -> None:
    first = tmp_path / "one" / "molt"
    second = tmp_path / "two" / "molt"
    first.mkdir(parents=True)
    second.mkdir(parents=True)

    assert custody_layout.out_of_tree_scratch_root(
        first
    ) == custody_layout.out_of_tree_scratch_root(first)
    assert custody_layout.out_of_tree_scratch_root(
        first
    ) != custody_layout.out_of_tree_scratch_root(second)


def test_a_symlinked_spelling_resolves_to_the_physical_family(tmp_path: Path) -> None:
    root = tmp_path / "Molt"
    checkout = root / "molt-src"
    checkout.mkdir(parents=True)
    alias = tmp_path / "alias"
    try:
        alias.symlink_to(checkout, target_is_directory=True)
    except OSError as exc:  # Windows without the symlink privilege
        pytest.skip(f"host cannot create directory symlinks: {exc}")

    assert custody_layout.custody_root(alias) == root.resolve()
