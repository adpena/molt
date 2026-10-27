"""The lane command family (tools/molt_dev_lanes.py) on real Git repositories.

Fixtures follow the checkout-family layout of `molt.custody_layout`: a bare
origin, a main checkout at ``<root>/molt-src`` and lanes under
``<root>/worktrees``. Git plumbing, the filesystem and the coordination
records are the oracles. The provisioning table is replaced by fixture inputs
whose provisioners log each run; `molt.checkout_provisioning` still runs them.
Only the identical unprovisioned starting history is shared per session;
each test owns an independent writable origin and a real Git working clone.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import py_compile
import sys

import pytest

from molt import checkout_provisioning as cp
from molt.process_environment import ProcessEnvironment
from tests.integration_driver_fixture import (
    clone_tree,
    isolated_checkout_artifacts as isolated_checkout_artifacts,
    shared_session_tree,
)
from tests.test_molt_dev import _commit_file, _git, _init_repo
from tools import controller_source
from tools import molt_dev, molt_dev_lanes as lanes
from tools.molt_dev_change_ids import landing_representation
from tools.driver_errors import EXIT_OK, EXIT_FAIL, EXIT_USAGE, DriverError  # noqa: E402

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
if name == "uv-environment":
    # uv tags every environment it creates.
    (tree / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55\\n", encoding="utf-8")
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


def _fixture_checkout_input(
    name, checkout, environ=None, *, guard_source_root=None, python=None, groups=None
):
    """The fixture's checkout inputs: provisioners that log each run. A child
    process of a test installs it as the fixture does."""
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
    # As uv does, an inherited UV_PROJECT_ENVIRONMENT names the tree.
    named = environment.get("UV_PROJECT_ENVIRONMENT")
    tree = (
        Path(named) if name == "uv-environment" and named else checkout / f"{name}-tree"
    )
    return cp.LockedTree(
        name=name,
        guard_source_root=guard_source_root,
        checkout=checkout,
        lock_files=("fixture.lock",),
        tree=tree,
        sentinel="ready",
        tool=python.stem,
        arguments=("provision.py", name),
        environment=environment.updated(
            {"PATH": str(python.parent) + os.pathsep + environment.get("PATH", "")}
        ),
    )


@pytest.fixture(scope="session")
def _family_origin_seed(
    tmp_path_factory: pytest.TempPathFactory, worker_id: str
) -> Path:
    """Build the lane family's unprovisioned history once across workers."""

    def build(root: Path) -> None:
        seed = root / "seed"
        _init_repo(seed)
        (seed / "tools").mkdir()
        for relative, text in (
            ("provision.py", _PROVISIONER),
            ("fixture.lock", "v1\n"),
            # A virtual uv project deliberately installs no editable package.
            ("pyproject.toml", "[tool.fixture]\n"),
            ("tools/install_git_hooks.py", _INSTALLER),
            (".gitignore", "*-tree/\n"),
        ):
            (seed / relative).write_text(text, encoding="utf-8")
        _git(seed, "add", "--", ".")
        _commit_file(seed, "README.md", "seed\n", "seed")
        _git(root, "clone", "--bare", "-q", str(seed), str(root / "origin.git"))

    root = shared_session_tree(tmp_path_factory, worker_id, "lane-family-origin", build)
    return root / "origin.git"


@pytest.fixture
def family(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    isolated_checkout_artifacts: Path,
    _family_origin_seed: Path,
) -> Path:
    """Private origin.git, real <root>/molt-src clone and empty worktrees."""
    for key in ("MOLT_VENV", "UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV"):
        monkeypatch.delenv(key, raising=False)
    origin = tmp_path / "origin.git"
    clone_tree(_family_origin_seed, origin)
    root = tmp_path / "family"
    main = root / "molt-src"
    root.mkdir()
    _git(root, "clone", "-q", str(origin), str(main))
    _git(main, "config", "user.email", "dev@molt.test")
    _git(main, "config", "user.name", "Molt Dev")

    monkeypatch.setattr(cp, "checkout_input", _fixture_checkout_input)
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
        "candidate_controller": False,
        "pinned_controller": None,
    }
    base.update(values)
    return argparse.Namespace(**base)


def _start(root: Path, slug: str) -> Path:
    assert lanes.cmd_lane_start(_args(root, slug=slug)) == EXIT_OK
    # The lane's slot worktree is recorded, not named by the slug (HF-491).
    worktree = Path(_record(root, slug)["repo_root"])
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


def _assert_released(main: Path, worktree: Path) -> None:
    """A retired lane's slot stays registered, detached and clean, so the next
    lane inherits its warm checkout state (HF-491)."""
    assert worktree.resolve() in _registered(main)
    assert _git(worktree, "symbolic-ref", "-q", "HEAD", check=False).returncode == 1
    assert _git(worktree, "status", "--porcelain").stdout == ""


def _record(root: Path, slug: str) -> dict:
    path = root / "molt-src" / "logs" / "agents" / f"lane-{slug}" / "coordination.json"
    return json.loads(path.read_text(encoding="utf-8"))


def test_lane_family_origin_does_not_share_mutable_git_state(
    family: Path, _family_origin_seed: Path, tmp_path: Path
) -> None:
    origin = family.parent / "origin.git"
    sibling = tmp_path / "sibling-origin.git"
    clone_tree(_family_origin_seed, sibling)
    initial = _git(_family_origin_seed, "rev-parse", "HEAD").stdout.strip()
    original_config = (_family_origin_seed / "config").read_bytes()
    (origin / "config").write_bytes(original_config + b"\n# private mutation\n")
    (origin / "hooks" / "private-hook").write_bytes(b"private hook\n")
    tip = _commit_file(family / "molt-src", "private.txt", "private\n", "private")
    _git(family / "molt-src", "push", "-q", "origin", "HEAD:main")
    assert _ref(origin, "refs/heads/main") == tip
    for untouched in (_family_origin_seed, sibling):
        assert (untouched / "config").read_bytes() == original_config
        assert not (untouched / "hooks" / "private-hook").exists()
        assert _ref(untouched, "refs/heads/main") == initial
        assert _git(untouched, "show", "HEAD:README.md").stdout == "seed\n"
    # Atomic Git ref replacement alone would miss hardlinked object aliases.
    relative = Path("objects") / initial[:2] / initial[2:]
    original_object = (_family_origin_seed / relative).read_bytes()
    (origin / relative).chmod(0o600)
    (origin / relative).write_bytes(b"deliberately corrupt private object")
    assert (_family_origin_seed / relative).read_bytes() == original_object
    assert (sibling / relative).read_bytes() == original_object


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
    assert not (worktree / "edgebox-typescript-tree").exists()
    assert lanes._checkout_input_states(worktree) == {
        "uv-environment": "current",
        "browser-acorn": "current",
    }
    # An explicitly selected optional tree remains recognized by preservation
    # custody even though it is not a lane setup/status prerequisite.
    cp.ensure("edgebox-typescript", worktree)
    assert (worktree / "edgebox-typescript-tree/ready").is_file()
    assert "edgebox-typescript-tree/" not in lanes._unharvested_ignored_paths(
        molt_dev.Git(worktree)
    )

    with pytest.raises(DriverError, match="already exists"):
        lanes.cmd_lane_start(_args(family, slug="feature"))


