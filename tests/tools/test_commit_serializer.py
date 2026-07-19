"""Teeth for the serialized commit primitive (APPARATUS A11).

Proves the ask: the serializer refuses ``git add -A`` (and every sweep/flag/glob
pathspec), refuses a stale-sha commit (a file changed since the author read it),
and ALLOWS a clean named-file pathspec commit. Plus the lock is real and the
falsifiable ``--check`` self-test fails when a guard rots. A commit guard that
never refuses certifies nothing.
"""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import commit_serializer as cser  # noqa: E402


# --- validate_pathspec: NAMED FILES ONLY -------------------------------------


@pytest.mark.parametrize(
    "bad",
    [
        [],
        ["-A"],
        ["--all"],
        ["-a"],
        ["-u"],
        ["."],
        ["*"],
        [":/"],
        ["a", "-A"],
        ["src/*.rs"],
    ],
)
def test_validate_pathspec_refuses_sweeps_and_flags(bad: list[str]) -> None:
    assert cser.validate_pathspec(bad) is not None


@pytest.mark.parametrize(
    "good", [["a.rs"], ["src/x.py", "docs/y.md"], ["runtime/molt-backend/src/z.rs"]]
)
def test_validate_pathspec_allows_named_files(good: list[str]) -> None:
    assert cser.validate_pathspec(good) is None


# --- expected content sha ----------------------------------------------------


def test_check_expected_shas_detects_change_and_missing(tmp_path: Path) -> None:
    f = tmp_path / "f.txt"
    f.write_text("v1", encoding="utf-8")
    good = cser.sha256_file(f)
    assert good is not None
    assert cser.check_expected_shas(tmp_path, {"f.txt": good}) == []
    # file changed under the author -> mismatch
    f.write_text("v2 sibling hunk", encoding="utf-8")
    mism = cser.check_expected_shas(tmp_path, {"f.txt": good})
    assert len(mism) == 1 and mism[0].path == "f.txt" and mism[0].actual != good
    # missing file -> mismatch with actual None
    miss = cser.check_expected_shas(tmp_path, {"gone.txt": good})
    assert len(miss) == 1 and miss[0].actual is None


# --- the serialized commit, end-to-end, in a real temp repo ------------------


def _git(root: Path, *args: str) -> subprocess.CompletedProcess:
    return run_guarded_test_process(
        ["git", *args], cwd=str(root), capture_output=True, text=True, encoding="utf-8"
    )


@pytest.fixture()
def repo(tmp_path: Path) -> Path:
    _git(tmp_path, "init", "-q")
    _git(tmp_path, "config", "user.email", "a@b.c")
    _git(tmp_path, "config", "user.name", "t")
    _git(tmp_path, "config", "commit.gpgsign", "false")
    (tmp_path / "seed.txt").write_text("seed\n", encoding="utf-8")
    _git(tmp_path, "add", "seed.txt")
    _git(tmp_path, "commit", "-q", "-m", "seed")
    return tmp_path


def test_clean_named_pathspec_commit_is_allowed(repo: Path) -> None:
    (repo / "a.txt").write_text("hello\n", encoding="utf-8")
    res = cser.serialized_commit(repo, "add a.txt", ["a.txt"])
    assert res.rc == cser.RC_OK, res.reason
    assert res.committed_sha
    # the file is actually in HEAD now
    show = _git(repo, "show", "--name-only", "--pretty=format:", "HEAD")
    assert "a.txt" in show.stdout


def test_add_dash_A_is_refused_at_commit_level(repo: Path) -> None:
    (repo / "b.txt").write_text("x\n", encoding="utf-8")
    res = cser.serialized_commit(repo, "sweep", ["-A"])
    assert res.rc == cser.RC_PATHSPEC_REFUSED
    # and nothing was committed: HEAD subject unchanged
    assert _git(repo, "log", "-1", "--pretty=format:%s").stdout.strip() == "seed"


def test_stale_sha_commit_is_refused(repo: Path) -> None:
    f = repo / "c.txt"
    f.write_text("original\n", encoding="utf-8")
    author_read_sha = cser.sha256_file(f)
    # a sibling lane changes the file after the author read it
    f.write_text("original\nsibling hunk\n", encoding="utf-8")
    res = cser.serialized_commit(
        repo, "commit c", ["c.txt"], expected_shas={"c.txt": author_read_sha}
    )
    assert res.rc == cser.RC_SHA_MISMATCH
    assert res.mismatches and res.mismatches[0].path == "c.txt"
    # nothing committed
    assert _git(repo, "log", "-1", "--pretty=format:%s").stdout.strip() == "seed"


def test_matching_sha_commit_is_allowed(repo: Path) -> None:
    f = repo / "d.txt"
    f.write_text("stable\n", encoding="utf-8")
    sha = cser.sha256_file(f)
    res = cser.serialized_commit(
        repo, "commit d", ["d.txt"], expected_shas={"d.txt": sha}
    )
    assert res.rc == cser.RC_OK, res.reason


def test_dry_run_passes_guards_without_committing(repo: Path) -> None:
    (repo / "e.txt").write_text("z\n", encoding="utf-8")
    res = cser.serialized_commit(repo, "would commit", ["e.txt"], dry_run=True)
    assert res.rc == cser.RC_OK
    assert _git(repo, "log", "-1", "--pretty=format:%s").stdout.strip() == "seed"


# --- the lock is real --------------------------------------------------------


def test_serializer_lock_acquires_and_releases(tmp_path: Path) -> None:
    with cser.serializer_lock(tmp_path, timeout=5.0) as fd:
        assert isinstance(fd, int)
    # re-acquire after release must succeed (not deadlock)
    with cser.serializer_lock(tmp_path, timeout=5.0):
        pass


# --- the falsifiable self-test + CLI -----------------------------------------


def test_selftest_passes_clean() -> None:
    code, failures = cser._run_selftest()
    assert code == 0 and failures == []


def test_selftest_fails_if_pathspec_guard_rots(monkeypatch) -> None:
    monkeypatch.setattr(cser, "validate_pathspec", lambda files: None)  # always allow
    code, failures = cser._run_selftest()
    assert code == 1 and failures


def test_cli_check_exit_code() -> None:
    proc = run_guarded_test_process(
        [sys.executable, str(ROOT / "tools" / "commit_serializer.py"), "--check"],
        capture_output=True,
        text=True,
        cwd=str(ROOT),
    )
    assert proc.returncode == 0, proc.stdout + proc.stderr


def test_cli_parses_path_with_sha_suffix() -> None:
    sha = "a" * 64
    path, parsed = cser._parse_file_arg(f"src/x.rs:{sha}")
    assert path == "src/x.rs" and parsed == sha
    # a path with no sha suffix stays a plain path
    p2, s2 = cser._parse_file_arg("src/y.rs")
    assert p2 == "src/y.rs" and s2 is None
