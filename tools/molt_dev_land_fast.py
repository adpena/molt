"""`molt_dev.py land --fast`: the integrator's one push to main (HF-701).

The "not rocket science rule" (bors, homu, Zuul, merge queues): main advances
only to the exact tree that passed its gates. This command merges main into a
managed lane, tests that merged tree on the landing's mechanical affected set
(the static gates and the precheck's try run: direct-reader pytest cohorts,
changed-crate Clippy and changed-crate lib tests in every declared cell),
writes a landing receipt, and pushes one landing commit, whose text the
receipt generates, with ``--force-with-lease=refs/heads/main:<base>``. A main
that moved is merged and tested again. See docs/design/landing_queue.md,
section 9.
"""

from __future__ import annotations

import argparse
import os
import time
from datetime import UTC, datetime
from enum import StrEnum
from pathlib import Path
from types import ModuleType

from tools import landing_receipt
from tools.driver_errors import EXIT_FAIL, EXIT_OK, DriverError
from tools.molt_dev_common import _fail, _ok, _say, _step, _warn

# A move of main between test and publication costs one more full test; three
# attempts bound a run while another actor keeps publishing.
MAX_ATTEMPTS = 3
# The queue runner holds the integration lock for one batch; publication waits
# for it rather than racing it.
LOCK_WAIT_SECONDS = 3600.0
MAIN_REF = "refs/heads/main"


def _driver() -> ModuleType:
    from tools import molt_dev

    return molt_dev


class Publication(StrEnum):
    LANDED = "landed"
    MAIN_MOVED = "main moved"


def _merge_main(git, repo: Path, base: str) -> str:
    """Merge ``base`` into the lane unless it is already an ancestor; a
    conflict that only generated outputs own is regenerated, any other aborts."""
    drv = _driver()
    head = drv.require_git_object_id(git.head_sha(), label="lane head")
    if git.is_ancestor(base, head):
        _ok(f"lane already contains {git.short(base)}; no merge")
        return head
    _step(f"merge {git.short(base)} into the lane")
    merged = git._run(["merge", "--no-ff", "--no-edit", base], check=False)
    if merged.returncode != 0:
        from tools.generated_merge import GeneratedMergeError, resolve_generated_merge

        try:
            resolved = resolve_generated_merge(
                lambda arguments: git._run(arguments, check=False)
            )
        except (GeneratedMergeError, OSError, ValueError) as exc:
            resolved = False
            _warn(f"generated-merge resolution failed: {exc}")
        if not resolved:
            if (
                git._run(
                    ["rev-parse", "-q", "--verify", "MERGE_HEAD"], check=False
                ).returncode
                == 0
            ):
                git._run(["merge", "--abort"])
            raise DriverError(
                f"merging {git.short(base)} into the lane conflicts outside "
                "generated outputs; the merge was aborted. Resolve it in the lane "
                "(python -B tools/generated_merge.py merge origin/main), then land "
                f"again.\n{merged.stdout.strip()}\n{merged.stderr.strip()}"
            )
    merged_head = drv.require_git_object_id(git.head_sha(), label="merged lane head")
    drv._assert_integration_source(git, merged_head)
    _ok(f"merged: {git.short(head)} -> {git.short(merged_head)}")
    return merged_head


def _receipt_for(
    precheck,
    *,
    base: str,
    tree: str,
    head: str,
    controller: str | None,
    started_at: str,
) -> dict:
    from tools.molt_dev_gates import display

    seconds = dict(precheck.seconds)
    gates = [
        landing_receipt.gate_record(
            phase=run.phase,
            command=display(gate.command),
            seconds=seconds.get(display(gate.command)),
            log=run.log(gate),
        )
        for run in precheck.runs
        for gate in run.gates
    ]
    unverified: list[str] = []
    narrowed: list[str] = []
    for gate in gates:
        log = gate["log"]
        if not log or not Path(str(log)).is_file():
            continue
        for line in (
            Path(str(log)).read_text(encoding="utf-8", errors="replace").splitlines()
        ):
            if line.startswith("changed-crate-tests: unverified: "):
                unverified.append(
                    line.removeprefix("changed-crate-tests: unverified: ")
                )
            elif line.startswith("changed-crate-tests:   narrowed: "):
                narrowed.append(line.removeprefix("changed-crate-tests:   narrowed: "))
    return landing_receipt.build_receipt(
        mode="fast",
        base=base,
        tree=tree,
        tested_commit=head,
        controller=controller,
        started_at=started_at,
        gates=gates,
        waived=[display(gate.command) for gate in precheck.waived],
        unverified=unverified,
        narrowed=narrowed,
    )


def _remote_main(git, remote: str) -> str | None:
    listed = git._run(["ls-remote", remote, MAIN_REF], check=False)
    if listed.returncode != 0:
        return None
    for line in listed.stdout.splitlines():
        fields = line.split()
        if len(fields) == 2 and fields[1] == MAIN_REF:
            return fields[0]
    return None