def test_a_lane_start_that_cannot_record_leaves_no_worktree_or_branch(
    family: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A lane without its record is one `lane retire` refuses (review v2, B-1).

    The command removes the worktree and branch it created, and the slug is
    free for the next start.
    """
    from tools import agent_coordination

    main = family / "molt-src"
    create_task = agent_coordination.create_task

    def refuse(**kwargs):
        raise RuntimeError("tool releases unreadable")

    monkeypatch.setattr(agent_coordination, "create_task", refuse)
    with pytest.raises(
        DriverError,
        match=r"cannot start lane/orphan: tool releases unreadable; removed "
        r".*slot-01 and lane/orphan$",
    ):
        lanes.cmd_lane_start(_args(family, slug="orphan"))
    assert not (family / "worktrees" / "slot-01").exists()
    assert _ref(main, "refs/heads/lane/orphan") is None
    assert not any(path.name == "slot-01" for path in _registered(main))

    monkeypatch.setattr(agent_coordination, "create_task", create_task)
    worktree = _start(family, "orphan")
    record = json.loads(
        (main / "logs/agents/lane-orphan/coordination.json").read_text("utf-8")
    )
    assert record["lane"]["controller"] == "candidate"
    assert worktree.is_dir()


def test_an_undo_that_git_refuses_keeps_the_branch_and_says_so(
    family: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A worktree Git will not remove keeps its branch (review v3, A-1).

    Deleting the branch under a kept worktree left a worktree whose HEAD named
    no branch and a slug no command could free.
    """
    from tools import agent_coordination

    main = family / "molt-src"

    def write_then_refuse(**kwargs):
        (kwargs["source_root"] / "notes.txt").write_text("mine\n", encoding="utf-8")
        raise RuntimeError("record refused")

    monkeypatch.setattr(agent_coordination, "create_task", write_then_refuse)
    with pytest.raises(DriverError) as raised:
        lanes.cmd_lane_start(_args(family, slug="dirty"))
    message = str(raised.value)
    worktree = family / "worktrees" / "slot-01"
    assert "kept" in message and "git worktree remove refused" in message
    assert f"git worktree remove --force {worktree}" in message
    assert "git branch -D lane/dirty" in message
    assert (worktree / "notes.txt").read_text(encoding="utf-8") == "mine\n"
    assert _ref(main, "refs/heads/lane/dirty") is not None
    assert _git(worktree, "symbolic-ref", "HEAD").stdout.strip() == (
        "refs/heads/lane/dirty"
    )


def test_a_worktree_add_that_fails_after_branching_is_undone(
    family: Path,
) -> None:
    """`git worktree add` keeps the worktree and branch when its post-checkout
    hook fails; the start removes both (review v3, A-2)."""
    main = family / "molt-src"
    hooks = family / "hooks"
    hooks.mkdir()
    hook = hooks / "post-checkout"
    hook.write_text("#!/bin/sh\nexit 3\n", encoding="utf-8")
    hook.chmod(0o755)
    _git(main, "config", "core.hooksPath", str(hooks))
    with pytest.raises(
        DriverError,
        match=r"cannot start lane/hooked: git worktree add failed .*; removed "
        r".*slot-01 and lane/hooked$",
    ):
        lanes.cmd_lane_start(_args(family, slug="hooked"))
    assert not (family / "worktrees" / "slot-01").exists()
    assert _ref(main, "refs/heads/lane/hooked") is None
    _git(main, "config", "--unset", "core.hooksPath")
    assert _start(family, "hooked").is_dir()


def test_a_second_start_of_a_slug_refuses_while_the_first_runs(
    family: Path,
) -> None:
    """Two starts of one slug once raced: the loser's undo removed the
    winner's worktree and branch (review v4). One start per slug runs at a
    time; the second refuses and creates nothing."""
    from molt.file_locks import _release_file_lock, _try_acquire_file_lock

    main = family / "molt-src"
    common = Path(_git(main, "rev-parse", "--git-common-dir").stdout.strip())
    if not common.is_absolute():
        common = main / common
    lock = common / "molt" / "lane-start" / "twin.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    handle = _try_acquire_file_lock(lock)
    assert handle is not None
    try:
        with pytest.raises(
            DriverError,
            match="another `lane start twin` or `lane retire twin` is running",
        ):
            lanes.cmd_lane_start(_args(family, slug="twin"))
        assert _ref(main, "refs/heads/lane/twin") is None
        assert not (family / "worktrees" / "slot-01").exists()
    finally:
        _release_file_lock(handle)
    assert _start(family, "twin").is_dir()


# A lane command in its own process, so its slug lock is the OS lock: in one
# process a second holder stops at the in-process mutex (review v5, A-1). It
# installs the fixture's checkout inputs and skips provisioning. With
# ``hold``, it writes ``ready`` when it reaches that module function and waits
# for ``go`` before it calls it. A release_slot hold instead pauses immediately
# after the real detach, before retirement deletes refs and closes its record.
_LANE_CHILD = """
import sys, time
from pathlib import Path
sys.path.insert(0, {root!r})
from molt import checkout_provisioning as cp
from tests.test_molt_dev_lanes import _args, _fixture_checkout_input
from tools import molt_dev_lanes as lanes
from tools.driver_errors import DriverError
cp.checkout_input = _fixture_checkout_input
lanes.provision_lane = lambda worktree: []
if {hold!r}:
    owner = lanes.molt_dev_retire if {hold!r} == "release_slot" else lanes
    real = getattr(owner, {hold!r})
    def held(*args, **kwargs):
        after_release = {hold!r} == "release_slot"
        result = real(*args, **kwargs) if after_release else None
        Path({ready!r}).write_text("ready")
        deadline = time.monotonic() + 60
        while not Path({go!r}).exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        return result if after_release else real(*args, **kwargs)
    setattr(owner, {hold!r}, held)
command = lanes.cmd_lane_start if {command!r} == "start" else lanes.cmd_lane_retire
try:
    print("RESULT ok", command(_args(Path({family!r}), slug={slug!r})), flush=True)
except DriverError as exc:
    print("RESULT refused", str(exc).replace(chr(10), " "), flush=True)
"""


def _lane_child(
    family: Path,
    signals: tuple[Path, Path],
    *,
    command: str,
    slug: str,
    hold: str | None = None,
):
    import subprocess

    from tests.process_guard_common import start_owned_test_process

    ready, go = signals
    root = str(Path(__file__).resolve().parents[1])
    code = _LANE_CHILD.format(
        root=root, hold=hold, ready=str(ready), go=str(go),
        family=str(family), slug=slug, command=command,
    )  # fmt: skip
    return start_owned_test_process(
        [sys.executable, "-B", "-c", code],
        cwd=root,
        env=dict(os.environ),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def _await_hold(process, ready: Path, what: str) -> None:
    import time

    deadline = time.monotonic() + 60
    while not ready.exists():
        assert process.poll() is None, process.communicate()
        assert time.monotonic() < deadline, f"the child never reached {what}"
        time.sleep(0.02)


def test_two_processes_starting_one_slug_never_undo_each_other(
    family: Path, tmp_path: Path
) -> None:
    """The review v4 race with two real processes and the OS lock: the first
    holds its checks open, the second refuses at once, and the first's
    worktree, branch and record survive (review v5, A-1)."""
    from tests.process_guard_common import close_owned_test_process

    main = family / "molt-src"
    signals = (tmp_path / "ready", tmp_path / "go")
    first = _lane_child(
        family, signals, command="start", slug="twin", hold="work_in_progress_refusal"
    )
    try:
        _await_hold(first, signals[0], "its checks")
        second = _lane_child(family, signals, command="start", slug="twin")
        try:
            second_out, second_err = second.communicate(timeout=60)
        finally:
            close_owned_test_process(second)
        signals[1].write_text("go", encoding="utf-8")
        first_out, first_err = first.communicate(timeout=60)
    finally:
        signals[1].write_text("go", encoding="utf-8")
        close_owned_test_process(first)
    assert "another `lane start twin`" in second_out, second_out + second_err
    assert "RESULT ok 0" in first_out, first_out + first_err
    worktree = family / "worktrees" / "slot-01"
    assert any(path.resolve() == worktree.resolve() for path in _registered(main))
    assert _ref(main, "refs/heads/lane/twin") == _ref(main, "refs/remotes/origin/main")


def test_a_retirement_refuses_while_another_process_provisions_the_lane(
    family: Path, tmp_path: Path
) -> None:
    """A start holds the slug's lock through provisioning, so a retirement
    refuses at once instead of removing the lane under its provisioners. The
    start runs in its own process and waits inside provisioning; with the lock
    released before provisioning (review v5's A-5 code), the retirement
    removed the lane (review v6, A6-1)."""
    from tests.process_guard_common import close_owned_test_process

    main = family / "molt-src"
    signals = (tmp_path / "ready", tmp_path / "go")
    start = _lane_child(
        family, signals, command="start", slug="busy", hold="provision_lane"
    )
    try:
        _await_hold(start, signals[0], "provisioning")
        with pytest.raises(
            DriverError,
            match="another `lane start busy` or `lane retire busy` is running",
        ):
            lanes.cmd_lane_retire(_args(family, slug="busy"))
        signals[1].write_text("go", encoding="utf-8")
        out, err = start.communicate(timeout=60)
    finally:
        signals[1].write_text("go", encoding="utf-8")
        close_owned_test_process(start)
    assert "RESULT ok 0" in out, out + err
    worktree = family / "worktrees" / "slot-01"
    assert worktree.resolve() in _registered(main)
    assert _ref(main, "refs/heads/lane/busy") == _ref(main, "refs/remotes/origin/main")
    assert _record(family, "busy")["status"] == "running"


def test_two_processes_retiring_one_slug_never_meet(
    family: Path, tmp_path: Path
) -> None:
    """`lane retire` holds the slug's lock for its whole run: while one
    retirement waits inside its run in another process, a second refuses at
    once, and the first then retires the lane. With the lock released as soon
    as it was taken, both removed the one lane (review v6, A6-1)."""
    from tests.process_guard_common import close_owned_test_process

    main = family / "molt-src"
    worktree = _start(family, "dup")
    signals = (tmp_path / "ready", tmp_path / "go")
    first = _lane_child(
        family, signals, command="retire", slug="dup", hold="_retire_lane"
    )
    try:
        _await_hold(first, signals[0], "its retirement")
        with pytest.raises(
            DriverError,
            match="another `lane start dup` or `lane retire dup` is running",
        ):
            lanes.cmd_lane_retire(_args(family, slug="dup"))
        assert worktree.is_dir()
        signals[1].write_text("go", encoding="utf-8")
        out, err = first.communicate(timeout=120)
    finally:
        signals[1].write_text("go", encoding="utf-8")
        close_owned_test_process(first)
    assert "RESULT ok 0" in out, out + err
    _assert_released(main, worktree)
    assert _ref(main, "refs/heads/lane/dup") is None
    assert _record(family, "dup")["status"] == "landed"


def test_a_retirement_waits_for_no_start_and_a_start_for_no_retirement(
    family: Path,
) -> None:
    """`lane retire` holds the slug's lock too: it refuses while a start of
    the slug runs, including its provisioning (review v5, A-5)."""
    from molt.file_locks import _release_file_lock, _try_acquire_file_lock

    main = family / "molt-src"
    _start(family, "held")
    common = Path(_git(main, "rev-parse", "--git-common-dir").stdout.strip())
    if not common.is_absolute():
        common = main / common
    handle = _try_acquire_file_lock(common / "molt" / "lane-start" / "held.lock")
    assert handle is not None
    try:
        with pytest.raises(DriverError, match="`lane retire held` is running"):
            lanes.cmd_lane_retire(_args(family, slug="held"))
        assert (family / "worktrees" / "slot-01").is_dir()
    finally:
        _release_file_lock(handle)


def test_a_branch_without_a_record_names_its_hand_removal(family: Path) -> None:
    """A branch a stopped start or a hand left has no record, so `lane retire`
    cannot take it; `lane start` and `lane retire` name the hand commands and
    the worktree that holds the branch (review v5, A-4; review v6, A6-2)."""
    main = family / "molt-src"
    holder = family / "worktrees" / "orphaned"
    _git(
        main, "worktree", "add", "-q", "-b", "lane/orphaned", str(holder), "origin/main"
    )
    for command in (lanes.cmd_lane_start, lanes.cmd_lane_retire):
        with pytest.raises(DriverError) as raised:
            command(_args(family, slug="orphaned"))
        message = str(raised.value)
        assert "no lane record" in message, message
        assert "and `git branch -D lane/orphaned`" in message, message
        assert any(
            f"`git worktree remove {path}`" in message
            for path in (holder, holder.resolve())
        ), message
    assert holder.resolve() in _registered(main)


def test_the_hand_removal_hint_reaches_only_a_branch_retire_cannot_take(
    family: Path,
) -> None:
    """The hint lived in the shared record reader, so `land` of a hand-made
    lane branch with unlanded work advised deleting that work, and `lane
    retire` of a slug with no branch named a branch that does not exist; a
    branch with a closed record was called one with no record (review v6,
    A6-2)."""
    main = family / "molt-src"
    with pytest.raises(DriverError) as raised:
        lanes.cmd_lane_retire(_args(family, slug="nosuch"))
    assert "git branch -D" not in str(raised.value), raised.value

    hand = family / "worktrees" / "hand"
    _git(main, "worktree", "add", "-q", "-b", "lane/hand", str(hand), "origin/main")
    _git(hand, "config", "user.email", "dev@molt.test")
    _git(hand, "config", "user.name", "Molt Dev")
    tip = _commit_file(hand, "hand.txt", "hand\n", "hand work")
    land = molt_dev.build_parser().parse_args(
        ["land", "--repo", str(hand), "--candidate-controller"]
    )
    with pytest.raises(DriverError, match="managed lane custody unavailable") as raised:
        land.func(land)
    assert "git branch -D" not in str(raised.value), raised.value
    assert _ref(main, "refs/heads/lane/hand") == tip

    _start(family, "closed")
    assert lanes.cmd_lane_retire(_args(family, slug="closed")) == EXIT_OK
    _git(main, "branch", "lane/closed", "origin/main")
    with pytest.raises(DriverError) as raised:
        lanes.cmd_lane_start(_args(family, slug="closed"))
    message = str(raised.value)
    assert "a closed lane record" in message and "landed" in message, message
    assert "no lane record" not in message, message
    assert "`git branch -D lane/closed`" in message, message


def test_an_undo_names_how_to_finish_what_it_keeps(
    family: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A branch that moved before the record failed is kept with the commands
    that finish by hand; a path another process made is kept while the
    branch this start made goes (review v4)."""
    from tools import agent_coordination

    main = family / "molt-src"
    create_task = agent_coordination.create_task

    def commit_then_refuse(**kwargs):
        worktree = kwargs["source_root"]
        _git(worktree, "config", "user.email", "dev@molt.test")
        _git(worktree, "config", "user.name", "Molt Dev")
        _commit_file(worktree, "work.txt", "kept\n", "work")
        raise RuntimeError("record refused")

    monkeypatch.setattr(agent_coordination, "create_task", commit_then_refuse)
    with pytest.raises(DriverError) as raised:
        lanes.cmd_lane_start(_args(family, slug="moved"))
    message = str(raised.value)
    assert "removed " in message and "kept lane/moved" in message
    assert "git log " in message and "git branch -D lane/moved" in message
    assert not (family / "worktrees" / "slot-01").exists()
    assert _ref(main, "refs/heads/lane/moved") is not None

    # Another process makes the first slot's path during the checks: the start
    # leaves it untouched and takes the next slot (HF-491).
    stranger = family / "worktrees" / "slot-01"

    def make_the_path(*args, **kwargs):
        stranger.mkdir(parents=True)
        (stranger / "theirs.txt").write_text("theirs\n", encoding="utf-8")
        return None

    monkeypatch.setattr(lanes, "work_in_progress_refusal", make_the_path)
    monkeypatch.setattr(agent_coordination, "create_task", create_task)
    raced = _start(family, "raced")
    assert raced == family / "worktrees" / "slot-02"
    assert (stranger / "theirs.txt").read_text(encoding="utf-8") == "theirs\n"
    assert stranger.resolve() not in _registered(main)


def test_lane_commands_refuse_a_pinned_claim_for_another_controller(
    family: Path,
) -> None:
    """A forged `--pinned-controller` runs nothing (review v3, A-3)."""
    main = family / "molt-src"
    claim = "0" * 32
    with pytest.raises(DriverError, match="has no record"):
        lanes.cmd_lane_start(_args(family, slug="forged", pinned_controller=claim))
    assert _ref(main, "refs/heads/lane/forged") is None
    assert not (family / "worktrees" / "slot-01").exists()
    _start(family, "kept")
    with pytest.raises(DriverError, match="has no record"):
        lanes.cmd_lane_retire(_args(family, slug="kept", pinned_controller=claim))
    assert (family / "worktrees" / "slot-01").is_dir()


def test_lane_start_refuses_bad_slugs_and_unlaid_roots(
    family: Path, tmp_path: Path
) -> None:
    for slug in ("Upper", "a/b", "a..b", "x.lock", "-x", "con"):
        with pytest.raises(DriverError) as raised:
            lanes.cmd_lane_start(_args(family, slug=slug))
        assert raised.value.code == EXIT_USAGE, slug
    plain = tmp_path / "plain-clone"
    _git(tmp_path, "clone", "-q", str(tmp_path / "origin.git"), str(plain))
    with pytest.raises(DriverError, match="pass --root"):
        lanes.cmd_lane_start(_args(family, slug="x", repo=str(plain)))
    assert _ref(plain, "refs/heads/lane/x") is None


def test_lane_start_refuses_past_the_work_in_progress_limit(
    family: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tools import drift_harvest

    main = family / "molt-src"
    _start(family, "first")
    live = len(_registered(main))
    # At the limit a new lane would pass it: refused, nothing created.
    monkeypatch.setattr(drift_harvest, "MAX_LIVE_WORKTREES", live)
    with pytest.raises(
        DriverError, match=rf"{live} live worktrees: .* limit of {live}"
    ):
        lanes.cmd_lane_start(_args(family, slug="second"))
    assert _ref(main, "refs/heads/lane/second") is None
    # One under it, a lane starts.
    monkeypatch.setattr(drift_harvest, "MAX_LIVE_WORKTREES", live + 1)
    _start(family, "second")


def test_lanes_take_distinct_slots_and_reuse_a_released_one(family: Path) -> None:
    """Concurrent lanes take distinct slots; a retired lane's slot is the next
    lane's, at the same path, so its path-keyed state stays warm (HF-491)."""
    main = family / "molt-src"
    first = _start(family, "first")
    second = _start(family, "second")
    assert (first.name, second.name) == ("slot-01", "slot-02")
    runs = first / "uv-environment-tree" / "runs"
    assert runs.read_text(encoding="utf-8") == "run\n"

    assert lanes.cmd_lane_retire(_args(family, slug="first")) == EXIT_OK
    _assert_released(main, first)
    third = _start(family, "third")
    assert third == first
    # The reused slot's checkout input is current: no second provisioning run.
    assert runs.read_text(encoding="utf-8") == "run\n"
    assert lanes._checkout_input_states(third)["uv-environment"] == "current"
    assert _git(third, "symbolic-ref", "HEAD").stdout.strip() == "refs/heads/lane/third"
    assert _ref(main, "refs/heads/lane/third") == _ref(main, "refs/remotes/origin/main")
    assert _record(family, "third")["lane"]["slot"] is True


@pytest.mark.parametrize("alias", [False, True], ids=["path", "symlink"])
def test_lane_start_prefers_the_callers_released_slot(
    family: Path, tmp_path: Path, alias: bool
) -> None:
    """Two free slots distinguish caller locality from first-free allocation."""
    main = family / "molt-src"
    first = _start(family, "first")
    preferred = _start(family, "preferred")
    for slug in ("first", "preferred"):
        assert lanes.cmd_lane_retire(_args(family, slug=slug)) == EXIT_OK
    caller = preferred
    if alias:
        caller = tmp_path / "caller-alias"
        try:
            caller.symlink_to(preferred, target_is_directory=True)
        except OSError as exc:
            pytest.skip(f"host cannot create directory symlinks: {exc}")
    before = _registered(main)
    assert (
        lanes.cmd_lane_start(_args(family, slug="local", repo=str(caller))) == EXIT_OK
    )
    assert Path(_record(family, "local")["repo_root"]) == preferred
    assert _registered(main) == before
    assert (
        _git(preferred, "symbolic-ref", "HEAD").stdout.strip()
        == "refs/heads/lane/local"
    )
    assert _ref(main, "refs/heads/lane/local") == _ref(main, "refs/remotes/origin/main")
    _assert_released(main, first)
    for slot in (first, preferred):
        for name in ("uv-environment", "browser-acorn"):
            assert (slot / f"{name}-tree" / "runs").read_text(
                encoding="utf-8"
            ) == "run\n"


def test_lane_start_does_not_prefer_a_caller_outside_the_selected_slot_root(
    family: Path,
) -> None:
    main = family / "molt-src"
    caller = _start(family, "caller")
    assert lanes.cmd_lane_retire(_args(family, slug="caller")) == EXIT_OK
    selected_root = family / "other-slots"
    args = _args(family, slug="elsewhere", repo=str(caller))
    args.root = str(selected_root)
    assert lanes.cmd_lane_start(args) == EXIT_OK
    assert Path(_record(family, "elsewhere")["repo_root"]) == selected_root / "slot-01"
    _assert_released(main, caller)
    assert (caller / "uv-environment-tree" / "runs").read_text(
        encoding="utf-8"
    ) == "run\n"


@pytest.mark.parametrize("state", ["active", "dirty"])
def test_lane_start_leaves_an_unavailable_caller_untouched(
    family: Path, state: str
) -> None:
    main = family / "molt-src"
    first = _start(family, "first")
    caller = _start(family, "caller")
    assert lanes.cmd_lane_retire(_args(family, slug="first")) == EXIT_OK
    if state == "dirty":
        assert lanes.cmd_lane_retire(_args(family, slug="caller")) == EXIT_OK
        (caller / "README.md").write_bytes(b"owner's tracked change\n")
        (caller / "notes.txt").write_bytes(b"owner's untracked change\n")
    before_record = _record(family, "caller")
    before_head = _git(caller, "rev-parse", "HEAD").stdout
    before_branch = _git(caller, "symbolic-ref", "-q", "HEAD", check=False).stdout
    before_readme = (caller / "README.md").read_bytes()
    assert (
        lanes.cmd_lane_start(_args(family, slug="fallback", repo=str(caller)))
        == EXIT_OK
    )
    assert Path(_record(family, "fallback")["repo_root"]) == first
    assert _git(caller, "rev-parse", "HEAD").stdout == before_head
    assert (
        _git(caller, "symbolic-ref", "-q", "HEAD", check=False).stdout == before_branch
    )
    assert _record(family, "caller") == before_record
    assert (caller / "README.md").read_bytes() == before_readme
    if state == "dirty":
        assert (caller / "notes.txt").read_bytes() == b"owner's untracked change\n"
    assert len(_registered(main)) == 3


@pytest.mark.parametrize(
    ("protection", "prefer"),
    [("locked", False), ("locked", True), ("prunable", False)],
)
def test_lane_start_reserves_protected_slot_numbers(
    family: Path, monkeypatch: pytest.MonkeyPatch, protection: str, prefer: bool
) -> None:
    """Both preferred and ordinary choice use the same protected-row boundary."""
    main = family / "molt-src"
    held = _start(family, "held")
    assert lanes.cmd_lane_retire(_args(family, slug="held")) == EXIT_OK
    if protection == "locked":
        _git(main, "worktree", "lock", "--reason", "owner custody", str(held))
        saved = held
    else:
        # Preserve the directory while making its real Git registration stale.
        saved = family / "held-checkout"
        held.rename(saved)
    row = next(
        row for row in lanes._worktrees(molt_dev.Git(main)) if Path(row["path"]) == held
    )
    assert row[protection] is True
    before_readme = (saved / "README.md").read_bytes()
    before_record = _record(family, "held")
    caller = held if prefer else main
    monkeypatch.setattr(lanes, "LANE_SLOT_LIMIT", 1)
    with pytest.raises(DriverError, match=f"{protection} registration"):
        lanes.cmd_lane_start(_args(family, slug="blocked", repo=str(caller)))
    assert _ref(main, "refs/heads/lane/blocked") is None
    monkeypatch.setattr(lanes, "LANE_SLOT_LIMIT", 2)
    assert (
        lanes.cmd_lane_start(_args(family, slug="fallback", repo=str(caller)))
        == EXIT_OK
    )
    assert (
        Path(_record(family, "fallback")["repo_root"])
        == family / "worktrees" / "slot-02"
    )
    assert (saved / "README.md").read_bytes() == before_readme
    assert _record(family, "held") == before_record
    row = next(
        row for row in lanes._worktrees(molt_dev.Git(main)) if Path(row["path"]) == held
    )
    assert row[protection] is True
    if protection == "locked":
        _assert_released(main, held)
    else:
        assert not held.exists()


def test_lane_start_reuses_a_slot_while_its_old_retirement_finishes(
    family: Path, tmp_path: Path
) -> None:
    """Detach admits a new owner before the old process deletes refs/record."""
    from tests.process_guard_common import close_owned_test_process

    main = family / "molt-src"
    first = _start(family, "first")
    released = _start(family, "retiring")
    assert lanes.cmd_lane_retire(_args(family, slug="first")) == EXIT_OK
    signals = (tmp_path / "ready", tmp_path / "go")
    retiring = _lane_child(
        family, signals, command="retire", slug="retiring", hold="release_slot"
    )
    try:
        _await_hold(retiring, signals[0], "its completed detach")
        _assert_released(main, released)
        assert _ref(main, "refs/heads/lane/retiring") is not None
        assert _record(family, "retiring")["status"] == "running"
        assert (
            lanes.cmd_lane_start(_args(family, slug="next", repo=str(released)))
            == EXIT_OK
        )
        assert Path(_record(family, "next")["repo_root"]) == released
        (released / "new-owner.txt").write_bytes(b"new lane custody\n")
        signals[1].write_text("go", encoding="utf-8")
        out, err = retiring.communicate(timeout=120)
    finally:
        signals[1].write_text("go", encoding="utf-8")
        close_owned_test_process(retiring)
    assert "RESULT ok 0" in out, out + err
    assert _ref(main, "refs/heads/lane/retiring") is None
    assert _record(family, "retiring")["status"] == "landed"
    assert _record(family, "next")["status"] == "running"
    assert (
        _git(released, "symbolic-ref", "HEAD").stdout.strip() == "refs/heads/lane/next"
    )
    assert (released / "new-owner.txt").read_bytes() == b"new lane custody\n"
    _assert_released(main, first)


def test_lane_start_from_released_slot_uses_family_work_in_progress_count(
    family: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tools import drift_harvest

    main = family / "molt-src"
    slot = _start(family, "first")
    assert lanes.cmd_lane_retire(_args(family, slug="first")) == EXIT_OK
    before = _registered(main)
    # Two registered checkouts, but only main is active WIP. Activating the
    # released slot consumes the second WIP position without another checkout.
    assert len(before) == 2
    monkeypatch.setattr(drift_harvest, "MAX_LIVE_WORKTREES", 2)
    assert lanes.cmd_lane_start(_args(family, slug="reused", repo=str(slot))) == EXIT_OK
    assert Path(_record(family, "reused")["repo_root"]) == slot
    assert _registered(main) == before
    assert _git(slot, "symbolic-ref", "HEAD").stdout.strip() == "refs/heads/lane/reused"
    # The same entrypoint must still refuse a third active lane at the cap.
    with pytest.raises(DriverError, match="2 live worktrees: .* limit of 2"):
        lanes.cmd_lane_start(_args(family, slug="excess", repo=str(slot)))
    assert _ref(main, "refs/heads/lane/excess") is None
    assert _registered(main) == before


def test_a_free_slot_with_changes_is_skipped_untouched(family: Path) -> None:
    """A detached slot with changes is residue someone must reconcile: a start
    neither resets nor takes it."""
    main = family / "molt-src"
    first = _start(family, "first")
    assert lanes.cmd_lane_retire(_args(family, slug="first")) == EXIT_OK
    (first / "notes.txt").write_text("someone's\n", encoding="utf-8")

    second = _start(family, "second")
    assert second.name == "slot-02"
    assert (first / "notes.txt").read_text(encoding="utf-8") == "someone's\n"
    assert first.resolve() in _registered(main)


def test_the_slot_table_refuses_past_its_limit(
    family: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    main = family / "molt-src"
    monkeypatch.setattr(lanes, "LANE_SLOT_LIMIT", 1)
    _start(family, "only")
    with pytest.raises(DriverError, match="all 1 lane slots .* hold lanes"):
        lanes.cmd_lane_start(_args(family, slug="extra"))
    assert _ref(main, "refs/heads/lane/extra") is None


def test_a_failed_start_in_a_reused_slot_detaches_it_again(
    family: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The undo of a start in a reused slot keeps the slot and its state: it
    detaches the slot at the base and deletes only the branch it made."""
    from tools import agent_coordination

    main = family / "molt-src"
    slot = _start(family, "first")
    assert lanes.cmd_lane_retire(_args(family, slug="first")) == EXIT_OK
    warm = slot / "uv-environment-tree" / "ready"
    assert warm.is_file()

    def refuse(**kwargs):
        raise RuntimeError("record refused")

    monkeypatch.setattr(agent_coordination, "create_task", refuse)
    with pytest.raises(DriverError, match="cannot start lane/second: record refused"):
        lanes.cmd_lane_start(_args(family, slug="second"))
    _assert_released(main, slot)
    assert warm.is_file()
    assert _ref(main, "refs/heads/lane/second") is None


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
    _assert_released(family / "molt-src", worktree)
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


def test_lane_retire_releases_a_landed_lane_and_keeps_its_slot_warm(
    family: Path,
    isolated_checkout_artifacts: Path,
) -> None:
    from molt import dx

    main = family / "molt-src"
    worktree = _start(family, "landed")
    tip = _commit_file(worktree, "landed.txt", "landed\n", "landed work")
    _git(worktree, "push", "-q", "origin", "HEAD:main", "HEAD:refs/heads/lane/landed")
    artifacts = isolated_checkout_artifacts
    state = (
        artifacts
        / "build-environments"
        / "development"
        / dx.checkout_component(worktree)
        / ("a" * 64)
    )
    state.mkdir(parents=True)
    (state / "pyvenv.cfg").write_text("home = /x\n", encoding="utf-8")

    assert lanes.cmd_lane_retire(_args(family, slug="landed")) == 0
    _assert_released(main, worktree)
    assert _registered(main) == [main.resolve(), worktree.resolve()]
    assert _git(worktree, "rev-parse", "HEAD").stdout.strip() == tip
    assert _ref(main, "refs/heads/lane/landed") is None
    origin = family.parent / "origin.git"
    assert _ref(origin, "refs/heads/lane/landed") is None
    assert _ref(origin, "refs/heads/main") == tip
    assert _record(family, "landed")["status"] == "landed"
    # The slot keeps its checkout state, and the next lane inherits it warm
    # (HF-491): the same path keys the same Cargo target and environment.
    assert (state / "pyvenv.cfg").read_text(encoding="utf-8") == "home = /x\n"
    assert _start(family, "next") == worktree
    assert (state / "pyvenv.cfg").read_text(encoding="utf-8") == "home = /x\n"
    assert _record(family, "next")["lane"]["slot"] is True


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
    _assert_released(family / "molt-src", worktree)
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
    assert lanes.cmd_lane_retire(_args(family, slug="merge-rules")) == EXIT_OK
    _assert_released(family / "molt-src", worktree)
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


@pytest.mark.parametrize("selected_input", [False, True])
def test_queue_provisions_selected_sandbox_inputs_and_retires_a_landed_lane(
    family: Path, selected_input: bool
) -> None:
    from tools import molt_dev_queue as queue

    main = family / "molt-src"
    policy = 'always = ["true"]\n'
    if selected_input:
        # Real non-Python tool admission reads the exact tree that the fixture
        # provisioner produced. The command's own toolchain closure is the only
        # grant; default uv/Acorn enumeration would fail the empty case below.
        tool = "cmd.exe" if os.name == "nt" else "cat"
        args = (
            ["/d", "/c", "type", "browser-acorn-tree\\ready"]
            if os.name == "nt"
            else ["browser-acorn-tree/ready"]
        )
        policy += (
            'always_toolchains = ["fixture-tree"]\n'
            '[[toolchain_policy]]\nname = "fixture-tree"\n'
            + "executable = "
            + json.dumps(tool)
            + "\n"
            + "version_args = "
            + json.dumps(args)
            + "\n"
            + "version_pattern = '^v1$'\ncheckout_input = 'browser-acorn'\n"
        )
    _commit_file(main, "tools/proof_plan.toml", policy, "policy")
    _git(main, "push", "-q", "origin", "HEAD:main")
    worktree = _start(family, "queued")
    tip = _commit_file(worktree, "queued.txt", "queued\n", "queued work")
    paths = queue.queue_paths(main)
    request = queue.enqueue(paths, head=tip, worktree=worktree, branch="lane/queued")

    args = molt_dev.build_parser().parse_args(
        ["queue", "run", "--repo", str(main), "--candidate-controller"]
    )
    assert args.func(args) == EXIT_OK

    landed = queue.read_result(paths, request.id)
    assert landed is not None and landed["status"] == "landed"
    # Lane start still provisioned its editing environment. The sandbox owns
    # only selected inputs; its real admission probe proved their contents.
    assert not (paths.sandbox / "uv-environment-tree").exists()
    assert (paths.sandbox / "browser-acorn-tree" / "ready").is_file() is selected_input
    if selected_input:
        assert (paths.sandbox / "browser-acorn-tree" / "ready").read_text(
            encoding="utf-8"
        ) == "v1\n"
        assert (paths.sandbox / "browser-acorn-tree" / "runs").read_text(
            encoding="utf-8"
        ) == "run\n"
    assert not (paths.sandbox / "edgebox-typescript-tree").exists()
    _git(main, "fetch", "-q", "origin")
    assert "queued.txt" in _git(main, "ls-tree", "--name-only", "origin/main").stdout
    _assert_released(family / "molt-src", worktree)
    assert _ref(main, "refs/heads/lane/queued") is None
    assert _record(family, "queued")["status"] == "landed"


def test_the_landing_runner_waits_out_a_brief_hold_of_the_slug_lock(
    family: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The runner retires a landed lane in its own process. When another
    process held the slug's lock at that moment (here a start's checks of the
    slug), the runner recorded landed_retirement_failed and kept the lane; it
    now waits up to LANDED_RETIREMENT_LOCK_WAIT_S for the lock (review v6,
    A6-3)."""
    import threading

    from tests.process_guard_common import close_owned_test_process
    from tools import molt_dev_queue as queue

    main = family / "molt-src"
    _commit_file(main, "tools/proof_plan.toml", 'always = ["true"]\n', "policy")
    _git(main, "push", "-q", "origin", "HEAD:main")
    worktree = _start(family, "queued")
    tip = _commit_file(worktree, "queued.txt", "queued\n", "queued work")
    paths = queue.queue_paths(main)
    request = queue.enqueue(paths, head=tip, worktree=worktree, branch="lane/queued")
    signals = (tmp_path / "ready", tmp_path / "go")
    retire = lanes.cmd_lane_retire
    starts: list[str] = []

    def retire_while_a_start_checks(*args, **kwargs):
        start = _lane_child(
            family, signals, command="start", slug="queued", hold="_ref_sha"
        )
        try:
            _await_hold(start, signals[0], "its checks")
            release = threading.Timer(0.5, signals[1].write_text, ("go",))
            release.start()
            try:
                return retire(*args, **kwargs)
            finally:
                release.join()
        finally:
            signals[1].write_text("go", encoding="utf-8")
            out, err = start.communicate(timeout=60)
            close_owned_test_process(start)
            starts.append(out + err)

    monkeypatch.setattr(lanes, "cmd_lane_retire", retire_while_a_start_checks)
    args = molt_dev.build_parser().parse_args(
        ["queue", "run", "--repo", str(main), "--candidate-controller"]
    )
    assert args.func(args) == EXIT_OK

    assert len(starts) == 1
    assert "RESULT refused lane/queued already exists" in starts[0], starts[0]
    result = queue.read_result(paths, request.id)
    assert result is not None and result["status"] == "landed", result
    _assert_released(family / "molt-src", worktree)
    assert _ref(main, "refs/heads/lane/queued") is None
    assert _record(family, "queued")["status"] == "landed"


# The Cache Directory Tagging Specification's first line (bford.info/cachedir).
CACHEDIR_TAG = "Signature: 8a477f597d28d172789f06886806bc55\n# a tool's cache\n"


def _ignore(root: Path, worktree: Path, *patterns: str) -> None:
    _git(worktree, "config", "core.excludesFile", str(root / "ignore"))
    (root / "ignore").write_text("".join(f"{p}\n" for p in patterns), encoding="utf-8")


def _bytecode(worktree: Path, source: str) -> Path:
    """CPython's own bytecode for a source, at the PEP 3147 path beside it,
    where import writes it without a pycache prefix. Landing gates set
    PYTHONPYCACHEPREFIX, under which `cache_from_source` points outside the
    lane, so the path is built from the cache tag instead."""
    path = worktree / source
    cache = (
        path.parent / "__pycache__" / f"{path.stem}.{sys.implementation.cache_tag}.pyc"
    )
    compiled = py_compile.compile(str(path), cfile=str(cache), doraise=True)
    assert compiled is not None
    return Path(compiled)


@pytest.mark.parametrize("bytecode_ignore", ["__pycache__/", "*.pyc"])
def test_queue_retires_a_lane_whose_ignored_output_its_writers_regenerate(
    family: Path, bytecode_ignore: str
) -> None:
    """`lane start` and any test run leave bytecode, tool caches and the
    editable build's egg-info in a lane; none of it is signal, including the
    bytecode of a module or package the lane deletes."""
    from tools import molt_dev_queue as queue

    main = family / "molt-src"
    _commit_file(main, "tools/proof_plan.toml", 'always = ["true"]\n', "policy")
    _git(main, "push", "-q", "origin", "HEAD:main")
    worktree = _start(family, "built")
    deleted = ("oldmod.py", "oldpkg/__init__.py", "oldpkg/v1.2.py")
    for relative in deleted:
        _commit_file(worktree, relative, "x = 1\n", f"add {relative}")
    _ignore(
        family,
        worktree,
        bytecode_ignore,
        ".ruff_cache/",
        "*.egg-info/",
        "tmp/",
        "target/",
    )
    for relative in deleted:
        _bytecode(worktree, relative)
    _git(worktree, "rm", "-q", "-r", "--", "oldmod.py", "oldpkg")
    _git(worktree, "commit", "-q", "--no-verify", "-m", "delete the old modules")
    tip = _commit_file(worktree, "built.txt", "built\n", "built work")
    pycache = _bytecode(worktree, "provision.py").parent
    assert pycache == worktree / "__pycache__", "the lane must hold the bytecode"
    _bytecode(worktree, "tools/install_git_hooks.py")
    # pytest's assertion rewriter names its bytecode after its own version.
    (pycache / "provision.cpython-312-pytest-9.0.2.pyc").write_bytes(b"rewritten")
    if bytecode_ignore == "__pycache__/":
        # importlib publishes each file through a temporary `<file>.<id>`; a
        # `*.pyc` ignore set leaves one untracked, and the dirty check holds it.
        (pycache / "provision.cpython-313.pyc.4366061648").write_bytes(b"interrupted")
    (worktree / ".ruff_cache" / "0.17.1").mkdir(parents=True)
    (worktree / ".ruff_cache" / "CACHEDIR.TAG").write_text(
        CACHEDIR_TAG, encoding="utf-8"
    )
    (worktree / ".ruff_cache" / "0.17.1" / "1147").write_bytes(b"lint state")
    egg_info = worktree / "fixture.egg-info"
    egg_info.mkdir()
    for name in ("PKG-INFO", "SOURCES.txt", "top_level.txt", "requires.txt"):
        (egg_info / name).write_text("generated\n", encoding="utf-8")
    # pytest's cache inside a plain scratch directory (pyproject's cache_dir),
    # an empty one, and the empty lock checkout_state writes per state root.
    (worktree / "tmp" / "pytest-cache" / "v").mkdir(parents=True)
    (worktree / "tmp" / "pytest-cache" / "CACHEDIR.TAG").write_text(
        CACHEDIR_TAG, encoding="utf-8"
    )
    (worktree / "tmp" / "empty").mkdir()
    locks = worktree / "target" / ".disk_guard" / "checkout-state-locks"
    locks.mkdir(parents=True)
    (locks / ("ab" * 32 + ".lock")).write_bytes(b"")
    paths = queue.queue_paths(main)
    request = queue.enqueue(paths, head=tip, worktree=worktree, branch="lane/built")

    args = molt_dev.build_parser().parse_args(
        ["queue", "run", "--repo", str(main), "--candidate-controller"]
    )
    assert args.func(args) == EXIT_OK
    landed = queue.read_result(paths, request.id)
    assert landed is not None and landed["status"] == "landed"
    _assert_released(family / "molt-src", worktree)
    assert _ref(main, "refs/heads/lane/built") is None


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
        "pycache-stranger",
        "pycache-directory",
        "legacy-bytecode",
        "package-stranger",
        "untagged-cache",
        "egg-info-stranger",
        "scratch-stranger",
        "state-lock-with-data",
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
    elif hold == "pycache-stranger":
        _ignore(family, worktree, "__pycache__/")
        # Only bytecode belongs in __pycache__; a hand file there holds the lane.
        sentinel = _bytecode(worktree, "provision.py").parent / "provision.notes.txt"
        sentinel.write_text("unique notes\n", encoding="utf-8")
    elif hold == "pycache-directory":
        # Bytecode is a file; a directory under its name is not cache.
        _ignore(family, worktree, "__pycache__/")
        named = _bytecode(worktree, "provision.py").parent / "provision.cpython-313.pyc"
        named.mkdir()
        sentinel = named / "notes.md"
        sentinel.write_text("unique notes\n", encoding="utf-8")
    elif hold == "legacy-bytecode":
        # Outside __pycache__, CPython imports a .pyc without its source.
        _ignore(family, worktree, "*.pyc")
        sentinel = worktree / "sourceless.pyc"
        py_compile.compile(
            str(worktree / "provision.py"), cfile=str(sentinel), doraise=True
        )
    elif hold == "package-stranger":
        # Git names an untracked package beside its ignored children; a hand
        # file among them is judged on its own.
        _ignore(family, worktree, "__pycache__/", "*.log")
        (worktree / "oldpkg").mkdir()
        (worktree / "oldpkg" / "__init__.py").write_text("x = 1\n", encoding="utf-8")
        _bytecode(worktree, "oldpkg/__init__.py")
        (worktree / "oldpkg" / "__init__.py").unlink()
        sentinel = worktree / "oldpkg" / "notes.log"
        sentinel.write_text("unique notes\n", encoding="utf-8")
    elif hold == "scratch-stranger":
        # A plain directory holds the lane when one entry is signal.
        _ignore(family, worktree, "tmp/")
        (worktree / "tmp" / "pytest-cache").mkdir(parents=True)
        (worktree / "tmp" / "pytest-cache" / "CACHEDIR.TAG").write_text(
            CACHEDIR_TAG, encoding="utf-8"
        )
        sentinel = worktree / "tmp" / "notes" / "findings.md"
        sentinel.parent.mkdir()
        sentinel.write_text("unique notes\n", encoding="utf-8")
    elif hold == "state-lock-with-data":
        # A lock is metadata only while it is empty.
        _ignore(family, worktree, "target/")
        sentinel = worktree / "target" / ".disk_guard" / "checkout-state-locks"
        sentinel.mkdir(parents=True)
        sentinel = sentinel / ("cd" * 32 + ".lock")
        sentinel.write_text("unique data is not lock metadata\n", encoding="utf-8")
    elif hold == "untagged-cache":
        _ignore(family, worktree, ".tool_cache/")
        (worktree / ".tool_cache").mkdir()
        (worktree / ".tool_cache" / "CACHEDIR.TAG").write_text(
            "Signature: not the specification's\n", encoding="utf-8"
        )
        sentinel = worktree / ".tool_cache" / "result.json"
        sentinel.write_text('{"unique": true}\n', encoding="utf-8")
    elif hold == "egg-info-stranger":
        _ignore(family, worktree, "*.egg-info/")
        (worktree / "fixture.egg-info").mkdir()
        (worktree / "fixture.egg-info" / "PKG-INFO").write_text(
            "generated\n", encoding="utf-8"
        )
        sentinel = worktree / "fixture.egg-info" / "notes.md"
        sentinel.write_text("unique notes\n", encoding="utf-8")
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
            return EXIT_FAIL

        # A slot lane's cleanup step is its release (HF-491).
        monkeypatch.setattr(lanes.molt_dev_retire, "release_slot", fail_cleanup)
    before = sentinel.read_bytes()
    args = molt_dev.build_parser().parse_args(
        ["queue", "run", "--repo", str(main), "--candidate-controller"]
    )
    # Already published requests must still pass retirement, without another gate run.
    assert args.func(args) == EXIT_OK
    result = queue.read_result(paths, request.id)
    assert result is not None
    assert result["schema"] == "molt.landing-result.v2"
    assert result["status"] == "landed_retirement_failed"
    assert result["landed"] == tip
    assert "lane retire held" in result["detail"]
    if hold == "stale-input":
        # A stale input is named with the command that makes it current.
        assert "stale checkout inputs (uv-environment)" in result["detail"]
        assert "molt.checkout_provisioning ensure" in result["detail"]
    assert queue._await_landing(paths, request.id) == EXIT_FAIL
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
    assert land.func(land) == EXIT_OK
    assert started == [main]
    [request] = queue.pending(paths)
    args = molt_dev.build_parser().parse_args(
        ["queue", "run", "--repo", str(main), "--candidate-controller"]
    )
    assert args.func(args) == EXIT_OK
    assert queue._await_landing(paths, request.id) == EXIT_OK
    _assert_released(family / "molt-src", worktree)
    assert not paths.sandbox.exists()
    # A crash after retirement but before queue-result publication is idempotent
    # only with the exact recorded tip and path, not merely a missing branch.
    assert (
        lanes.cmd_lane_retire(
            _args(family, slug="published"),
            expected_tip=tip,
            expected_worktree=worktree,
        )
        == EXIT_OK
    )
    with pytest.raises(DriverError, match="reconcile its retirement receipt"):
        lanes.cmd_lane_retire(
            _args(family, slug="published"),
            expected_tip="f" * 40,
            expected_worktree=worktree,
        )


def test_land_makes_a_stale_lane_input_current_before_its_handoff(
    family: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A dependency refresh merged into a lane leaves its environment stale;
    `land` provisions it, as `uv run` syncs before it runs."""
    from tools import molt_dev_queue as queue

    main = family / "molt-src"
    worktree = _start(family, "refreshed")
    _commit_file(worktree, "work.txt", "refreshed\n", "refreshed work")
    tree = worktree / "uv-environment-tree"
    before = (tree / "runs").read_text(encoding="utf-8").count("run")
    (tree / "ready").unlink()
    monkeypatch.setattr(queue, "_start_queue_runner", lambda *_args: None)
    land = molt_dev.build_parser().parse_args(
        ["land", "--repo", str(worktree), "--candidate-controller", "--no-precheck"]
    )
    assert land.func(land) == EXIT_OK
    assert (tree / "runs").read_text(encoding="utf-8").count("run") == before + 1
    assert (tree / "ready").is_file()
    assert len(queue.pending(queue.queue_paths(main))) == 1


def test_a_stale_lane_environment_is_held_whatever_the_shell_names(
    family: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A shell that names another checkout's environment (the queue runner
    drops it; a manual handoff may not) still sees the lane's own tree."""
    main = family / "molt-src"
    worktree = _start(family, "foreign")
    tip = _commit_file(worktree, "work.txt", "foreign\n", "foreign work")
    (worktree / "uv-environment-tree" / "ready").unlink()
    monkeypatch.setenv("UV_PROJECT_ENVIRONMENT", str(main / ".venv"))
    git = lanes._driver().Git(worktree)
    with pytest.raises(DriverError, match=r"stale checkout inputs \(uv-environment\)"):
        lanes.validate_lane_handoff(git, "lane/foreign", worktree, tip)


def test_land_leaves_the_shared_hooks_to_lane_start(
    family: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The hooks serve every worktree and install only from main's sources, so
    `land` must not provision them: a lane that edits a hook, or that predates
    main's newest hooks, still lands."""
    from tools import molt_dev_queue as queue

    main = family / "molt-src"
    worktree = _start(family, "hooked")
    _commit_file(worktree, "work.txt", "hooked\n", "hooked work")
    ensure = cp.ensure

    def refuse_hooks(name, *args, **kwargs):
        if name == "git-hooks":
            raise cp.ProvisioningError("git-hooks: the lane differs from origin/main")
        return ensure(name, *args, **kwargs)

    monkeypatch.setattr(cp, "ensure", refuse_hooks)
    monkeypatch.setattr(queue, "_start_queue_runner", lambda *_args: None)
    land = molt_dev.build_parser().parse_args(
        ["land", "--repo", str(worktree), "--candidate-controller", "--no-precheck"]
    )
    assert land.func(land) == EXIT_OK
    assert len(queue.pending(queue.queue_paths(main))) == 1


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
            _assert_released(family / "molt-src", worktree)
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
    assert lanes.cmd_lane_retire(_args(family, slug="receipt")) == EXIT_OK
    assert _record(family, "receipt")["status"] == disposition
    assert _record(family, "receipt")["lane_retired"]["tip"] == tip


def _commit_candidate_controller_source(worktree: Path) -> str:
    """Real committed source for the existing controller archive authority."""

    for relative in controller_source.CONTROLLER_PATHS:
        path = worktree / relative
        if relative in {"tools", "src/molt", "config"}:
            path /= "candidate-source.txt"
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            content = {
                "src/sitecustomize.py": "# fixture source\n",
                "uv.lock": 'version = 1\nrequires-python = ">=3.12"\n',
                ".python-version": "3.12\n",
            }.get(relative, "candidate source\n")
            path.write_text(content, encoding="utf-8")
    driver = worktree / "tools" / "molt_dev.py"
    driver.write_text("# exact reviewed candidate driver\n", encoding="utf-8")
    _git(worktree, "add", "--", *controller_source.CONTROLLER_PATHS)
    return _commit_file(worktree, "candidate.txt", "candidate\n", "candidate source")


@pytest.mark.parametrize("physical_source", ["checkout", "controller"])
def test_candidate_runner_launch_uses_its_immutable_committed_source(
    family, monkeypatch, physical_source
):
    from contextlib import ExitStack
    from types import SimpleNamespace
    from tests.process_guard_common import ModuleView, run_guarded_test_process
    from molt.file_locks import _try_acquire_file_lock
    from tools import (
        molt_dev_controller as controller,
        molt_dev_queue as queue,
    )
    from tools import molt_dev_detached as detached
    from tools.venv_exec import venv_python

    main = family / "molt-src"
    candidate = _start(family, "controller-candidate")
    tip = _commit_candidate_controller_source(candidate)
    common = molt_dev._git_common_directory(main)
    assert common is not None
    paths = queue.queue_paths(main)
    main_signal = main / "primary-owned.txt"
    main_signal.write_text("unrelated dirty primary\n", encoding="utf-8")
    poison = family / "poison-pythonpath"
    poison.mkdir()
    (poison / "candidate_runner_poison.py").write_text(
        "raise RuntimeError('ambient candidate policy imported')\n", encoding="utf-8"
    )
    monkeypatch.setenv("PYTHONPATH", str(poison))
    # The helper's real non-isolated-parent branch must not decide the
    # immutable runner's isolation policy. Keep the selected interpreter.
    monkeypatch.setattr(
        detached,
        "sys",
        ModuleView(
            sys, flags=SimpleNamespace(isolated=0, utf8_mode=sys.flags.utf8_mode)
        ),
    )
    observed = []
    synced = []

    def sync_environment(root):
        assert (
            _try_acquire_file_lock(
                root.parent.parent / "controller-use" / f"{root.name}.lock"
            )
            is None
        )
        environment_root = root / controller.ENVIRONMENT_NAME
        # Stub only the canonical dependency provisioner's actuation. This
        # real, empty venv is distinct from the caller's editable environment.
        run_guarded_test_process(
            [
                sys.executable,
                "-I",
                "-S",
                "-B",
                "-m",
                "venv",
                "--without-pip",
                str(environment_root),
            ],
            cwd=main,
            check=True,
        )
        probe = run_guarded_test_process(
            [
                str(venv_python(environment_root)),
                "-I",
                "-B",
                "-c",
                "import sysconfig; print(sysconfig.get_path('purelib'))",
            ],
            cwd=main,
            check=True,
        )
        packages = Path(probe.stdout.strip()).resolve()
        assert packages.is_relative_to(environment_root.resolve())
        (packages / "candidate_dependency_marker.py").write_text(
            "VALUE = 'controller-only'\n", encoding="utf-8"
        )
        synced.append(root)

    def launch(state, command, *, cwd, environment, controller):
        assert cwd == main
        assert command[0] == str(venv_python(controller.root / queue.ENVIRONMENT_NAME))
        assert command[-1] == "--candidate-controller"
        assert str(controller.driver) in command
        prefix = command[: command.index(str(controller.driver))]
        assert "-I" in prefix and "-B" in prefix
        assert environment["PYTHONPATH"] == str(poison)
        probe = run_guarded_test_process(
            [
                *prefix,
                "-c",
                "import importlib.util,json,sys,candidate_dependency_marker; "
                "print(json.dumps([sys.flags.isolated,sys.dont_write_bytecode,"
                "importlib.util.find_spec('candidate_runner_poison') is None,"
                "candidate_dependency_marker.VALUE]))",
            ],
            env=environment,
            cwd=cwd,
            text=True,
            check=True,
        )
        assert json.loads(probe.stdout) == [1, True, True, "controller-only"]
        assert candidate not in controller.root.parents
        record = json.loads(
            (controller.root / ".molt-controller.json").read_text(encoding="utf-8")
        )
        assert record["commit"] == tip
        assert (
            controller.driver.read_text(encoding="utf-8")
            == "# exact reviewed candidate driver\n"
        )
        assert (
            _try_acquire_file_lock(
                controller.root.parent.parent
                / "controller-use"
                / f"{controller.key}.lock"
            )
            is None
        )
        assert (controller.root / queue.ENVIRONMENT_NAME).is_dir()
        observed.append(controller.root)
        return 4321

    monkeypatch.setattr(queue, "launch_detached", launch)
    monkeypatch.setattr(
        controller,
        "_sync_environment",
        sync_environment,
    )
    with ExitStack() as custody:
        source_driver = candidate / "tools" / "molt_dev.py"
        if physical_source == "controller":
            selected = custody.enter_context(
                controller_source.controller_source(candidate, common, tip)
            )
            source_driver = selected.driver
            monkeypatch.setattr(
                queue,
                "controller_source",
                lambda *_args: pytest.fail("existing source was re-extracted"),
            )
        facade = ModuleView(molt_dev, __file__=str(source_driver))
        monkeypatch.setattr(queue, "_driver", lambda: facade)
        queue._start_queue_runner(paths, main, _args(family, candidate_controller=True))
    assert len(observed) == 1
    assert synced == observed
    assert main_signal.read_text(encoding="utf-8") == "unrelated dirty primary\n"
    assert candidate.is_dir()


@pytest.mark.parametrize("change", ["tracked", "untracked", "head-during-capture"])
def test_candidate_runner_refuses_uncommitted_or_changed_source(
    family, monkeypatch, change
):
    from contextlib import contextmanager
    from tests.process_guard_common import ModuleView
    from tools import molt_dev_queue as queue

    main = family / "molt-src"
    candidate = _start(family, "candidate-source")
    _commit_candidate_controller_source(candidate)
    if change == "tracked":
        (candidate / "tools" / "molt_dev.py").write_text(
            "# dirty review work\n", encoding="utf-8"
        )
    elif change == "untracked":
        (candidate / "not-committed.py").write_text(
            "# pending review work\n", encoding="utf-8"
        )
    else:
        publish = queue.controller_source

        @contextmanager
        def advance(*args):
            with publish(*args) as selected:
                _commit_file(candidate, "later.txt", "later\n", "advanced source")
                yield selected

        monkeypatch.setattr(queue, "controller_source", advance)
    facade = ModuleView(molt_dev, __file__=str(candidate / "tools" / "molt_dev.py"))
    monkeypatch.setattr(queue, "_driver", lambda: facade)
    monkeypatch.setattr(
        queue,
        "launch_detached",
        lambda *_a, **_k: pytest.fail("unadmitted source launched"),
    )
    with pytest.raises(DriverError, match="clean committed|changed during capture"):
        queue._start_queue_runner(
            queue.queue_paths(main), main, _args(family, candidate_controller=True)
        )
    assert candidate.is_dir()


@pytest.mark.parametrize("advanced_after_publication", [False, True])
def test_old_in_lane_runner_repins_after_unlock_and_retires_without_gates(
    family, monkeypatch, advanced_after_publication
):
    from tests.process_guard_common import ModuleView
    from tools import molt_dev_queue as queue

    main = family / "molt-src"
    _commit_candidate_controller_source(main)
    _commit_file(main, "tools/proof_plan.toml", 'always = ["true"]\n', "policy")
    _git(main, "push", "-q", "origin", "HEAD:main")
    first = _start(family, "legacy-runner")
    second = _start(family, "second-lane")
    first_tip = _commit_file(first, "first.txt", "first\n", "first lane")
    second_tip = _commit_file(second, "second.txt", "second\n", "second lane")
    paths = queue.queue_paths(main)
    requests = [
        queue.enqueue(
            paths, head=first_tip, worktree=first, branch="lane/legacy-runner"
        ),
        queue.enqueue(
            paths, head=second_tip, worktree=second, branch="lane/second-lane"
        ),
    ]
    queue._write_plan(paths, [[request.id for request in requests]])
    original_commands = queue._COMMANDS
    monkeypatch.setattr(
        queue, "_COMMANDS", ModuleView(original_commands, repo_root=first)
    )
    integrate = molt_dev.cmd_integrate
    integrations = []

    def integrate_once(args):
        integrations.append(Path(args.repo))
        return integrate(args)

    monkeypatch.setattr(molt_dev, "cmd_integrate", integrate_once)
    repins = []

    def repin(args, argv):
        assert queue._runner_lock_free(paths), "repin executed under runner lock"
        assert Path.cwd() == main and Path(args.repo) == main
        assert "--candidate-controller" not in argv
        assert "--pinned-controller" not in argv
        assert all(queue.read_result(paths, request.id) is None for request in requests)
        assert [request.id for request in queue.pending(paths)] == [
            request.id for request in requests
        ]
        assert paths.plan.is_file(), "pending retirement lost its batch plan"
        assert first.is_dir() and second.is_dir()
        if advanced_after_publication:
            _commit_file(first, "later.txt", "unenrolled\n", "later lane work")
        git = molt_dev.Git(main)
        tip = git.rev_parse("origin/main")
        common = molt_dev._git_common_directory(main)
        assert common is not None
        with controller_source.controller_source(main, common, tip) as surviving:
            assert first not in surviving.root.parents
            monkeypatch.setattr(
                queue,
                "_COMMANDS",
                ModuleView(original_commands, repo_root=surviving.root),
            )
            published_args = _args(family)
            handle = queue._take_runner_lock(paths)
            assert handle is not None
            try:
                assert queue._land_one_batch(published_args, git, paths) is False
            finally:
                molt_dev._release_file_lock(handle)
        repins.append(tip)
        return EXIT_OK

    monkeypatch.setattr(queue, "run_from_pinned_controller", repin)
    args = molt_dev.build_parser().parse_args(
        ["queue", "run", "--repo", str(main), "--candidate-controller"]
    )
    monkeypatch.chdir(main)
    assert queue.cmd_queue_run(args) == EXIT_OK
    assert len(integrations) == 1 and len(repins) == 1
    first_result = queue.read_result(paths, requests[0].id)
    second_result = queue.read_result(paths, requests[1].id)
    assert first_result is not None and second_result is not None
    assert second_result["status"] == "landed"
    _assert_released(main, second)
    if advanced_after_publication:
        assert first_result["status"] == "landed_retirement_failed"
        assert "advanced after enrollment" in first_result["detail"]
        assert (first / "later.txt").read_text(encoding="utf-8") == "unenrolled\n"
        assert _ref(main, "refs/heads/lane/legacy-runner") is not None
    else:
        assert first_result["status"] == "landed"
        _assert_released(main, first)
    assert not paths.plan.exists()
