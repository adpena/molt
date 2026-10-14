"""Teeth for tools/ff_land.py: prove it fails CLOSED on every unsafe land."""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

TOOL = Path(__file__).resolve().parents[2] / "tools" / "ff_land.py"


def _git(repo: Path, *args: str) -> str:
    return run_guarded_test_process(
        ["git", *args], cwd=repo, check=True, capture_output=True, text=True
    ).stdout.strip()


def _run(repo: Path, *args: str, tool: Path = TOOL) -> subprocess.CompletedProcess[str]:
    environment = dict(os.environ)
    if tool != TOOL:
        environment["PYTHONPATH"] = os.pathsep.join((str(repo), str(repo / "src")))
    return run_guarded_test_process(
        [sys.executable, "-B", str(tool), *args],
        cwd=repo,
        env=environment,
        capture_output=True,
        text=True,
    )


def _init_commit(repo: Path, name: str = "a.txt", content: str = "v1\n") -> None:
    (repo / name).write_text(content, encoding="utf-8")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-q", "-m", f"add {name}")


@pytest.fixture()
def work(tmp_path: Path) -> Path:
    bare = tmp_path / "remote.git"
    bare.mkdir()
    _git(bare, "init", "--bare", "-q", "-b", "main")
    w = tmp_path / "work"
    w.mkdir()
    _git(w, "init", "-q", "-b", "main")
    _git(w, "config", "user.email", "t@t")
    _git(w, "config", "user.name", "t")
    _git(w, "remote", "add", "origin", str(bare))
    _init_commit(w)
    _git(w, "push", "-q", "origin", "main")
    return w


def _second_clone_pushes(work: Path, tmp_path: Path) -> None:
    """Advance origin/main out-of-band so `work` becomes non-fast-forwardable."""
    origin_url = _git(work, "remote", "get-url", "origin")
    other = tmp_path / "other"
    _git(tmp_path, "clone", "-q", origin_url, str(other))
    _git(other, "config", "user.email", "o@o")
    _git(other, "config", "user.name", "o")
    _init_commit(other, "remote_only.txt", "remote\n")
    _git(other, "push", "-q", "origin", "main")


def test_clean_ff_lands(work: Path) -> None:
    _init_commit(work, "b.txt")
    res = _run(work)
    assert res.returncode == 0, res.stdout
    assert "FF-LAND OK: pushed" in res.stdout
    # remote actually advanced to our HEAD
    assert _git(work, "rev-parse", "HEAD") == _git(work, "rev-parse", "origin/main")


def test_nothing_to_land(work: Path) -> None:
    res = _run(work)
    assert res.returncode == 0, res.stdout
    assert "nothing to land" in res.stdout


def test_dirty_tree_refused(work: Path) -> None:
    (work / "a.txt").write_text("locally edited\n", encoding="utf-8")
    res = _run(work)
    assert res.returncode == 2, res.stdout
    assert "REFUSED" in res.stdout and "uncommitted" in res.stdout


def test_non_fast_forward_refused(work: Path, tmp_path: Path) -> None:
    _second_clone_pushes(work, tmp_path)
    _init_commit(work, "c.txt")  # our own divergent commit
    res = _run(work)
    assert res.returncode == 1, res.stdout
    assert "REFUSED (DRIFT)" in res.stdout
    # our commit must NOT have landed
    assert _git(work, "rev-parse", "HEAD") != _git(work, "rev-parse", "origin/main")


def test_stale_proof_plan_projection_is_refused_before_push(work: Path) -> None:
    generator = work / "tools" / "gen_proof_plan.py"
    generator.parent.mkdir()
    generator.write_text(
        "import sys\nprint('proof-plan projection stale: generated.json')\nsys.exit(1)\n",
        encoding="utf-8",
    )
    generator.with_name("proof_plan.toml").write_text("fixture = true\n")
    _init_commit(work, "b.txt")
    before = _git(work, "rev-parse", "origin/main")

    res = _run(work)

    assert res.returncode == 4, res.stdout
    assert "REFUSED (GENERATED DRIFT)" in res.stdout
    assert "proof-plan projection stale" in res.stdout
    _git(work, "fetch", "origin", "--quiet")
    assert _git(work, "rev-parse", "origin/main") == before


def test_dry_run_does_not_push(work: Path) -> None:
    _init_commit(work, "d.txt")
    before = _git(work, "rev-parse", "origin/main")
    res = _run(work, "--dry-run")
    assert res.returncode == 0, res.stdout
    assert "dry-run" in res.stdout
    _git(work, "fetch", "origin", "--quiet")
    assert _git(work, "rev-parse", "origin/main") == before


def _projection_fixture(work: Path, *, stale: bool = False, after: str = "") -> Path:
    """A real committed generator with an independent input/output oracle."""
    tools = work / "tools"
    tools.mkdir(exist_ok=True)
    generator = tools / "gen_proof_plan.py"
    generator.write_text(
        "from pathlib import Path\n"
        "import sys\n"
        "root = Path(__file__).resolve().parents[1]\n"
        "assert '--check' in sys.argv\n"
        "if (root/'authority.txt').read_bytes() != (root/'projection.txt').read_bytes():\n"
        "    print('committed projection is stale')\n"
        "    sys.exit(1)\n" + after,
        encoding="utf-8",
    )
    generator.with_name("proof_plan.toml").write_text("fixture = true\n")
    (work / "authority.txt").write_text("committed\n")
    (work / "projection.txt").write_text("stale\n" if stale else "committed\n")
    _init_commit(work, "b.txt")
    return generator


