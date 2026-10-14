#!/usr/bin/env python3
"""Fail-closed fast-forward land: validate and push the captured HEAD commit
only when it is a fast-forward of the remote branch.

This encapsulates the drift-safe landing guard that the worktree/ff-push pattern
in ``docs/agent/ORCHESTRATION.md`` requires and that is otherwise hand-written
(and easy to get subtly wrong -- e.g. ``git commit -- files -m msg`` silently
treats ``-m msg`` as a pathspec, so the commit never happens and a later push
lands nothing or the wrong thing). It complements:

  * ``tools/tree_drift_check.py``      -- is my tree stale/masking? (detection)
  * ``tools/dirty_tree_landing_audit.py`` -- did every dirty path make the range? (coverage)
  * this tool                          -- is landing HEAD a clean fast-forward? (the push guard)

It fails CLOSED (never lands) when any of these hold, naming the reason:
  * without --allow-dirty, the working tree has uncommitted TRACKED changes
    (commit the intended paths before landing);
  * ``<remote>/<branch>`` is NOT an ancestor of HEAD -- i.e. it advanced under
    you and landing would require a non-fast-forward (rebase first).
  * the selected commit's proof-plan projection is stale relative to that
    commit's declared authority inputs.

Nothing to land is a successful no-op. Only a genuine fast-forward is pushed.
A fast-forward cannot trample the
remote branch, so a successful land here is trample-safe by construction.

Usage::

    python tools/ff_land.py [--remote origin] [--branch main] [--dry-run]
                            [--allow-dirty] [--quiet]
"""

from __future__ import annotations

import argparse
import os
import subprocess
from pathlib import Path

try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

from tools.release.git_source_snapshot import (  # noqa: E402
    immutable_git_environment,
)


def _git(
    root: Path, *args: str, check: bool = True
) -> subprocess.CompletedProcess[str]:
    return _COMMANDS.run(
        ["git", *args],
        cwd=root,
        env=immutable_git_environment(os.environ),
        check=check,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )


def _out(root: Path, *args: str) -> str:
    return _git(root, *args).stdout.strip()


def _repo_root() -> Path:
    return Path(_out(Path.cwd(), "rev-parse", "--show-toplevel"))


def _tracked_dirty(root: Path) -> list[str]:
    """Paths with staged/unstaged modifications (excludes untracked ?? entries).

    Read raw (no strip) because porcelain columns are position-sensitive.
    """
    out = _git(root, "status", "--porcelain").stdout
    dirty: list[str] = []
    for line in out.split("\n"):
        if not line.strip() or line.startswith("??"):
            continue
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        dirty.append(path)
    return dirty


