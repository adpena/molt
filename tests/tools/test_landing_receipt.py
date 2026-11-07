"""Landing receipts and the pre-push hook that requires them (HF-701).

The hook tests install the shipped `.githooks/pre-push` into a real temporary
repository and push to a real bare remote, so the refusal is Git's own.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
from pathlib import Path

import pytest

import tools.install_git_hooks as ig
from tests.process_guard_common import run_guarded_test_process
from tools import landing_receipt as lr

REPO_ROOT = Path(__file__).resolve().parents[2]
PRE_PUSH = next(h for h in ig.HOOKS if h.name == "pre-push" and not h.retired)


def _git(repo: Path, *args: str, check: bool = True):
    return run_guarded_test_process(
        ["git", "-C", str(repo), *args],
        check=check,
        capture_output=True,
        text=True,
    )


def _out(repo: Path, *args: str) -> str:
    return _git(repo, *args).stdout.strip()


@pytest.fixture
def pushed(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    """A checkout with the real hook installed, its remote, and main's base."""
    if os.name == "nt" or shutil.which("uv") is None:
        pytest.skip("the shipped hook launcher needs POSIX sh and uv")
    remote = tmp_path / "remote.git"
    run_guarded_test_process(["git", "init", "--bare", "-q", str(remote)], check=True)
    repo = tmp_path / "repo"
    repo.mkdir()
    run_guarded_test_process(["git", "init", "-q", "-b", "main", str(repo)], check=True)
    for key, value in (("user.name", "Molt Dev"), ("user.email", "dev@molt.test")):
        _git(repo, "config", key, value)
    (repo / ".githooks").mkdir()
    for name in ("pre-push", "molt-hook-launch.sh"):
        shutil.copy(REPO_ROOT / ".githooks" / name, repo / ".githooks" / name)
    (repo / "tools").mkdir()
    shutil.copy(REPO_ROOT / "tools" / "landing_receipt.py", repo / "tools")
    # The repository is its own common checkout; its environment is this one.
    (repo / ".venv" / "bin").mkdir(parents=True)
    (repo / ".venv" / "bin" / "python").symlink_to(Path(sys.executable))
    (repo / ".gitignore").write_text(".venv/\n", encoding="utf-8")
    _git(repo, "add", ".")
    _git(repo, "commit", "-qm", "base")
    _git(repo, "remote", "add", "origin", str(remote))
    # The base reaches main before the hook exists.
    _git(repo, "push", "-q", "origin", "HEAD:refs/heads/main")
    monkeypatch.setattr(ig, "HOOKS_SOURCE_DIR", repo / ".githooks")
    assert (
        ig.install(
            check=False,
            uninstall=False,
            repo_root=repo,
            hooks=(PRE_PUSH,),
            merge_drivers=(),
        )
        == 0
    )
    base = _out(repo, "rev-parse", "HEAD")
    (repo / "work.txt").write_text("work\n", encoding="utf-8")
    _git(repo, "add", "work.txt")
    _git(repo, "commit", "-qm", "work")
    return repo, remote, base


def _receipt(repo: Path, *, base: str, commit: str) -> Path:
    tree = _out(repo, "rev-parse", f"{commit}^{{tree}}")
    receipt = lr.build_receipt(
        mode="fast",
        base=base,
        tree=tree,
        tested_commit=commit,
        controller="candidate",
        started_at="2026-11-04T00:00:00+00:00",
        gates=[],
    )
    return lr.write_receipt(repo / ".git", receipt)


