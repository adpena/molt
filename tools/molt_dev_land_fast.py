"""`molt_dev.py land --fast`: the integrator's one push to main (HF-701).

The "not rocket science rule" (bors, homu, Zuul, merge queues): main advances
only to the exact tree that passed its gates. This command merges main into a
managed lane, tests that merged tree on the landing's mechanical affected set
(the static gates and the precheck's try run: direct-reader pytest cohorts,
changed-crate Clippy and changed-crate lib tests in every declared cell),
writes a landing receipt, and pushes one landing commit, whose text the
receipt generates, with ``--force-with-lease=refs/heads/main:<base>``. A main
that moved is merged; the static gates and the try-run gates the move can
affect run again, and the receipt carries the rest from the earlier base.
See docs/design/landing_queue.md, section 9.
"""

from __future__ import annotations

import argparse
import json
import os
import time
from datetime import UTC, datetime
from enum import StrEnum
from pathlib import Path
from types import ModuleType
from typing import TYPE_CHECKING
from collections.abc import Callable

from tools import landing_receipt
from tools.driver_errors import EXIT_FAIL, EXIT_OK, DriverError
from tools.molt_dev_common import _fail, _ok, _say, _step, _warn

if TYPE_CHECKING:
    from tools.molt_dev import Git
    from tools.molt_dev_policy import SelectedGate

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
    prior: dict | None = None,
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
    if prior is not None:
        gates += _carried_records(prior, gates)
    # The crate-test gate's typed summary line, never its human text.
    from tools.check_changed_crate_tests import SCHEMA as CRATE_TESTS_SCHEMA

    unverified: list[str] = []
    narrowed: list[str] = []
    for gate in gates:
        log = gate["log"]
        if not log or not Path(str(log)).is_file():
            continue
        for line in (
            Path(str(log)).read_text(encoding="utf-8", errors="replace").splitlines()
        ):
            if not line.startswith("{"):
                continue
            try:
                summary = json.loads(line)
            except json.JSONDecodeError:
                continue
            if (
                not isinstance(summary, dict)
                or summary.get("schema") != CRATE_TESTS_SCHEMA
            ):
                continue
            unverified.extend(summary.get("unverified", ()))
            narrowed.extend(
                "[" + "+".join(cell["sources"]) + "] " + ", ".join(cell["narrowed"])
                for cell in summary.get("cells", ())
                if cell.get("narrowed")
            )
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
        postsubmit_only=precheck.postsubmit_only,
    )


def _changed(git: Git, old: str, new: str) -> list[str]:
    out = git._run(["diff", "--name-only", old, new]).stdout
    return [line for line in out.splitlines() if line]


def _interaction_rerun(
    git: Git,
    repo: Path,
    *,
    lane_paths: list[str],
    tested_base: str,
    base: str,
) -> Callable[[SelectedGate], bool]:
    """Which try-run gates main's move ``tested_base`` -> ``base`` can affect.

    A gate the lane passed on ``tested_base`` keeps its verdict on ``base``
    unless the move can change it: a pytest cohort that names a test file the
    move's own selection also runs, or a Cargo gate when the
    reverse-dependency closures of the lane's and the move's paths share a
    package (or either reaches the whole workspace or the toolchain). Any
    other try-run gate reruns. Chromium's commit queue and TAP make the same
    trade; postsubmit runs everything.
    """
    from tools.cargo_reach import Workspace, reach
    from tools.proof_plan import CargoTargetAccess
    from tools.molt_dev_policy import select_landing_gates
    from tools.molt_dev_queue import queue_paths, recent_postsubmit_failures

    moved_paths = _changed(git, tested_base, base)
    if not moved_paths:
        _say(
            f"    main moved {git.short(tested_base)} -> {git.short(base)} with no path; nothing reruns"
        )
        return lambda gate: False
    _policy, _config, _touched, moved_gates, _matched = select_landing_gates(
        git,
        repo,
        base,
        tested_base,
        extra=[],
        recent_failures=recent_postsubmit_failures(queue_paths(repo)),
    )
    moved_tests = {
        target for gate in moved_gates if gate.pytest for target in _pytest_files(gate)
    }
    cargo: list[bool] = []  # computed on the first Cargo gate

    def cargo_interacts() -> bool:
        if not cargo:
            workspace = Workspace.load(repo)
            lane, moved = reach(workspace, lane_paths), reach(workspace, moved_paths)
            cargo.append(
                lane.whole_workspace
                or moved.whole_workspace
                or lane.toolchain
                or moved.toolchain
                or bool(lane.packages & moved.packages)
                or bool(lane.projects & moved.projects)
            )
            _say(
                f"    main moved {git.short(tested_base)} -> {git.short(base)}: Cargo reach "
                + (
                    "shared; Cargo gates rerun"
                    if cargo[0]
                    else "disjoint; Cargo gates carried"
                )
            )
        return cargo[0]

    def rerun(gate: SelectedGate) -> bool:
        if gate.pytest:
            # Cohorts merge files by their worker envelope, so two selections
            # can share a file under different cohort commands.
            return not moved_tests.isdisjoint(_pytest_files(gate))
        # A Cargo gate's verdict can change only through a package both
        # reverse-dependency closures reach (a package it builds depends on
        # both changes), so disjoint closures carry it.
        if gate.cargo_target_access is not CargoTargetAccess.NONE:
            return cargo_interacts()
        return True

    return rerun


def _pytest_files(gate: SelectedGate) -> set[str]:
    """The test files a pytest gate names (a node id counts as its file)."""
    command = gate.command
    words = command.split() if isinstance(command, str) else list(command)
    return {
        word.partition("::")[0]
        for word in words
        if isinstance(word, str) and word.startswith("tests/")
    }