def _proof_plan_fresh(root: Path, source_commit: str) -> tuple[bool, str]:
    """Check the captured commit's generated-proof authority when present."""

    from molt.compiler_distribution import MAX_SOURCE_BYTES, MAX_SOURCE_FILES
    from molt.temporary_artifacts import OwnedTemporaryDirectory
    from molt.toolchain_identity import resolve_executable
    from tools.release.git_source_snapshot import (
        capture_git_source_snapshot,
        materialize_git_source_snapshot,
        run_git_source_python,
    )

    try:
        present = _out(
            root,
            "ls-tree",
            "--name-only",
            source_commit,
            "--",
            "tools/gen_proof_plan.py",
        )
        if not present:
            return True, ""
        environment = immutable_git_environment(os.environ)
        git = resolve_executable("git", environment=environment, label="landing Git")
        snapshot = capture_git_source_snapshot(
            root,
            source_commit,
            git=git,
            environment=environment,
            required_markers=frozenset(
                {"tools/gen_proof_plan.py", "tools/proof_plan.toml"}
            ),
            max_files=MAX_SOURCE_FILES,
            max_bytes=MAX_SOURCE_BYTES,
        )
        with OwnedTemporaryDirectory(prefix="molt-landing-proof-") as temporary:
            captured_root = materialize_git_source_snapshot(
                snapshot,
                Path(temporary) / "source",
                repo_root=root,
                git=git,
                environment=environment,
            )
            result = run_git_source_python(
                captured_root,
                "tools/gen_proof_plan.py",
                ["--check"],
                environment=environment,
                timeout=120,
            )
            snapshot.verify(captured_root)
        output = (result.stdout + result.stderr).strip()
        return result.returncode == 0, output
    except (OSError, UnicodeError, ValueError, subprocess.SubprocessError) as exc:
        return False, str(exc)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--remote", default="origin")
    parser.add_argument("--branch", default="main")
    parser.add_argument(
        "--dry-run", action="store_true", help="check + report, do not push"
    )
    parser.add_argument(
        "--allow-dirty",
        action="store_true",
        help="permit uncommitted tracked changes; validate and push only the captured commit",
    )
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args(argv)

    root = _repo_root()
    target = f"{args.remote}/{args.branch}"

    dirty = _tracked_dirty(root)
    if dirty and not args.allow_dirty:
        print(
            f"FF-LAND REFUSED: {len(dirty)} uncommitted tracked change(s) — commit by pathspec first "
            f"(git commit -m MSG -- <files>); NOTE '-m' must come BEFORE '--'."
        )
        for p in dirty[:20]:
            print(f"  ! dirty {p}")
        return 2

    try:
        _git(root, "fetch", args.remote, "--quiet")
    except subprocess.CalledProcessError as exc:
        print(
            f"FF-LAND ERROR: git fetch {args.remote} failed: {exc.stderr.strip() if exc.stderr else exc}"
        )
        return 3

    try:
        source_commit = _out(root, "rev-parse", "HEAD")
        target_commit = _out(root, "rev-parse", target)
        head = source_commit[:9]
        remote_sha = target_commit[:9]
    except subprocess.CalledProcessError as exc:
        print(
            f"FF-LAND ERROR: cannot resolve {target}: {exc.stderr.strip() if exc.stderr else exc}"
        )
        return 3

    ahead = int(_out(root, "rev-list", "--count", f"{target_commit}..{source_commit}"))
    behind = int(_out(root, "rev-list", "--count", f"{source_commit}..{target_commit}"))

    if ahead == 0:
        print(
            f"FF-LAND: nothing to land — HEAD={head} has 0 commits ahead of {target}={remote_sha} (behind={behind})."
        )
        return 0

    is_ff = (
        _git(
            root,
            "merge-base",
            "--is-ancestor",
            target_commit,
            source_commit,
            check=False,
        ).returncode
        == 0
    )
    if not is_ff or behind != 0:
        print(
            f"FF-LAND REFUSED (DRIFT): {target} advanced to {remote_sha} ({behind} commit(s) you lack); "
            f"HEAD={head} is NOT a fast-forward. Rebase onto {target}, re-verify, then re-run."
        )
        if not args.quiet:
            for line in _out(
                root, "log", "--oneline", f"{source_commit}..{target_commit}"
            ).splitlines()[:15]:
                print(f"  ~ (theirs) {line}")
        return 1

    proof_plan_fresh, proof_plan_output = _proof_plan_fresh(root, source_commit)
    if not proof_plan_fresh:
        print(
            "FF-LAND REFUSED (GENERATED DRIFT): proof-plan projection is stale; "
            "stage the changed authority inputs, run "
            "`python tools/gen_proof_plan.py --from-index --write`, "
            "commit both generated outputs, "
            "and re-verify."
        )
        if proof_plan_output and not args.quiet:
            for line in proof_plan_output.splitlines()[:20]:
                print(f"  ! {line}")
        return 4

    # Clean fast-forward: safe to land.
    if args.dry_run:
        print(
            f"FF-LAND OK (dry-run): HEAD={head} fast-forwards {target}={remote_sha} by {ahead} commit(s); not pushed."
        )
        return 0

    push = _git(
        root, "push", args.remote, f"{source_commit}:{args.branch}", check=False
    )
    if push.returncode != 0:
        print("FF-LAND ERROR: push rejected")
        # Hooks may explain rejection on either stream. Preserve both instead
        # of reducing the failure to Git's generic stderr summary.
        if push.stdout:
            print(push.stdout, end="" if push.stdout.endswith("\n") else "\n")
        if push.stderr:
            print(push.stderr, end="" if push.stderr.endswith("\n") else "\n")
        return 1
    new_remote = _out(root, "rev-parse", "--short", target)
    print(
        f"FF-LAND OK: pushed {ahead} commit(s) {remote_sha}..{head} -> {target} (now {new_remote})."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
