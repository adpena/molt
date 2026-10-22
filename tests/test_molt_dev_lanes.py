"""The lane command family (tools/molt_dev_lanes.py) on real Git repositories.

Fixtures follow the checkout-family layout of `molt.custody_layout`: a bare
origin, a main checkout at ``<root>/molt-src`` and lanes under
``<root>/worktrees``. Git plumbing, the filesystem and the coordination
records are the oracles. The provisioning table is replaced by fixture inputs
whose provisioners log each run; `molt.checkout_provisioning` still runs them.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import sys

import pytest

from molt import checkout_provisioning as cp
from molt.process_environment import ProcessEnvironment
from tests.test_molt_dev import _commit_file, _git, _init_repo
from tools import molt_dev, molt_dev_lanes as lanes
from tools.molt_dev_change_ids import landing_representation
from tools.molt_dev_common import DriverError

CHANGE_A = "I" + "a1" * 20
CHANGE_B = "I" + "b2" * 20

_PROVISIONER = """\
import sys
from pathlib import Path
name = sys.argv[1]
tree = Path(f"{name}-tree")
tree.mkdir(exist_ok=True)
with (tree / "runs").open("a", encoding="utf-8") as log:
    log.write("run\\n")
(tree / "ready").write_text(Path("fixture.lock").read_text(encoding="utf-8"), encoding="utf-8")
"""

# Installs into the Git common directory, as tools/install_git_hooks.py does.
_INSTALLER = """\
import subprocess, sys
from pathlib import Path
common = subprocess.run(
    ["git", "rev-parse", "--git-common-dir"], capture_output=True, text=True, check=True
).stdout.strip()
installed = Path(common).resolve() / "molt-fixture-hook.txt"
if sys.argv[1:] == ["--check"]:
    raise SystemExit(0 if installed.is_file() else 1)