def _publish(
    git,
    repo: Path,
    *,
    remote: str,
    branch: str,
    base: str,
    head: str,
    tree: str,
    receipt: dict,
    subject: str,
    common_dir: Path,
) -> Publication:
    """Commit the landing commit and push it with the lease, under the
    repository integration lock. Returns LANDED or MAIN_MOVED; raises when the
    outcome cannot be established (nothing is reset then)."""
    drv = _driver()
    lock_path = drv._integration_lock_path(repo)
    _step("publish under the repository integration lock")
    handle = drv._try_acquire_file_lock(lock_path)
    if handle is None:
        _say("    waiting for the repository integration lock (a queue landing)")
        handle = drv._acquire_file_lock(
            lock_path,
            timeout_s=LOCK_WAIT_SECONDS,
            timeout_message="the integration lock stayed held; nothing was published",
            poll_s=2.0,
        )
    try:
        git.fetch(remote, branch)
        current = git.rev_parse(f"{remote}/{branch}")
        if current != base:
            _warn(f"{remote}/{branch} moved {git.short(base)} -> {git.short(current)}")
            return Publication.MAIN_MOVED
        if head != receipt["tested_commit"] and head in receipt["commits"]:
            landing = head  # an earlier run's unpublished landing commit
        else:
            message = landing_receipt.commit_message(subject, receipt)
            committed = git._run(
                ["commit", "--quiet", "--allow-empty", "-F", "-"],
                check=False,
                input_text=message,
            )
            landing = git.head_sha()
            shape = git._run(
                ["rev-parse", "HEAD^{tree}", "HEAD^@"], check=False
            ).stdout.split()
            if committed.returncode != 0 or shape != [tree, head]:
                if landing != head:
                    git._run(["reset", "--quiet", "--keep", head])
                raise DriverError(
                    "the landing commit failed or changed the tested tree or parent; "
                    f"the lane is back at {git.short(head)}:\n"
                    f"{committed.stdout.strip()}\n{committed.stderr.strip()}"
                )
            landing_receipt.add_commit(common_dir, tree=tree, base=base, commit=landing)
        _step(
            f"push {git.short(landing)} to {remote}/{branch} with the lease on {git.short(base)}"
        )
        pushed = git._run(
            [
                "push",
                f"--force-with-lease={MAIN_REF}:{base}",
                remote,
                f"{landing}:{MAIN_REF}",
            ],
            check=False,
        )
        if pushed.returncode != 0:
            _warn(
                f"git push exited {pushed.returncode}; reading the remote instead "
                f"(hazard 2)\n{pushed.stdout.strip()}\n{pushed.stderr.strip()}"
            )
        observed = _remote_main(git, remote)
        if observed == landing:
            git.fetch(remote, branch)
            if not git.is_ancestor(landing, git.rev_parse(f"{remote}/{branch}")):
                raise DriverError(
                    f"{remote} reports {MAIN_REF} = {landing}, but the fetched "
                    f"{remote}/{branch} does not contain it; investigate before landing again"
                )
            _ok(f"confirmed: {remote}/{branch} is {git.short(landing)}")
            return Publication.LANDED
        if observed is None:
            raise DriverError(
                f"cannot read {remote} {MAIN_REF} after the push; the landing commit "
                f"{git.short(landing)} is kept; run land --fast again"
            )
        if observed == base:
            raise DriverError(
                f"{remote} still holds {git.short(base)}: the push was refused "
                f"(lease or hook); the landing commit {git.short(landing)} is kept"
            )
        if git.head_sha() == landing:
            git._run(["reset", "--quiet", "--keep", head])
        _warn(f"main moved to {observed[:12]} during the push")
        return Publication.MAIN_MOVED
    finally:
        drv._release_file_lock(handle)


