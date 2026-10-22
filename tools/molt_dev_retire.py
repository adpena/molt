#!/usr/bin/env python3
"""Worktree retirement for the canonical Molt dev driver (hazard 3).

A landed worktree leaves only when every source merge is ancestral to upstream,
every ordinary commit is represented by ancestry, Change-Id or patch-id, and
no tracked change remains. Retirement stops the checkout's own
daemons, binds the reclaim of its checkout state while the checkout still
exists, steps out of it, removes it through ``git worktree remove`` from the
surviving main checkout, and then reclaims its state (HF-313). Just before
the removal, each running Linux VM that still mounts host paths and holds
files under the checkout or its state roots releases them, so the removal
frees them at once (HF-463). ``integrate --cleanup-worktree``, ``cleanup``,
``lane retire`` and ``drift_harvest --prune`` share this one path.
"""

from __future__ import annotations

import argparse
import os
import subprocess
from collections.abc import Callable, Mapping
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
      * a commit of upstream_ref..HEAD is not represented on upstream
        (`landing_representation`): merges require actual ancestry; ordinary
        commits also admit Change-Id or patch-id, OR
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
    finish_retirement = prepare_checkout_retirement(checkout)
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
    finish_retirement()
    return EXIT_OK


def prepare_checkout_retirement(checkout: Path) -> Callable[[], None]:
    """Prepare a checkout's removal while it exists; return what runs after.

    Integration runs from the checkout it removes, so everything the returned
    callable needs is imported here: after `git worktree remove` this process
    can import nothing from it, and it can launch no guarded command, whose
    guard runs from the checkout's sources. So the Linux VM release runs now:
    each running VM that mounts host paths and holds files under the checkout
    or its state roots releases them (``tools/linux_vm.py``), and the removal
    frees those files at once. The returned callable reclaims the state roots;
    roots it cannot resolve stay for `tools/disk_guard.py --gc`. A failure
    never fails the cleanup.
    """
    import molt.build_state_layout  # noqa: F401 - the owner-key rule loads lazily
    from molt.checkout_state import (
        CheckoutOwner,
        checkout_state_roots,
        checkout_state_use,
    )
    from molt.process_environment import ProcessEnvironment
    from molt.dx import DxConfigError, artifact_root
    from tools import disk_guard

    environment = ProcessEnvironment(os.environ)
    removed = [checkout]
    state_root: Path | None = None
    try:
        state_root = artifact_root(checkout, environment)
        # Retirement owns this checkout's complete known inventory. A normal
        # command stamps only its selected target; the caller may legitimately
        # be running from a different checkout or using a different session.
        roots = checkout_state_roots(checkout, environment)
        with checkout_state_use(
            (root.path, CheckoutOwner(root.owner, root.kind)) for root in roots
        ):
            pass
    except (OSError, ValueError, DxConfigError) as exc:
        _warn(f"state roots of {checkout} were not stamped ({exc}); they stay")
        state_root = None
    else:
        removed.extend(root.path for root in roots)
    _release_guest_caches(checkout, removed, environment)

    def finish() -> None:
        # A root the guard defers (a held lock or a protected path) stays for
        # a later `tools/disk_guard.py --gc`.
        if state_root is None:
            return
        try:
            result = disk_guard.reclaim_checkout_state(
                checkout, root=state_root, env=environment
            )
        except (OSError, ValueError) as exc:
            _warn(f"state roots of {checkout} were not reclaimed ({exc}); they stay")
            return
        for item in result.reclaimed:
            _ok(f"reclaimed {item['kind']} {item['path']}")
        for item in result.skipped:
            _warn(f"kept {item['path']} ({item['reason']}); disk_guard --gc retries it")
        for error in result.errors:
            _warn(f"state root not reclaimed: {error}")

    return finish


def _release_guest_caches(
    checkout: Path, paths: list[Path], environment: Mapping[str, str]
) -> None:
    """Release the host files running Linux VMs pin under ``paths``.

    Advisory: whatever the import, the definitions, Lima, lsof or a guest
    raises is reported, and the cleanup goes on.
    """
    try:
        from tools import linux_vm

        releases = linux_vm.release_for_paths(
            paths, environment=environment, host=linux_vm.LocalHost(environment)
        )
    except (
        ImportError,
        OSError,
        ValueError,
        TypeError,
        KeyError,
        RuntimeError,
        subprocess.SubprocessError,
    ) as exc:
        _warn(
            f"Linux VMs may keep files of {checkout} allocated after its removal "
            f"({type(exc).__name__}: {exc}); run tools/linux_vm.py release"
        )
        return
    for item in releases:
        if item.error is not None or item.before is None or item.after is None:
            _warn(f"{item.name} keeps its pins of {checkout}: {item.error}")
            continue
        _ok(
            f"{item.name} released its pins of {checkout}: "
            f"{item.before.open_under_paths - item.after.open_under_paths} host "
            f"files released, {item.after.open_under_paths} open in the guest; "
            f"{item.before.deleted_files - item.after.deleted_files} deleted "
            f"files freed ({item.before.deleted_bytes - item.after.deleted_bytes} "
            "bytes)"
        )


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
