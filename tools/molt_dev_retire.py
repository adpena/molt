#!/usr/bin/env python3
"""Worktree retirement for the canonical Molt dev driver (hazard 3).

A worktree leaves only when every commit is on upstream (by Change-Id or
patch-id) and no tracked change remains. Retirement stops the checkout's own
daemons, binds the reclaim of its checkout state while the checkout still
exists, steps out of it, removes it through ``git worktree remove`` from the
surviving main checkout, and then reclaims its state (HF-313). ``integrate
--cleanup-worktree``, ``cleanup`` and ``lane retire`` share this one path.
"""

from __future__ import annotations

import argparse
import os
from collections.abc import Callable
from pathlib import Path
from types import ModuleType

from tools.dirty_tree_policy import (
    DEFAULT_DIRTY_TREE_IGNORE_GLOBS as DEFAULT_IGNORE_GLOBS,
)
from tools.molt_dev_change_ids import landing_representation
from tools.molt_dev_common import (
    EXIT_FAIL,
    EXIT_OK,
    _fail,
    _ok,
    _run_fast_captured_command,
    _step,
    _warn,
)


def _driver() -> ModuleType:
    """The canonical driver; imported late because it imports this module."""
    from tools import molt_dev

    return molt_dev


def cleanup_worktree(
    git,
    repo: Path,
    upstream_ref: str,
    ignore_globs: tuple[str, ...],
    *,
    force_sha: str | None,
) -> int:
    """Remove the worktree at `repo` ONLY when safe (hazard 3).

    REFUSES when:
      * a commit of upstream_ref..HEAD is not on upstream by Change-Id or
        patch-id (`landing_representation`), OR
      * tracked staged/unstaged (non-ignored) changes exist.
    --force (force_sha set) overrides BOTH, but only when force_sha names the
    exact HEAD being abandoned (so an abandon is deliberate + auditable).
    """
    drv = _driver()
    head = git.head_sha()
    unpushed = list(landing_representation(git, head, upstream_ref).unrepresented)
    dirty = drv._tracked_dirty(git, ignore_globs)

    if unpushed or dirty:
        reasons: list[str] = []
        if unpushed:
            reasons.append(
                f"{len(unpushed)} commit(s) not on {upstream_ref}: "
                + ", ".join(git.short(s) for s in unpushed)
            )
        if dirty:
            reasons.append(
                f"{len(dirty)} tracked change(s): "
                + ", ".join(f"{xy} {p}" for xy, p in dirty)
            )
        if force_sha is None:
            _fail(
                "REFUSING worktree cleanup (hazard 3): "
                + "; ".join(reasons)
                + f". HEAD={git.short(head)}. To abandon anyway, pass "
                f"--force {git.short(head)} (naming the sha makes the abandon "
                "deliberate)."
            )
            return EXIT_FAIL
        if git.rev_parse(force_sha) != head:
            _fail(
                f"--force {force_sha} does not match HEAD {git.short(head)}; "
                "refusing (the force target must name the exact abandoned sha)."
            )
            return EXIT_FAIL
        _warn(
            f"--force: abandoning worktree at HEAD {git.short(head)} despite: "
            + "; ".join(reasons)
        )

    # Safe (or force-confirmed): remove the worktree via git (so the admin
    # bookkeeping in the main repo's .git/worktrees is updated too).
    main_repo = drv._main_worktree(git)
    if main_repo is None:
        _fail(
            "could not determine the main worktree to run `git worktree remove` "
            "from; refusing to delete files directly."
        )
        return EXIT_FAIL
    if main_repo.resolve() == repo.resolve():
        _fail("refusing to remove the MAIN worktree; this targets disposable worktrees")
        return EXIT_FAIL
    retire_checkout_daemons(repo)
    checkout = repo.resolve()
    # A landing runs `integrate --repo .` from the checkout it removes. Step
    # out first: after the removal, anything that resolves against this
    # process's working directory (`Path.cwd()`) would find it deleted.
    if Path(os.getcwd()).resolve().is_relative_to(checkout):
        os.chdir(main_repo)
    reclaim_checkout_state = prepare_checkout_state_retirement(checkout)
    remove_args = ["worktree", "remove"]
    # `git worktree remove` refuses (without --force) on ANY modified/untracked
    # file, including the ignored-set churn (e.g. legacy/keyed wasm sha sidecars) our OWN
    # safety policy already cleared. Our policy is the authority: if we got here
    # with residual churn that is purely ignored/untracked, OR the operator
    # passed --force, tell git to proceed. We never reach here with non-ignored
    # tracked changes (those are refused above), so this --force can only be
    # waving through churn we already deemed safe.
    has_residual_churn = bool(git.status_porcelain())
    if force_sha is not None or has_residual_churn:
        remove_args.append("--force")
    remove_args.append(str(repo))
    proc = _run_fast_captured_command(
        ["git", "-C", str(main_repo), *remove_args],
        # Git for Windows may retain the launch cwd in its executable shim.
        # Both that cwd and Git's -C must outlive the worktree being removed.
        cwd=main_repo,
        timeout=120.0,
    )
    if proc.returncode != 0:
        _fail(
            f"git worktree remove failed (exit {proc.returncode}): "
            f"{proc.stderr.strip()}"
        )
        return EXIT_FAIL
    _ok(f"removed worktree {repo}")
    if reclaim_checkout_state is not None:
        reclaim_checkout_state()
    return EXIT_OK