installed.write_text("hook\\n", encoding="utf-8")
"""


@pytest.fixture
def family(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """origin.git, <root>/molt-src and an empty <root>/worktrees."""
    for key in ("MOLT_VENV", "UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV"):
        monkeypatch.delenv(key, raising=False)
    seed = tmp_path / "seed"
    _init_repo(seed)
    (seed / "tools").mkdir()
    for relative, text in (
        ("provision.py", _PROVISIONER),
        ("fixture.lock", "v1\n"),
        ("tools/install_git_hooks.py", _INSTALLER),
        (".gitignore", "*-tree/\n"),
    ):
        (seed / relative).write_text(text, encoding="utf-8")
    _git(seed, "add", "--", ".")
    _commit_file(seed, "README.md", "seed\n", "seed")
    origin = tmp_path / "origin.git"
    _git(tmp_path, "clone", "--bare", "-q", str(seed), str(origin))
    root = tmp_path / "family"
    main = root / "molt-src"
    root.mkdir()
    _git(root, "clone", "-q", str(origin), str(main))
    _git(main, "config", "user.email", "dev@molt.test")
    _git(main, "config", "user.name", "Molt Dev")

    def fixture_input(name, checkout, environ=None, *, guard_source_root=None):
        checkout = Path(checkout).resolve()
        environment = ProcessEnvironment(os.environ if environ is None else environ)
        if name == "git-hooks":
            return cp.OwnedInstallation(
                name=name,
                guard_source_root=guard_source_root,
                checkout=checkout,
                owner="tools/install_git_hooks.py",
                environment=environment,
            )
        python = Path(sys.executable)
        return cp.LockedTree(
            name=name,
            guard_source_root=guard_source_root,
            checkout=checkout,
            lock_files=("fixture.lock",),
            tree=checkout / f"{name}-tree",
            sentinel="ready",
            tool=python.stem,
            arguments=("provision.py", name),
            environment=environment.updated(
                {"PATH": str(python.parent) + os.pathsep + environment.get("PATH", "")}
            ),
        )

    monkeypatch.setattr(cp, "checkout_input", fixture_input)
    # Checkout state lives under a private artifact root, never the host's.
    artifacts = tmp_path / "artifacts"
    artifacts.mkdir()
    monkeypatch.setenv("MOLT_EXT_ROOT", str(artifacts))
    return root


def _args(root: Path, **values) -> argparse.Namespace:
    base = {
        "repo": str(root / "molt-src"),
        "remote": "origin",
        "branch": "main",
        "base": None,
        "root": None,
        "abandon": False,
        "ignore": None,
        "json": False,
        "no_fetch": False,
    }
    base.update(values)
    return argparse.Namespace(**base)


def _start(root: Path, slug: str) -> Path:
    assert lanes.cmd_lane_start(_args(root, slug=slug)) == molt_dev.EXIT_OK
    worktree = root / "worktrees" / slug
    _git(worktree, "config", "user.email", "dev@molt.test")
    _git(worktree, "config", "user.name", "Molt Dev")
    return worktree


def _ref(repo: Path, ref: str) -> str | None:
    proc = _git(repo, "rev-parse", "--verify", "--quiet", ref, check=False)
    return proc.stdout.strip() if proc.returncode == 0 else None


def _registered(main: Path) -> list[Path]:
    return [
        Path(line.removeprefix("worktree ")).resolve()
        for line in _git(main, "worktree", "list", "--porcelain").stdout.splitlines()
        if line.startswith("worktree ")
    ]


def _record(root: Path, slug: str) -> dict:
    path = root / "molt-src" / "logs" / "agents" / f"lane-{slug}" / "coordination.json"
    return json.loads(path.read_text(encoding="utf-8"))


def test_lane_start_creates_branch_worktree_record_and_inputs(family: Path) -> None:
    main = family / "molt-src"
    worktree = _start(family, "feature")

    base = _ref(main, "refs/remotes/origin/main")
    assert _ref(main, "refs/heads/lane/feature") == base
    assert worktree.resolve() in _registered(main)
    assert _git(worktree, "symbolic-ref", "HEAD").stdout.strip() == (
        "refs/heads/lane/feature"
    )
    # No upstream tracking: a lane pushes nowhere by default.
    assert (
        _git(
            main, "config", "--get", "branch.lane/feature.merge", check=False
        ).returncode
        == 1
    )

    record = _record(family, "feature")
    assert record["status"] == "running"
    assert record["branch"] == "lane/feature"
    assert Path(record["repo_root"]) == worktree.resolve()
    assert record["lane"]["base_commit"] == base

    for name in ("uv-environment", "browser-acorn"):
        assert (worktree / f"{name}-tree" / "ready").read_text(
            encoding="utf-8"
        ) == "v1\n"
        assert (worktree / f"{name}-tree" / cp.STAMP_NAME).is_file()
        assert (worktree / f"{name}-tree" / "runs").read_text(
            encoding="utf-8"
        ) == "run\n"
    assert (main / ".git" / "molt-fixture-hook.txt").is_file()

    with pytest.raises(DriverError, match="already exists"):
        lanes.cmd_lane_start(_args(family, slug="feature"))


def test_lane_start_refuses_bad_slugs_and_unlaid_roots(
    family: Path, tmp_path: Path
) -> None:
    for slug in ("Upper", "a/b", "a..b", "x.lock", "-x", "con"):
        with pytest.raises(DriverError) as raised:
            lanes.cmd_lane_start(_args(family, slug=slug))
        assert raised.value.code == molt_dev.EXIT_USAGE, slug
    plain = tmp_path / "plain-clone"
    _git(tmp_path, "clone", "-q", str(tmp_path / "origin.git"), str(plain))
    with pytest.raises(DriverError, match="pass --root"):
        lanes.cmd_lane_start(_args(family, slug="x", repo=str(plain)))
    assert _ref(plain, "refs/heads/lane/x") is None


def test_lane_retire_refuses_an_unlanded_lane(family: Path) -> None:
    worktree = _start(family, "unlanded")
    tip = _commit_file(worktree, "work.txt", "work\n", "unlanded work")

    with pytest.raises(DriverError, match="not landed: 1 commit"):
        lanes.cmd_lane_retire(_args(family, slug="unlanded"))
    assert worktree.is_dir()
    assert _ref(family / "molt-src", "refs/heads/lane/unlanded") == tip
    assert _record(family, "unlanded")["status"] == "running"


def test_lane_retire_abandon_bundles_the_commits_before_removal(
    family: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    main = family / "molt-src"
    worktree = _start(family, "dropped")
    _commit_file(worktree, "one.txt", "one\n", "first abandoned commit")
    tip = _commit_file(worktree, "two.txt", "two\n", "second abandoned commit")
    capsys.readouterr()

    assert lanes.cmd_lane_retire(_args(family, slug="dropped", abandon=True)) == 0
    bundle = Path(capsys.readouterr().out.strip().splitlines()[0])
    assert bundle == family / "drift-bundles" / f"lane-dropped-{tip[:12]}.bundle"
    assert not worktree.exists()
    assert _ref(main, "refs/heads/lane/dropped") is None

    # The bundle restores both commits into the repository.
    _git(
        main, "fetch", "-q", str(bundle), "refs/heads/lane/dropped:refs/heads/restored"
    )
    assert _ref(main, "refs/heads/restored") == tip
    subjects = _git(main, "log", "--format=%s", "origin/main..restored").stdout
    assert subjects.splitlines() == [
        "second abandoned commit",
        "first abandoned commit",
    ]
    record = _record(family, "dropped")
    assert record["status"] == "abandoned"
    assert record["lane_retired"]["bundle"] == str(bundle)


def test_lane_retire_removes_a_landed_lane_completely(family: Path) -> None:
    from molt import dx

    main = family / "molt-src"
    worktree = _start(family, "landed")
    tip = _commit_file(worktree, "landed.txt", "landed\n", "landed work")
    _git(worktree, "push", "-q", "origin", "HEAD:main", "HEAD:refs/heads/lane/landed")
    artifacts = family.parent / "artifacts"  # the fixture's MOLT_EXT_ROOT
    state = dx.stable_uv_project_env_dir(
        artifacts, purpose="dx", python="3.12", source_root=worktree
    )
    state.mkdir(parents=True)
    (state / "pyvenv.cfg").write_text("home = /x\n", encoding="utf-8")

    assert lanes.cmd_lane_retire(_args(family, slug="landed")) == 0
    assert not worktree.exists()
    assert _registered(main) == [main.resolve()]
    assert _ref(main, "refs/heads/lane/landed") is None
    origin = family.parent / "origin.git"
    assert _ref(origin, "refs/heads/lane/landed") is None
    assert _ref(origin, "refs/heads/main") == tip
    assert not state.exists(), "the checkout state retires with the checkout"
    assert _record(family, "landed")["status"] == "landed"


def _replay_onto_advanced_main(family: Path, commits: list[str], amend: bool) -> None:
    """Historical replay/amend landings remain represented after main moves."""
    main = family / "molt-src"
    _git(main, "pull", "-q", "--ff-only", "origin", "main")
    _commit_file(main, "other.txt", "other\n", "main moved on")
    for commit in commits:
        _git(main, "cherry-pick", commit)
    if amend:
        # The landing amends its last commit (a ledger citation resolved).
        (main / "b.txt").write_text("b, amended at landing\n", encoding="utf-8")
        _git(main, "commit", "-q", "--amend", "--no-edit", "-a")
    _git(main, "push", "-q", "origin", "HEAD:main")


def test_lane_retire_accepts_a_replayed_and_amended_landing(family: Path) -> None:
    worktree = _start(family, "replayed")
    first = _commit_file(worktree, "a.txt", "a\n", "no trailer: matched by patch-id")
    second = _commit_file(
        worktree, "b.txt", "b\n", f"amended at landing\n\nChange-Id: {CHANGE_B}"
    )
    _replay_onto_advanced_main(family, [first, second], amend=True)
    main = family / "molt-src"
    _git(main, "fetch", "-q", "origin")
    assert (
        _git(
            main, "merge-base", "--is-ancestor", second, "origin/main", check=False
        ).returncode
        == 1
    )

    assert lanes.cmd_lane_retire(_args(family, slug="replayed")) == 0
    assert not worktree.exists()
    assert _ref(main, "refs/heads/lane/replayed") is None
    assert _record(family, "replayed")["status"] == "landed"


def test_lane_retire_refuses_an_amended_commit_without_a_change_id(
    family: Path,
) -> None:
    worktree = _start(family, "untraced")
    first = _commit_file(worktree, "a.txt", "a\n", "matched by patch-id")
    second = _commit_file(worktree, "b.txt", "b\n", "no Change-Id to follow")
    _replay_onto_advanced_main(family, [first, second], amend=True)

    with pytest.raises(DriverError, match="not landed: 1 commit") as raised:
        lanes.cmd_lane_retire(_args(family, slug="untraced"))
    assert second[:9] in str(raised.value) and first[:9] not in str(raised.value)
    assert worktree.is_dir()


def test_landing_representation_rules(family: Path) -> None:
    main = family / "molt-src"
    git = molt_dev.Git(main)
    worktree = _start(family, "rules")
    patch = _commit_file(worktree, "p.txt", "p\n", "patch twin")
    traced = _commit_file(worktree, "t.txt", "t\n", f"traced\n\nChange-Id: {CHANGE_A}")
    _git(worktree, "commit", "-q", "--allow-empty", "-m", "empty, untraced")
    tip = _ref(worktree, "HEAD")
    assert tip is not None

    before = landing_representation(git, tip, "origin/main")
    assert not before.landed and len(before.unrepresented) == 3
    _git(main, "pull", "-q", "--ff-only", "origin", "main")
    # Move main first, so the replay is a new commit, not the same object.
    _commit_file(main, "other.txt", "other\n", "main moved on")
    _git(main, "cherry-pick", patch)
    # Same Change-Id, different content: the trailer, not the patch, matches.
    _commit_file(
        main, "t.txt", "rewritten\n", f"traced, rewritten\n\nChange-Id: {CHANGE_A}"
    )
    _git(main, "push", "-q", "origin", "HEAD:main")
    git.fetch("origin", "main")
    after = landing_representation(git, tip, "origin/main")
    assert set(before.commits) == set(after.commits)
    assert after.unrepresented == (tip,), "an empty commit needs its own match"
    assert patch not in after.unrepresented and traced not in after.unrepresented
    # A tip on upstream is the trivial instance.
    trivial = landing_representation(
        git, _ref(main, "origin/main") or "", "origin/main"
    )
    assert trivial.landed and trivial.commits == ()


def test_merge_resolution_requires_ancestry_before_lane_retirement(
    family: Path,
) -> None:
    main = family / "molt-src"
    worktree = _start(family, "merge-rules")
    ordinary = _commit_file(
        worktree, "ordinary.txt", "ordinary\n", f"ordinary\n\nChange-Id: {CHANGE_A}"
    )
    side = _commit_file(main, "main.txt", "main\n", "main advanced")
    _git(worktree, "merge", "--no-ff", "--no-commit", side)
    resolution = "unique manual merge resolution\n"
    merge = _commit_file(
        worktree,
        "resolution-only.txt",
        resolution,
        f"manual resolution\n\nChange-Id: {CHANGE_B}",
    )
    _git(main, "cherry-pick", ordinary)
    _git(
        main,
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        f"same label, no resolution\n\nChange-Id: {CHANGE_B}",
    )
    _git(main, "push", "-q", "origin", "HEAD:main")
    git = molt_dev.Git(main)
    git.fetch("origin", "main")
    represented = landing_representation(git, merge, "origin/main")
    assert represented.unrepresented == (merge,)
    with pytest.raises(DriverError, match="not landed: 1 commit"):
        lanes.cmd_lane_retire(_args(family, slug="merge-rules"))
    assert (worktree / "resolution-only.txt").read_text(encoding="utf-8") == resolution
    assert _ref(main, "refs/heads/lane/merge-rules") == merge

    _git(main, "merge", "--no-ff", "--no-edit", merge)
    # Amending the final integration commit must retain the original lane
    # merge as an ancestor, just as resolving a landing citation does.
    (main / "citation.txt").write_text("resolved citation\n", encoding="utf-8")
    _git(main, "add", "--", "citation.txt")
    _git(main, "commit", "-q", "--amend", "--no-edit")
    _git(main, "push", "-q", "origin", "HEAD:main")
    assert lanes.cmd_lane_retire(_args(family, slug="merge-rules")) == molt_dev.EXIT_OK
    assert not worktree.exists()
    assert _git(main, "show", "origin/main:resolution-only.txt").stdout == resolution
    assert (
        _git(main, "merge-base", "--is-ancestor", merge, "origin/main").returncode == 0
    )


@pytest.mark.parametrize("change", ["tracked", "untracked"])
def test_lane_retire_refuses_a_dirty_worktree(family: Path, change: str) -> None:
    worktree = _start(family, "dirty")
    if change == "tracked":
        (worktree / "README.md").write_text("edited\n", encoding="utf-8")
        named = "README.md"
    else:
        (worktree / "notes.txt").write_text("unsaved\n", encoding="utf-8")
        named = "notes.txt"

    with pytest.raises(DriverError, match="is dirty") as raised:
        lanes.cmd_lane_retire(_args(family, slug="dirty"))
    assert named in str(raised.value)
    assert worktree.is_dir()
    assert (worktree / named).is_file()
    assert _ref(family / "molt-src", "refs/heads/lane/dirty") is not None


def test_lane_retire_refuses_to_remove_the_running_checkout(
    family: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    worktree = _start(family, "self")
    monkeypatch.chdir(worktree)
    with pytest.raises(DriverError, match="run lane retire from another checkout"):
        lanes.cmd_lane_retire(_args(family, slug="self"))
    assert worktree.is_dir()


def test_lane_list_reports_each_lane(
    family: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    open_lane = _start(family, "open")
    _commit_file(open_lane, "open.txt", "open\n", "open work")
    (open_lane / "scratch.txt").write_text("scratch\n", encoding="utf-8")
    done = _start(family, "done")
    _commit_file(done, "done.txt", "done\n", "done work")
    _git(done, "push", "-q", "origin", "HEAD:main")
    capsys.readouterr()

    assert lanes.cmd_lane_list(_args(family, json=True)) == 0
    payload = json.loads(capsys.readouterr().out)
    by_slug = {lane["slug"]: lane for lane in payload["lanes"]}
    assert set(by_slug) == {"open", "done"}
    assert by_slug["open"]["ahead"] == 1 and by_slug["open"]["behind"] == 1
    assert not by_slug["open"]["landed"]
    assert by_slug["open"]["dirty"] == ["?? scratch.txt"]
    assert by_slug["open"]["provisioned"] is True
    assert by_slug["done"]["landed"] and by_slug["done"]["ahead"] == 0
    assert by_slug["done"]["dirty"] == []
    assert by_slug["done"]["record_status"] == "running"
    assert payload["repository_inputs"] == {"git-hooks": "current"}


def test_queue_provisions_its_sandbox_and_retires_a_landed_lane(family: Path) -> None:
    from tools import molt_dev_queue as queue

    main = family / "molt-src"
    _commit_file(main, "tools/proof_plan.toml", 'always = ["true"]\n', "policy")
    _git(main, "push", "-q", "origin", "HEAD:main")
    worktree = _start(family, "queued")
    tip = _commit_file(worktree, "queued.txt", "queued\n", "queued work")
    paths = queue.queue_paths(main)
    request = queue.enqueue(paths, head=tip, worktree=worktree, branch="lane/queued")

    args = molt_dev.build_parser().parse_args(
        ["queue", "run", "--repo", str(main), "--candidate-controller"]
    )
    assert args.func(args) == molt_dev.EXIT_OK

    assert queue.read_result(paths, request.id)["status"] == "landed"
    # The sandbox's declared inputs were built before its gates ran.
    assert (paths.sandbox / "uv-environment-tree" / "ready").is_file()
    assert (paths.sandbox / "browser-acorn-tree" / "ready").is_file()
    _git(main, "fetch", "-q", "origin")
    assert "queued.txt" in _git(main, "ls-tree", "--name-only", "origin/main").stdout
    assert not worktree.exists()
    assert _ref(main, "refs/heads/lane/queued") is None
    assert _record(family, "queued")["status"] == "landed"


@pytest.mark.parametrize(
    "hold",
    [
        "ignored",
        "stale-input",
        "provisioning-error",
        "locked",
        "advanced",
        "record",
        "cleanup",
        "shared-branch",
        "malformed-lock",
    ],
)
def test_published_lane_retirement_holds_preserve_signal_and_fail_waiter(
    family: Path,
    monkeypatch: pytest.MonkeyPatch,
    hold: str,
) -> None:
    from tools import molt_dev_queue as queue

    main = family / "molt-src"
    worktree = _start(family, "held")
    tip = _commit_file(worktree, "work.txt", "landed\n", "landed work")
    _git(worktree, "push", "-q", "origin", "HEAD:main")
    paths = queue.queue_paths(main)
    request = queue.enqueue(paths, head=tip, worktree=worktree, branch="lane/held")
    sentinel = worktree / "work.txt"
    if hold == "ignored":
        _git(worktree, "config", "core.excludesFile", str(family / "ignore"))
        (family / "ignore").write_text("private-notes/\n", encoding="utf-8")
        sentinel = worktree / "private-notes" / "unharvested.patch"
        sentinel.parent.mkdir()
        sentinel.write_text("unique source and evidence\n", encoding="utf-8")
    elif hold == "stale-input":
        (worktree / "uv-environment-tree" / "ready").unlink()
    elif hold == "provisioning-error":

        def unreadable_state(_spec):
            raise cp.ProvisioningError("declared lock is unreadable")

        monkeypatch.setattr(cp, "input_state", unreadable_state)
    elif hold == "shared-branch":
        sibling = family / "worktrees" / "other-owner"
        _git(main, "worktree", "add", "--force", "-q", str(sibling), "lane/held")
    elif hold == "malformed-lock":
        from molt.artifact_publication import publication_locks, _publication_lock_path

        with publication_locks([sentinel]):
            pass
        lock = _publication_lock_path(worktree)
        _git(worktree, "config", "core.excludesFile", str(family / "ignore"))
        (family / "ignore").write_text(lock.name + "\n", encoding="utf-8")
        lock.write_text("unique data is not lock metadata\n", encoding="utf-8")
        sentinel = lock
    elif hold == "locked":
        _git(main, "worktree", "lock", str(worktree))
    elif hold == "advanced":
        _commit_file(worktree, "next.txt", "not enrolled\n", "later work")
    elif hold == "record":
        record_path = lanes._record_path(main, "held")
        record = json.loads(record_path.read_text(encoding="utf-8"))
        record["lane"]["worktree"] = str(family / "another-owner")
        record_path.write_text(json.dumps(record), encoding="utf-8")
    else:

        def fail_cleanup(*_args, **_kwargs):
            assert queue.read_result(paths, request.id) is None, "no premature success"
            return molt_dev.EXIT_FAIL

        monkeypatch.setattr(lanes.molt_dev_retire, "cleanup_worktree", fail_cleanup)
    before = sentinel.read_bytes()
    args = molt_dev.build_parser().parse_args(
        ["queue", "run", "--repo", str(main), "--candidate-controller"]
    )
    # Already published requests must still pass retirement, without another gate run.
    assert args.func(args) == molt_dev.EXIT_OK
    result = queue.read_result(paths, request.id)
    assert result["schema"] == "molt.landing-result.v2"
    assert result["status"] == "landed_retirement_failed"
    assert result["landed"] == tip
    assert "lane retire held" in result["detail"]
    assert queue._await_landing(paths, request.id) == molt_dev.EXIT_FAIL
    assert sentinel.read_bytes() == before
    assert worktree in _registered(main)
    assert _ref(main, "refs/heads/lane/held") is not None
    assert not paths.sandbox.exists(), "published source requires no rebuild"
    if hold == "shared-branch":
        assert (sibling / "work.txt").read_bytes() == before
        assert sibling in _registered(main)


def test_already_published_lane_retires_and_replay_uses_closed_custody(
    family: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from tools import molt_dev_queue as queue

    main = family / "molt-src"
    worktree = _start(family, "published")
    tip = _commit_file(worktree, "work.txt", "published\n", "published work")
    _git(worktree, "push", "-q", "origin", "HEAD:main")
    # Real generator publication creates persistent metadata outside LockedTrees.
    from molt.artifact_publication import publication_locks, _publication_lock_path

    with publication_locks([worktree / "work.txt"]):
        pass
    lock = _publication_lock_path(worktree)
    assert lock.is_file() and lock.read_bytes() == b""
    _git(worktree, "config", "core.excludesFile", str(family / "ignore"))
    (family / "ignore").write_text(lock.name + "\n", encoding="utf-8")
    paths = queue.queue_paths(main)
    started = []
    monkeypatch.setattr(
        queue, "_start_queue_runner", lambda _paths, repo, _args: started.append(repo)
    )
    land = molt_dev.build_parser().parse_args(
        ["land", "--repo", str(worktree), "--candidate-controller"]
    )
    assert land.func(land) == molt_dev.EXIT_OK
    assert started == [main]
    [request] = queue.pending(paths)
    args = molt_dev.build_parser().parse_args(
        ["queue", "run", "--repo", str(main), "--candidate-controller"]
    )
    assert args.func(args) == molt_dev.EXIT_OK
    assert queue._await_landing(paths, request.id) == molt_dev.EXIT_OK
    assert not worktree.exists()
    assert not paths.sandbox.exists()
    # A crash after retirement but before queue-result publication is idempotent
    # only with the exact recorded tip and path, not merely a missing branch.
    assert (
        lanes.cmd_lane_retire(
            _args(family, slug="published"),
            expected_tip=tip,
            expected_worktree=worktree,
        )
        == molt_dev.EXIT_OK
    )
    with pytest.raises(DriverError, match="reconcile its retirement receipt"):
        lanes.cmd_lane_retire(
            _args(family, slug="published"),
            expected_tip="f" * 40,
            expected_worktree=worktree,
        )


@pytest.mark.parametrize("disposition", ["landed", "abandoned"])
def test_lane_retirement_recovers_failed_final_record_write(
    family: Path,
    monkeypatch: pytest.MonkeyPatch,
    disposition: str,
) -> None:
    from tools import agent_coordination

    main = family / "molt-src"
    worktree = _start(family, "receipt")
    tip = _commit_file(worktree, "work.txt", "published\n", "published work")
    if disposition == "landed":
        _git(
            worktree,
            "push",
            "-q",
            "origin",
            "HEAD:main",
            "HEAD:refs/heads/lane/receipt",
        )
    update = agent_coordination.update_task

    def fail_completion(*args, **kwargs):
        if "lane_retired" in kwargs.get("fields", {}):
            record = _record(family, "receipt")
            assert record["status"] == "running"
            assert record["lane_retirement"]["tip"] == tip
            assert not worktree.exists()
            assert _ref(main, "refs/heads/lane/receipt") is None
            raise OSError("terminal record publication interrupted")
        return update(*args, **kwargs)

    monkeypatch.setattr(agent_coordination, "update_task", fail_completion)
    with pytest.raises(OSError, match="terminal record"):
        lanes.cmd_lane_retire(
            _args(family, slug="receipt", abandon=disposition == "abandoned")
        )
    record = _record(family, "receipt")
    assert "lane_retired" not in record
    assert record["status"] == "running"
    monkeypatch.setattr(agent_coordination, "update_task", update)
    with pytest.raises(DriverError, match="custody changed"):
        lanes.cmd_lane_retire(
            _args(family, slug="receipt"),
            expected_tip=tip,
            expected_worktree=family / "wrong-owner",
        )
    with pytest.raises(DriverError, match="reconcile its retirement receipt"):
        lanes.cmd_lane_retire(
            _args(family, slug="receipt"),
            expected_tip="f" * 40,
            expected_worktree=worktree,
        )
    assert lanes.cmd_lane_retire(_args(family, slug="receipt")) == molt_dev.EXIT_OK
    assert _record(family, "receipt")["status"] == disposition
    assert _record(family, "receipt")["lane_retired"]["tip"] == tip