def _remote_main(remote: Path) -> str:
    return run_guarded_test_process(
        ["git", "--git-dir", str(remote), "rev-parse", "refs/heads/main"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def test_main_push_without_a_receipt_is_refused(pushed) -> None:
    repo, remote, base = pushed
    result = _git(repo, "push", "origin", "HEAD:refs/heads/main", check=False)
    assert result.returncode != 0
    assert "no landing receipt for tree" in result.stderr
    assert "land --fast" in result.stderr
    assert _remote_main(remote) == base


def test_a_matching_receipt_admits_the_exact_commit(pushed) -> None:
    repo, remote, base = pushed
    head = _out(repo, "rev-parse", "HEAD")
    _receipt(repo, base=base, commit=head)
    _git(repo, "push", "-q", "origin", "HEAD:refs/heads/main")
    assert _remote_main(remote) == head


def test_a_receipt_for_another_base_is_refused(pushed) -> None:
    repo, remote, base = pushed
    head = _out(repo, "rev-parse", "HEAD")
    _receipt(repo, base=head, commit=head)  # tested on a main that is not the remote's
    result = _git(repo, "push", "origin", "HEAD:refs/heads/main", check=False)
    assert result.returncode != 0
    assert f"on base {base}" in result.stderr
    assert _remote_main(remote) == base


def test_a_commit_the_receipt_does_not_list_is_refused(pushed) -> None:
    repo, remote, base = pushed
    tested = _out(repo, "rev-parse", "HEAD")
    _receipt(repo, base=base, commit=tested)
    # Same tree, another commit: the receipt must name the pushed commit.
    _git(repo, "commit", "--amend", "-qm", "work, reworded")
    result = _git(repo, "push", "origin", "HEAD:refs/heads/main", check=False)
    assert result.returncode != 0
    assert "does not list this commit" in result.stderr
    lr.add_commit(
        repo / ".git",
        tree=_out(repo, "rev-parse", "HEAD^{tree}"),
        base=base,
        commit=_out(repo, "rev-parse", "HEAD"),
    )
    _git(repo, "push", "-q", "origin", "HEAD:refs/heads/main")
    assert _remote_main(remote) == _out(repo, "rev-parse", "HEAD")


def test_other_refs_and_tags_pass_and_a_mixed_push_is_refused(pushed) -> None:
    repo, remote, base = pushed
    _git(repo, "tag", "v-test")
    _git(repo, "push", "-q", "origin", "HEAD:refs/heads/lane/x", "refs/tags/v-test")
    result = _git(
        repo,
        "push",
        "origin",
        "HEAD:refs/heads/lane/y",
        "HEAD:refs/heads/main",
        check=False,
    )
    assert result.returncode != 0
    assert "no landing receipt" in result.stderr
    assert _remote_main(remote) == base
    listed = _out(repo, "ls-remote", "origin", "refs/heads/lane/y")
    assert listed == ""  # a refused pre-push stops the whole push


def test_deleting_main_is_refused(pushed) -> None:
    repo, remote, base = pushed
    result = _git(repo, "push", "origin", ":refs/heads/main", check=False)
    assert result.returncode != 0
    assert "refused deleting main" in result.stderr
    assert _remote_main(remote) == base


def test_push_refusals_reads_each_line(tmp_path: Path) -> None:
    lines = [
        "refs/heads/a 1111 refs/heads/a 2222\n",
        "malformed\n",
        f"refs/heads/main {'0' * 40} refs/heads/main {'1' * 40}\n",
        f"refs/heads/main {'1' * 40} refs/heads/main {'0' * 40}\n",
    ]
    refusals = lr.push_refusals(lines, cwd=tmp_path)
    assert refusals == [
        "malformed pre-push line: 'malformed'",
        "refused deleting main",
        f"refused creating main at {'1' * 40}: main must already exist",
    ]


def test_counts_come_from_libtest_and_pytest_summaries() -> None:
    cargo = (
        "     Running unittests src/lib.rs (target/x/deps/a-1)\n"
        "test result: ok. 12 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; "
        "finished in 0.10s\n"
        "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; "
        "finished in 0.01s\n"
    )
    assert lr.counts_from_output(cargo) == {"passed": 15, "failed": 0, "skipped": 2}
    pytest_q = "....s.\n5 passed, 1 skipped, 3 warnings in 4.20s\n"
    assert lr.counts_from_output(pytest_q) == {"passed": 5, "failed": 0, "skipped": 1}
    framed = "===== 7 passed, 1 xfailed in 1.00s =====\n"
    assert lr.counts_from_output(framed) == {"passed": 7, "failed": 0, "skipped": 1}
    assert lr.counts_from_output("All checks passed!\n") is None


def test_receipts_are_keyed_by_tree_and_base_and_written_atomically(
    tmp_path: Path,
) -> None:
    tree, base, commit = "a" * 40, "b" * 40, "c" * 40
    receipt = lr.build_receipt(
        mode="queue",
        base=base,
        tree=tree,
        tested_commit=commit,
        controller=None,
        started_at="t",
        gates=[
            {
                "phase": "static",
                "command": "x",
                "seconds": 1.0,
                "counts": None,
                "log": None,
            }
        ],
    )
    path = lr.write_receipt(tmp_path, receipt)
    assert path == tmp_path / "molt" / "landing-receipts" / f"{tree}.{base}.json"
    assert json.loads(path.read_text(encoding="utf-8"))["commits"] == [commit]
    assert lr.read_receipt(tmp_path, tree=tree, base="d" * 40) is None
    assert [p.name for p in path.parent.iterdir()] == [path.name]
    with pytest.raises(ValueError):
        lr.receipt_path(tmp_path, tree="short", base=base)
    with pytest.raises(ValueError):
        lr.write_receipt(tmp_path, {**receipt, "status": "failed"})


def test_old_receipts_retire(tmp_path: Path) -> None:
    receipt = lr.build_receipt(
        mode="fast",
        base="b" * 40,
        tree="a" * 40,
        tested_commit="c" * 40,
        controller=None,
        started_at="t",
        gates=[],
    )
    old = lr.write_receipt(tmp_path, receipt)
    stale = old.stat().st_mtime - (lr.RETENTION_DAYS + 1) * 86400
    os.utime(old, (stale, stale))
    lr.write_receipt(tmp_path, {**receipt, "tree": "e" * 40})
    assert not old.exists()


def test_commit_message_is_projected_from_the_receipt_alone() -> None:
    receipt = lr.build_receipt(
        mode="fast",
        base="b" * 40,
        tree="a" * 40,
        tested_commit="c" * 40,
        controller="candidate",
        started_at="t",
        gates=[
            {
                "phase": "static",
                "command": "python -m ruff check .",
                "seconds": 1.0,
                "counts": None,
                "log": None,
            },
            {
                "phase": "try-run",
                "command": "python tools/check_changed_crate_tests.py --base "
                + "b" * 40,
                "seconds": 90.0,
                "counts": {"passed": 618, "failed": 0, "skipped": 4},
                "log": None,
            },
        ],
        waived=["cargo test --locked --profile release-fast -p molt-backend --lib"],
        narrowed=[
            "features of unselected packages not applied: molt-wasm-facts/host-tool"
        ],
    )
    message = lr.commit_message("Land the thing (HF-1)", receipt)
    assert message == (
        "Land the thing (HF-1)\n"
        "\n"
        "Landed by molt_dev.py land --fast: tree aaaaaaaaaaaa passed on base\n"
        "  bbbbbbbbbbbb (tested commit cccccccccccc).\n"
        "Static gates: 1 passed.\n"
        "Ran python tools/check_changed_crate_tests.py --base\n"
        "  bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb: 618 passed, 4 skipped.\n"
        "Waived to the queue batch: cargo test --locked --profile release-fast -p\n"
        "  molt-backend --lib.\n"
        "Narrowed: features of unselected packages not applied:\n"
        "  molt-wasm-facts/host-tool.\n"
        "Receipt: landing-receipts/" + "a" * 40 + "." + "b" * 40 + ".json\n"
    )
    with pytest.raises(ValueError):
        lr.commit_message("two\nlines", receipt)


def test_postsubmit_only_is_recorded_digested_and_counted_not_listed() -> None:
    # Owner 2026-11-06: presubmit runs importers; data readers are left to
    # postsubmit. The receipt names them, they change the selection digest,
    # and the commit message counts them without passing them.
    def receipt(left):
        return lr.build_receipt(
            mode="fast",
            base="b" * 40,
            tree="a" * 40,
            tested_commit="c" * 40,
            controller="candidate",
            started_at="t",
            gates=[
                {
                    "phase": "static",
                    "command": "python -m ruff check .",
                    "seconds": 1.0,
                    "counts": None,
                    "log": None,
                }
            ],
            postsubmit_only=left,
        )

    none, some = receipt(()), receipt(("tests/test_a.py", "tests/test_b.py::test_c"))
    assert none["postsubmit_only"] == []
    assert some["postsubmit_only"] == ["tests/test_a.py", "tests/test_b.py::test_c"]
    assert none["selection_sha256"] != some["selection_sha256"]
    message = lr.commit_message("Land a change", some)
    assert "Left to postsubmit: 2 data-reader test file(s)" in message
    assert "tests/test_a.py" not in message
    assert "Left to postsubmit" not in lr.commit_message("Land a change", none)
