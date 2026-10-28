"""Checkout-family layout authority: one rule for where checkout state lives."""

from __future__ import annotations

import tempfile
import os
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
    # A lane created at the declared root joins the same family.
    lanes = custody_layout.family_worktrees_root(checkout)
    assert lanes == (root / "worktrees").resolve()
    assert custody_layout.custody_root(lanes / "lane-b") == root.resolve()


def test_a_plain_clone_is_its_own_family_and_keeps_scratch_out_of_its_tree(
    tmp_path: Path,
) -> None:
    clone = tmp_path / "src" / "molt"
    clone.mkdir(parents=True)

    assert custody_layout.custody_root(clone) == clone.resolve()
    # Its own directory would nest lanes inside its source tree.
    assert custody_layout.family_worktrees_root(clone) is None
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

    one = custody_layout.scratch_root(first, first / "molt-src", storage_root=memory)
    assert one.parent == memory
    assert one == custody_layout.scratch_root(
        first, first / "worktrees" / "lane", storage_root=memory
    )
    assert one != custody_layout.scratch_root(
        second, second / "molt-src", storage_root=memory
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


@pytest.fixture
def git_checkout(tmp_path: Path) -> Path:
    from tests.test_dx_run_context import _git

    root = tmp_path / "family with spaces" / "molt-src"
    root.mkdir(parents=True)
    _git(root, "init", "-q", "-b", "main")
    (root / "tracked").write_text("source\n", encoding="utf-8")
    _git(root, "add", "tracked")
    _git(root, "commit", "-q", "-m", "source")
    return root


@pytest.mark.parametrize("main_name", ["molt-src", "plain-clone"])
@pytest.mark.parametrize(
    "manager", [".claude/worktrees/agent", ".codex/worktrees/id/molt-src"]
)
def test_nested_git_worktree_custody_is_outside_every_enclosing_checkout(
    git_checkout: Path, monkeypatch: pytest.MonkeyPatch, main_name: str, manager: str
) -> None:
    from tests.test_dx_run_context import _git

    root = git_checkout
    if main_name != root.name:
        root = root.rename(root.with_name(main_name))
    lane = root / manager
    _git(root, "worktree", "add", "-q", "--detach", str(lane))
    assert Path(_git(lane, "rev-parse", "--show-toplevel")).resolve() == lane
    expected_git = Path(_git(lane, "rev-parse", "--absolute-git-dir")).resolve()
    expected_common = Path(
        _git(lane, "rev-parse", "--path-format=absolute", "--git-common-dir")
    ).resolve()
    # Placement and HEAD inspection must not launch Git, even for a linked tree.
    monkeypatch.setenv("PATH", str(root.parent / "absent-tools"))
    assert custody_layout.git_directories(lane) == (expected_git, expected_common)
    assert custody_layout.enclosing_checkouts(lane) == (lane, root)
    assert custody_layout.custody_root(lane) == root.parent


@pytest.mark.parametrize("shape", ["sibling", "external-managed", "external-plain"])
def test_safe_linked_worktree_keeps_its_existing_layout(
    git_checkout: Path, tmp_path: Path, shape: str
) -> None:
    from tests.test_dx_run_context import _git

    if shape == "sibling":
        lane = git_checkout.parent / "worktrees" / "lane"
        expected = git_checkout.parent
    elif shape == "external-managed":
        lane = tmp_path / "codex" / "worktrees" / "id" / "molt-src"
        expected = lane.parent
    else:
        lane = tmp_path / "external" / "lane"
        expected = lane
    _git(git_checkout, "worktree", "add", "-q", "--detach", str(lane))
    assert custody_layout.custody_root(lane) == expected


def test_nested_independent_clone_does_not_put_outputs_in_its_parent_source(
    git_checkout: Path,
) -> None:
    from tests.test_dx_run_context import _git

    nested = git_checkout / "independent"
    nested.mkdir()
    _git(nested, "init", "-q")
    assert custody_layout.custody_root(nested) == git_checkout.parent


@pytest.mark.parametrize(
    "damage",
    ["pointer", "missing-target", "encoding", "common", "oversized", "symlink-loop"],
)
def test_present_broken_git_metadata_never_falls_back_to_a_lexical_root(
    git_checkout: Path, damage: str
) -> None:
    from tests.test_dx_run_context import _git

    lane = git_checkout / ".claude" / "worktrees" / "lane"
    _git(git_checkout, "worktree", "add", "-q", "--detach", str(lane))
    if damage == "common":
        git_dir = Path(_git(lane, "rev-parse", "--absolute-git-dir"))
        (git_dir / "commondir").write_text("\n", encoding="utf-8")
    elif damage == "symlink-loop":
        loop = git_checkout.parent / "metadata-loop"
        try:
            loop.symlink_to(loop, target_is_directory=True)
        except OSError as exc:
            pytest.skip(f"host cannot create directory symlinks: {exc}")
        (lane / ".git").write_text(f"gitdir: {loop}\n", encoding="utf-8")
    else:
        pointer = lane / ".git"
        pointer.write_bytes(
            {
                "pointer": b"not a Git directory pointer\n",
                "missing-target": b"gitdir: absent-administration\n",
                "encoding": b"\xff\n",
                "oversized": b"gitdir: " + b"x" * (64 * 1024),
            }[damage]
        )
    with pytest.raises(ValueError, match="Git .*metadata|Git common directory"):
        custody_layout.custody_root(lane)


def test_bare_repository_is_not_a_source_checkout_boundary(tmp_path: Path) -> None:
    from tests.test_dx_run_context import _git

    bare = tmp_path / "repository.git"
    bare.mkdir()
    _git(bare, "init", "-q", "--bare")
    assert _git(bare, "rev-parse", "--is-bare-repository") == "true"
    assert custody_layout.git_directories(bare) is None
    assert custody_layout.enclosing_checkouts(bare) == ()
    assert custody_layout.custody_root(bare) == bare


@pytest.mark.parametrize("name", ["molt-src", "plain-clone"])
def test_landing_source_is_external_and_repository_scoped(
    tmp_path: Path, name: str
) -> None:
    from tests.test_dx_run_context import _git

    primary = tmp_path / "Molt" / name
    primary.mkdir(parents=True)
    _git(primary, "init", "-q")
    common = primary / ".git"
    selected = custody_layout.landing_sandbox_path(primary, common)
    assert not selected.is_relative_to(primary)
    assert custody_layout.enclosing_checkouts(selected) == ()
    assert selected == custody_layout.landing_sandbox_path(primary, common)

    other = tmp_path / "other.git"
    other.mkdir()
    _git(other, "init", "-q", "--bare")
    assert selected != custody_layout.landing_sandbox_path(primary, other)


def test_landing_source_escapes_an_enclosing_checkout(git_checkout: Path) -> None:
    from tests.test_dx_run_context import _git

    nested = git_checkout / "nested" / "molt-src"
    nested.mkdir(parents=True)
    _git(nested, "init", "-q")
    selected = custody_layout.landing_sandbox_path(nested, nested / ".git")
    assert not selected.is_relative_to(git_checkout)
    assert custody_layout.enclosing_checkouts(selected) == ()


@pytest.mark.parametrize("alias", ["source", "parent", "exact-checkout"])
def test_landing_source_refuses_aliases_into_another_source(
    git_checkout: Path, alias: str
) -> None:
    selected = custody_layout.landing_sandbox_path(git_checkout, git_checkout / ".git")
    if alias == "source":
        selected.parent.mkdir(parents=True)
        link, target = selected, git_checkout
    elif alias == "parent":
        target = git_checkout / "output"
        target.mkdir()
        link = selected.parent
    else:
        from tests.test_dx_run_context import _git

        target = git_checkout.parent / "other-worktrees"
        target.mkdir()
        # Registration alone must not authorize a queue alias to another
        # source checkout, even when its basename and repository match.
        _git(
            git_checkout,
            "worktree",
            "add",
            "-q",
            "--detach",
            str(target / selected.name),
        )
        link = selected.parent
    try:
        link.symlink_to(target, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"host cannot create directory symlinks: {exc}")
    with pytest.raises(ValueError, match="symlink|inside another source checkout"):
        custody_layout.landing_sandbox_path(git_checkout, git_checkout / ".git")


@pytest.mark.parametrize(
    "name",
    [
        "agent two words",
        "agent-α-é",
        pytest.param(
            " agent ",
            marks=pytest.mark.skipif(
                os.name == "nt", reason="Win32 normalizes trailing path spaces"
            ),
        ),
        pytest.param(
            "agent\nline",
            marks=pytest.mark.skipif(
                os.name == "nt", reason="Win32 forbids LF in path components"
            ),
        ),
        pytest.param(
            "agent\rline",
            marks=pytest.mark.skipif(
                os.name == "nt", reason="Win32 forbids CR in path components"
            ),
        ),
    ],
)
def test_git_path_metadata_preserves_legal_path_bytes(
    git_checkout: Path, name: str
) -> None:
    from tests.test_dx_run_context import _git
    from tests.process_guard_common import run_guarded_test_process
    from tools.git_identity import repository_source_paths

    lane = git_checkout / ".claude" / "worktrees" / name
    _git(git_checkout, "worktree", "add", "-q", "--detach", str(lane))

    def run_git(root, arguments):
        result = run_guarded_test_process(
            ["git", "-C", str(root), *arguments], text=False, check=True, timeout=30
        )
        assert isinstance(result.stdout, bytes)
        return result.stdout

    # Git's own source-root/inventory framing is the oracle for these paths;
    # no parser-owned expected gitdir spelling is copied into the test.
    assert repository_source_paths(lane, run_git=run_git) == [lane / "tracked"]
    assert custody_layout.custody_root(lane) == git_checkout.parent