def _gate_identity(command: str) -> str:
    """A gate's shown command without its ``--base`` binding: the same gate
    on another base of main."""
    words = command.split()
    kept, skip = [], False
    for word in words:
        if skip:
            skip = False
            continue
        if word == "--base":
            skip = True
            continue
        kept.append(word)
    return " ".join(kept)


def _carried_records(prior: dict, rerun_records: list[dict]) -> list[dict]:
    """The prior receipt's try-run records the new run did not repeat, each
    marked with the base it passed on."""
    ran = {_gate_identity(str(record["command"])) for record in rerun_records}
    # A record carried twice keeps the base it actually passed on.
    return [
        {"carried_from_base": prior["base"], **record}
        for record in prior["gates"]
        if record["phase"] != "static"
        and _gate_identity(str(record["command"])) not in ran
    ]


def split_carried(
    gates: list[SelectedGate],
    prior: dict,
    rerun: Callable[[SelectedGate], bool],
) -> tuple[list[SelectedGate], list[dict]]:
    """The gates to run on the new base, and the prior receipt's records the
    rest carry.

    A gate is carried only when the prior receipt passed the same gate (its
    command less its ``--base``) outside the static phase and ``rerun`` says
    main's move cannot change its verdict; every other gate runs.
    """
    from tools.molt_dev_gates import display

    passed = {
        _gate_identity(str(record["command"])): record
        for record in prior["gates"]
        if record["phase"] != "static"
    }
    run: list[SelectedGate] = []
    carried: list[dict] = []
    for gate in gates:
        record = passed.get(_gate_identity(display(gate.command)))
        if record is None or gate.static or rerun(gate):
            run.append(gate)
        else:
            carried.append({"carried_from_base": prior["base"], **record})
    return run, carried


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
    # The last complete receipt of this run: a later attempt reruns only the
    # try-run gates main's move since its base can affect.
    prior: dict | None = None
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
            rerun = (
                None
                if prior is None
                else _interaction_rerun(
                    git,
                    repo,
                    lane_paths=_changed(git, base, head),
                    tested_base=str(prior["base"]),
                    base=base,
                )
            )
            precheck = precheck_landing(
                git,
                repo,
                head=head,
                upstream=base,
                session_id=session_id,
                budget=FAST_LANDING_BUDGET,
                rerun=rerun,
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
                prior=prior if precheck.carried else None,
            )
            path = landing_receipt.write_receipt(common_dir, receipt)
            _ok(f"receipt {path}")
        prior = receipt
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
            _sync_main_worktree(git, remote=args.remote, branch=args.branch)
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


def _sync_main_worktree(git, *, remote: str, branch: str) -> None:
    """Keep the main checkout a clean mirror of the branch just published.

    Sessions start in the main worktree and read its AGENTS.md, and nothing
    else moves it: it fell 2,446 commits behind (2026-11-07). A clean
    checkout on the branch fast-forwards; anything else is reported, never
    forced or reset.
    """
    main = git.worktree_paths()[0]
    mirror = type(git)(main)
    current = mirror._run(
        ["symbolic-ref", "--quiet", "--short", "HEAD"], check=False
    ).stdout.strip()
    if current != branch:
        _warn(
            f"main checkout {main} is on {current or 'a detached HEAD'}, not {branch}; not updated"
        )
        return
    if mirror._run(["status", "--porcelain", "--untracked-files=no"]).stdout.strip():
        _warn(f"main checkout {main} has uncommitted changes; not updated")
        return
    merged = mirror._run(
        ["merge", "--ff-only", "--quiet", f"{remote}/{branch}"], check=False
    )
    if merged.returncode != 0:
        _warn(
            f"main checkout {main} cannot fast-forward to {remote}/{branch}: "
            f"{merged.stderr.strip()}"
        )
        return
    _ok(f"main checkout {main} is at {remote}/{branch}")


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
    """What a landing would do, from the lane as it is: no merge, no gate.

    Selection diffs the lane against its merge base with main, which is the
    lane's own change once main is merged in."""
    from tools.git_identity import merge_base
    from tools.molt_dev_gates import display
    from tools.molt_dev_policy import left_to_postsubmit, select_landing_gates

    merge = (
        "none"
        if git.is_ancestor(base, head)
        else f"merge {git.short(base)} into the lane"
    )
    _say(f"    merge: {merge}")
    fork = merge_base(head, base, run_git=lambda args: git._run(args).stdout)
    _policy, config, touched, gates, matched = select_landing_gates(
        git, repo, head, fork, extra=[]
    )
    _say(
        f"    {len(touched)} touched path(s); matched rules: "
        f"{', '.join(r.name for r in matched) or 'none'}"
    )
    graph = config.graph_selection(touched)
    if graph is not None:
        _say(
            f"    dependency graph: {len(graph.selected)} test file(s) read a touched "
            f"path; {len(graph.run)} run in the try run"
        )
        for test, cells in sorted(graph.deferred.items()):
            _say(
                f"    deferred  {test}  ({', '.join(cells) or 'no proof cell owns it'})"
            )
        for test in left_to_postsubmit(graph, gates):
            _say(f"    postsubmit  {test}  (data reader; presubmit runs importers)")
    for gate in gates:
        role = "static" if gate.static else "try-run" if gate.try_run else "waived"
        _say(f"    {role:8} {display(gate.command)}")
    _ok("dry run: nothing merged, tested or published")
    return EXIT_OK