@pytest.mark.parametrize(
    "change", ["working", "staged", "deleted-generator", "poisoned-generator"]
)
def test_allow_dirty_validates_commit_and_preserves_unlanded_inputs(
    work: Path, change: str
) -> None:
    generator = _projection_fixture(work)
    committed = _git(work, "rev-parse", "HEAD")
    if change in {"working", "staged"}:
        (work / "authority.txt").write_text("unlanded input\n")
        if change == "staged":
            _git(work, "add", "authority.txt")
    elif change == "deleted-generator":
        generator.unlink()
    else:
        generator.write_text("raise AssertionError('unlanded generator executed')\n")
    before = _git(work, "status", "--porcelain")
    result = _run(work, "--allow-dirty")
    assert result.returncode == 0, result.stdout + result.stderr
    assert _git(work, "rev-parse", "origin/main") == committed
    assert _git(work, "status", "--porcelain") == before


def test_unlanded_repair_cannot_hide_stale_committed_projection(work: Path) -> None:
    generator = _projection_fixture(work, stale=True)
    before = _git(work, "rev-parse", "origin/main")
    (work / "projection.txt").write_text("committed\n")
    generator.write_text("# unlanded permissive generator\n")
    _git(work, "add", "projection.txt", "tools/gen_proof_plan.py")
    dirty = _git(work, "status", "--porcelain")
    result = _run(work, "--allow-dirty")
    assert result.returncode == 4, result.stdout + result.stderr
    assert "committed projection is stale" in result.stdout
    assert _git(work, "rev-parse", "origin/main") == before
    assert _git(work, "status", "--porcelain") == dirty


@pytest.mark.parametrize("stale", [False, True])
def test_same_checkout_landing_never_executes_unlanded_generator(
    work: Path, stale: bool
) -> None:
    generator = _projection_fixture(work, stale=stale)
    local_tool = generator.with_name("ff_land.py")
    # Exercise the real source-bound tool/import/guard closure in this checkout.
    # Keep the independent generator oracle already installed above.
    source = TOOL.parents[1]
    for relative in _git(
        source, "ls-files", "--", "tools", "src/molt", "pyproject.toml"
    ).splitlines():
        if relative == "tools/gen_proof_plan.py":
            continue
        destination = work / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source / relative, destination)
    _git(work, "add", "tools/ff_land.py")
    _git(work, "commit", "-q", "-m", "install landing tool")
    committed = _git(work, "rev-parse", "HEAD")
    before = _git(work, "rev-parse", "origin/main")
    generator.write_text("# unlanded successful no-op\n")
    dirty = _git(work, "status", "--porcelain")

    result = _run(work, "--allow-dirty", tool=local_tool)

    assert result.returncode == (4 if stale else 0), result.stdout + result.stderr
    assert _git(work, "rev-parse", "origin/main") == (before if stale else committed)
    assert _git(work, "status", "--porcelain") == dirty


def test_replace_ref_cannot_hide_committed_generator(work: Path) -> None:
    generator = _projection_fixture(work, stale=True)
    committed = _git(work, "rev-parse", "HEAD")
    before = _git(work, "rev-parse", "origin/main")
    generator.unlink()
    _git(work, "add", "tools/gen_proof_plan.py")
    _git(work, "commit", "-q", "-m", "replacement omits generator")
    replacement = _git(work, "rev-parse", "HEAD")
    _git(work, "switch", "--detach", committed)
    _git(work, "replace", committed, replacement)

    result = _run(work)

    assert result.returncode == 4, result.stdout + result.stderr
    assert "committed projection is stale" in result.stdout
    assert _git(work, "rev-parse", "origin/main") == before


def test_push_uses_validated_commit_when_local_head_advances_during_check(
    work: Path,
) -> None:
    _projection_fixture(
        work,
        after=(
            "import subprocess\n"
            f"subprocess.run(['git', '-C', {str(work)!r}, 'switch', '--quiet', 'main'], check=True)\n"
        ),
    )
    validated = _git(work, "rev-parse", "HEAD")
    _init_commit(work, "unreviewed.txt")
    advanced = _git(work, "rev-parse", "HEAD")
    _git(work, "switch", "--detach", validated)
    result = _run(work)
    assert result.returncode == 0, result.stdout + result.stderr
    assert _git(work, "rev-parse", "HEAD") == advanced
    assert _git(work, "rev-parse", "origin/main") == validated


@pytest.mark.parametrize("stream", ["stdout", "stderr"])
def test_foreign_push_rejection_reports_original_diagnostic(
    work: Path, stream: str
) -> None:
    _init_commit(work, "complete.txt")
    before = _git(work, "rev-parse", "origin/main")
    hook = work / ".git" / "hooks" / "pre-push"
    redirect = "" if stream == "stdout" else " >&2"
    hook.write_text(
        f"#!/bin/sh\nprintf '%s\\n' 'FOREIGN POLICY: review ticket missing'{redirect}\nexit 1\n",
        encoding="utf-8",
    )
    hook.chmod(0o755)
    result = _run(work)
    assert result.returncode == 1
    assert "FOREIGN POLICY: review ticket missing" in result.stdout
    assert "failed to push some refs" in result.stdout
    assert _git(work, "rev-parse", "origin/main") == before
    assert hook.read_text().endswith("exit 1\n")