def prepare_checkout_state_retirement(checkout: Path) -> Callable[[], None] | None:
    """Stamp a retiring checkout's state roots and bind their reclaim, now.

    Integration runs from the checkout it removes, so everything the reclaim
    needs is imported here, while that checkout still exists: after `git
    worktree remove` this process can import nothing from it. Returns None
    when the artifact root cannot be resolved; the roots then stay for
    `tools/disk_guard.py --gc`.
    """
    import molt.build_state_layout  # noqa: F401 - the owner-key rule loads lazily
    from molt.checkout_state import stamp_checkout_state
    from molt.dx import DxConfigError, artifact_root
    from tools import disk_guard

    try:
        stamp_checkout_state(checkout, os.environ)
        state_root = artifact_root(checkout, os.environ)
    except (OSError, ValueError, DxConfigError) as exc:
        _warn(f"state roots of {checkout} were not stamped ({exc}); they stay")
        return None

    def reclaim() -> None:
        # A root the guard defers (a held lock or a protected path) stays for
        # a later `tools/disk_guard.py --gc`; a failure never fails cleanup.
        try:
            result = disk_guard.reclaim_checkout_state(checkout, root=state_root)
        except (OSError, ValueError) as exc:
            _warn(f"state roots of {checkout} were not reclaimed ({exc}); they stay")
            return
        for item in result.reclaimed:
            _ok(f"reclaimed {item['kind']} {item['path']}")
        for item in result.skipped:
            _warn(f"kept {item['path']} ({item['reason']}); disk_guard --gc retries it")
        for error in result.errors:
            _warn(f"state root not reclaimed: {error}")

    return reclaim


def retire_checkout_daemons(repo: Path) -> None:
    """Retire the backend daemons of a checkout about to be removed.

    They would otherwise hold their memory until their idle timeout. A failure
    leaves them to that timeout and does not block the cleanup.
    """
    from molt.cli.backend_execution import retire_project_backend_daemons

    try:
        pids = retire_project_backend_daemons(repo, os.environ)
    except (OSError, ValueError) as exc:
        _warn(
            f"backend daemons of {repo} were not retired ({exc}); they retire when idle"
        )
        return
    if pids:
        _ok(f"retired {len(pids)} backend daemon(s) of {repo}: pids {list(pids)}")


def cmd_cleanup(args: argparse.Namespace) -> int:
    drv = _driver()
    repo = Path(args.repo).resolve()
    git = drv.Git(repo)
    upstream_ref = f"{args.remote}/{args.branch}"
    ignore_globs = tuple(DEFAULT_IGNORE_GLOBS) + tuple(args.ignore or ())
    _step(f"cleanup worktree {repo} (upstream {upstream_ref})")
    # Refresh upstream so the unpushed check is honest.
    git.fetch(args.remote, args.branch)
    return cleanup_worktree(git, repo, upstream_ref, ignore_globs, force_sha=args.force)