def cmd_land_fast(args: argparse.Namespace) -> int:
    drv = _driver()
    from tools.molt_dev_controller import integration_controller
    from tools.molt_dev_lanes import validate_lane_handoff
    from tools.molt_dev_policy import FAST_LANDING_BUDGET
    from tools.molt_dev_precheck import precheck_landing, repair_landing
    from tools.molt_dev_change_ids import resolve_change_references

    repo = Path(args.repo).resolve()
    git = drv.Git(repo)
    started = time.monotonic()
    head = drv.require_git_object_id(git.head_sha(), label="landing source")
    drv._assert_integration_source(git, head)
    lane = git.symbolic_head()
    if lane is None:
        raise DriverError("land --fast merges into a branch; this checkout is detached")
    subject = (args.subject or "").strip()
    if not (args.no_push or args.dry_run) and not subject:
        raise DriverError(
            "land --fast needs --subject: the landing commit's first line"
        )
    common_dir = drv._git_common_directory(repo)
    if common_dir is None:
        raise DriverError("cannot resolve the Git common directory")
    controller = integration_controller(args)
    if lane.startswith("lane/"):
        # A merged dependency refresh leaves the lane's inputs stale (as `land`).
        for item in validate_lane_handoff(git, lane, repo, head, provision=True):
            if item.provisioned:
                _ok(f"{item.name}: provisioned ({item.seconds:.2f} s)")
    session_id = os.environ.get("MOLT_SESSION_ID", "devtool")
    for attempt in range(1, MAX_ATTEMPTS + 1):
        _step(f"land --fast attempt {attempt} of {MAX_ATTEMPTS} [{controller}]")
        git.fetch(args.remote, args.branch)
        base = drv.require_git_object_id(
            git.rev_parse(f"{args.remote}/{args.branch}"), label="landing base"
        )
        if args.dry_run:
            return _dry_run(git, repo, head=git.head_sha(), base=base)
        head = _merge_main(git, repo, base)
        head = repair_landing(
            git, repo, head=head, upstream=base, session_id=session_id
        )
        head = resolve_change_references(
            git,
            repo,
            base,
            head,
            dry=False,
            assert_source=lambda commit: drv._assert_integration_source(git, commit),
            amend=False,
        )
        tree = git.rev_parse(f"{head}^{{tree}}")
        receipt = landing_receipt.read_receipt(common_dir, tree=tree, base=base)
        if receipt is not None and head in receipt["commits"]:
            _ok(
                f"receipt for tree {tree[:12]} on base {base[:12]} lists {git.short(head)}; not testing it again"
            )
        else:
            started_at = datetime.now(UTC).isoformat(timespec="seconds")
            precheck = precheck_landing(
                git,
                repo,
                head=head,
                upstream=base,
                session_id=session_id,
                budget=FAST_LANDING_BUDGET,
            )
            if not precheck.complete:
                _fail(
                    f"land --fast pushes only a complete run; {git.short(head)} is not landed"
                )
                return EXIT_FAIL
            drv._assert_integration_source(git, head)
            receipt = _receipt_for(
                precheck,
                base=base,
                tree=tree,
                head=head,
                controller=controller,
                started_at=started_at,
            )
            path = landing_receipt.write_receipt(common_dir, receipt)
            _ok(f"receipt {path}")
        if args.no_push:
            _ok(
                f"tested {git.short(head)} on {git.short(base)} in "
                f"{time.monotonic() - started:.0f} s; --no-push: nothing published"
            )
            return EXIT_OK
        outcome = _publish(
            git,
            repo,
            remote=args.remote,
            branch=args.branch,
            base=base,
            head=head,
            tree=tree,
            receipt=receipt,
            subject=subject,
            common_dir=common_dir,
        )
        if outcome is Publication.LANDED:
            _ok(f"landed in {time.monotonic() - started:.0f} s")
            if lane.startswith("lane/"):
                _install_hooks(repo)
                _say(
                    "    retire the lane: python -B tools/molt_dev.py lane retire "
                    + lane.removeprefix("lane/")
                )
            return EXIT_OK
        head = git.head_sha()
    _fail(f"main moved during each of {MAX_ATTEMPTS} attempts; nothing was published")
    return EXIT_FAIL


def _install_hooks(repo: Path) -> None:
    """The lane now equals main, so its hooks are main's: install them, which
    turns on the pre-push receipt check for every worktree (section 9.4.4)."""
    from molt import checkout_provisioning
    from tools.molt_dev_lanes import lane_environment

    drv = _driver()
    try:
        for item in checkout_provisioning.ensure_all(
            repo,
            lane_environment(os.environ),
            ("git-hooks",),
            guard_source_root=drv._COMMANDS.repo_root,
        ):
            if item.provisioned:
                _ok(f"{item.name}: installed from the landed main")
    except checkout_provisioning.ProvisioningError as exc:
        _warn(f"the managed hooks were not installed: {exc}")


def _dry_run(git, repo: Path, *, head: str, base: str) -> int:
    """What a landing would do, from the lane as it is: no merge, no gate."""
    from tools.molt_dev_policy import select_landing_gates
    from tools.molt_dev_gates import display

    merge = (
        "none"
        if git.is_ancestor(base, head)
        else f"merge {git.short(base)} into the lane"
    )
    _say(f"    merge: {merge}")
    _policy, _config, touched, gates, matched = select_landing_gates(
        git, repo, head, base, extra=[]
    )
    _say(
        f"    {len(touched)} touched path(s); matched rules: {', '.join(r.name for r in matched) or 'none'}"
    )
    for gate in gates:
        role = "static" if gate.static else "try-run" if gate.try_run else "waived"
        _say(f"    {role:8} {display(gate.command)}")
    _ok("dry run: nothing merged, tested or published")
    return EXIT_OK
