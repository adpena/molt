"""Checkout inputs provisioned on demand (src/molt/checkout_provisioning.py).

Each oracle is independent of the implementation: a fixture provisioner logs
every run it makes, Git and the filesystem show what exists, and the real
hook installer judges its own hooks. Fixture checkouts live in ``tmp_path``.
"""

from __future__ import annotations

import dataclasses
import json
import os
from pathlib import Path
import shutil
import sys

import pytest

from molt import checkout_provisioning as cp
from molt.process_environment import ProcessEnvironment
from tests.process_guard_common import run_guarded_test_process
from tests.test_molt_dev import _commit_file, _git, _init_repo
from tools.molt_dev_common import _run_fast_captured_command

REPO_ROOT = Path(__file__).resolve().parents[1]
DRIVER = REPO_ROOT / "tools" / "molt_dev.py"

# Logs each run and repairs the tree in place, as `uv sync` does: it drops
# the sentinel first, so a stale one never survives. "fail" completes the tree
# and then exits 3; "no-sentinel" exits 0 without the sentinel.
_PROVISIONER = """\
import sys
from pathlib import Path
lock = Path("fixture.lock").read_text(encoding="utf-8")
with open("provision-runs.log", "a", encoding="utf-8") as log:
    log.write(lock.strip() + "\\n")
tree = Path("tree")
tree.mkdir(exist_ok=True)
(tree / "ready").unlink(missing_ok=True)
(tree / "partial").write_text(lock, encoding="utf-8")
if "no-sentinel" not in lock:
    (tree / "ready").write_text(lock, encoding="utf-8")
if "fail" in lock:
    print("fixture provisioner refused the lock", file=sys.stderr)
    raise SystemExit(3)
"""

# The owner-installation contract of tools/install_git_hooks.py: --check
# exits 0 when current and 1 when stale; no argument installs.
_INSTALLER = """\
import sys
from pathlib import Path
source = Path("hook-source.txt").read_text(encoding="utf-8")
installed = Path("installed-hook.txt")
with open("installer-runs.log", "a", encoding="utf-8") as log:
    log.write(" ".join(sys.argv[1:]) or "install")
    log.write("\\n")
if sys.argv[1:] == ["--check"]:
    current = installed.is_file() and installed.read_text(encoding="utf-8") == source
    raise SystemExit(0 if current else 1)
if "broken" in source:
    print("installer cannot write the hook", file=sys.stderr)
    raise SystemExit(2)
installed.write_text("stuck" if "stuck" in source else source, encoding="utf-8")
"""


@pytest.fixture
def checkout(tmp_path: Path) -> Path:
    root = tmp_path / "checkout"
    _init_repo(root)
    (root / "provision.py").write_text(_PROVISIONER, encoding="utf-8")
    (root / "fixture.lock").write_text("v1\n", encoding="utf-8")
    (root / "tools").mkdir()
    (root / "tools" / "installer.py").write_text(_INSTALLER, encoding="utf-8")
    (root / "hook-source.txt").write_text("hook v1\n", encoding="utf-8")
    return root


def _tree(checkout: Path) -> cp.LockedTree:
    python = Path(sys.executable)
    environment = ProcessEnvironment(os.environ).updated(
        {"PATH": str(python.parent) + os.pathsep + os.environ.get("PATH", "")}
    )
    return cp.LockedTree(
        name="fixture-tree",
        checkout=checkout.resolve(),
        lock_files=("fixture.lock",),
        tree=checkout.resolve() / "tree",
        sentinel="ready",
        tool=python.stem,
        arguments=("provision.py",),
        environment=environment,
    )


def _hooks(checkout: Path) -> cp.OwnedInstallation:
    return cp.OwnedInstallation(
        name="fixture-hooks",
        checkout=checkout.resolve(),
        owner="tools/installer.py",
        environment=ProcessEnvironment(os.environ),
    )


def _runs(checkout: Path, log: str = "provision-runs.log") -> list[str]:
    path = checkout / log
    return path.read_text(encoding="utf-8").splitlines() if path.exists() else []


def test_a_matching_stamp_runs_no_provisioner(checkout: Path) -> None:
    spec = _tree(checkout)
    assert cp.input_state(spec) == "absent"

    first = cp.ensure_input(spec)
    assert first.provisioned
    assert _runs(checkout) == ["v1"]
    assert (checkout / "tree" / "ready").read_text(encoding="utf-8") == "v1\n"
    assert spec.stamp.is_file()

    second = cp.ensure_input(spec)
    assert not second.provisioned
    assert _runs(checkout) == ["v1"], "a current stamp must not run the provisioner"
    # The key is the lock's content: rewriting the same bytes stays current.
    (checkout / "fixture.lock").write_text("v1\n", encoding="utf-8")
    assert not cp.ensure_input(spec).provisioned
    assert _runs(checkout) == ["v1"]


def test_a_lock_change_reprovisions(checkout: Path) -> None:
    spec = _tree(checkout)
    cp.ensure_input(spec)
    (checkout / "fixture.lock").write_text("v2\n", encoding="utf-8")
    assert cp.input_state(spec) == "stale"

    assert cp.ensure_input(spec).provisioned
    assert _runs(checkout) == ["v1", "v2"]
    assert (checkout / "tree" / "ready").read_text(encoding="utf-8") == "v2\n"
    assert cp.input_state(spec) == "current"


def test_a_deleted_tree_takes_its_stamp_with_it(checkout: Path) -> None:
    spec = _tree(checkout)
    cp.ensure_input(spec)
    shutil.rmtree(checkout / "tree")
    assert cp.input_state(spec) == "absent"
    assert cp.ensure_input(spec).provisioned
    assert _runs(checkout) == ["v1", "v1"]


@pytest.mark.parametrize(
    ("lock", "diagnostic"),
    [
        ("fail\n", "fixture provisioner refused the lock"),
        ("no-sentinel\n", "does not exist"),
    ],
)
def test_a_failure_leaves_no_stamp_and_no_partial_success(
    checkout: Path, lock: str, diagnostic: str
) -> None:
    spec = _tree(checkout)
    cp.ensure_input(spec)
    (checkout / "fixture.lock").write_text(lock, encoding="utf-8")

    with pytest.raises(cp.ProvisioningError) as raised:
        cp.ensure_input(spec)
    message = str(raised.value)
    assert diagnostic in message
    assert "provision.py" in message, "the error names the command"
    # The provisioner wrote part of the tree, and the old stamp is gone.
    assert (checkout / "tree" / "partial").read_text(encoding="utf-8") == lock
    assert not spec.stamp.exists()
    assert cp.input_state(spec) == "absent"
    # Nothing caches the failure or the old success: the next call runs again.
    with pytest.raises(cp.ProvisioningError):
        cp.ensure_input(spec)
    assert _runs(checkout) == ["v1", lock.strip(), lock.strip()]

    # The failed run's tree is untrusted even under the old lock again.
    (checkout / "fixture.lock").write_text("v1\n", encoding="utf-8")
    assert cp.ensure_input(spec).provisioned
    assert _runs(checkout)[-1] == "v1"
    assert cp.input_state(spec) == "current"


def test_a_missing_tool_or_lock_fails_loud(checkout: Path) -> None:
    missing_tool = dataclasses.replace(
        _tree(checkout), tool="molt-no-such-package-manager"
    )
    with pytest.raises(cp.ProvisioningError, match="not on PATH"):
        cp.ensure_input(missing_tool)
    (checkout / "fixture.lock").unlink()
    with pytest.raises(cp.ProvisioningError, match="fixture.lock is unreadable"):
        cp.ensure_input(_tree(checkout))
    assert _runs(checkout) == []


@pytest.mark.parametrize("caller", [None, "caller value"])
def test_a_caller_choice_reaches_the_provisioner_explicitly(
    checkout: Path, caller: str | None
) -> None:
    (checkout / "choice.py").write_text("print('tool default')\n", encoding="utf-8")
    (checkout / "record.py").write_text(
        "import os\nfrom pathlib import Path\n"
        "Path('tree').mkdir(exist_ok=True)\n"
        "Path('tree/ready').write_text(os.environ['FIXTURE_TOOL_CACHE'])\n",
        encoding="utf-8",
    )
    spec = _tree(checkout)
    environment = spec.environment.without(("FIXTURE_TOOL_CACHE",))
    if caller is not None:
        environment = environment.updated({"FIXTURE_TOOL_CACHE": caller})
    spec = dataclasses.replace(
        spec,
        arguments=("record.py",),
        environment=environment,
        caller_choices=(("FIXTURE_TOOL_CACHE", ("choice.py",)),),
    )
    assert cp.ensure_input(spec).provisioned
    recorded = (checkout / "tree" / "ready").read_text(encoding="utf-8")
    assert recorded == (caller or "tool default")


def test_ensure_all_reports_every_failure_and_keeps_each_success(
    checkout: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    (checkout / "hook-source.txt").write_text("broken\n", encoding="utf-8")
    table = {"uv-environment": _tree(checkout), "git-hooks": _hooks(checkout)}
    monkeypatch.setattr(
        cp, "checkout_input", lambda name, *_args, **_kwargs: table[name]
    )

    with pytest.raises(cp.ProvisioningError) as raised:
        cp.ensure_all(checkout, names=("uv-environment", "git-hooks"))
    assert "installer cannot write the hook" in str(raised.value)
    # The tree's own provisioning stands; only the failed input stays stale.
    assert cp.input_state(table["uv-environment"]) == "current"
    assert cp.input_state(table["git-hooks"]) == "stale"


def test_owner_installation_uses_the_owner_check_and_install(checkout: Path) -> None:
    spec = _hooks(checkout)
    assert cp.input_state(spec) == "stale"
    assert cp.ensure_input(spec).provisioned
    # The state probe, then ensure's check, install and confirming check.
    assert _runs(checkout, "installer-runs.log") == [
        "--check",
        "--check",
        "install",
        "--check",
    ]
    assert (checkout / "installed-hook.txt").read_text(encoding="utf-8") == "hook v1\n"

    assert not cp.ensure_input(spec).provisioned
    assert _runs(checkout, "installer-runs.log")[4:] == ["--check"]

    (checkout / "hook-source.txt").write_text("hook stuck\n", encoding="utf-8")
    with pytest.raises(cp.ProvisioningError, match="still reports it stale"):
        cp.ensure_input(spec)


def test_owner_installation_runs_isolated_from_a_foreign_environment(
    checkout: Path, tmp_path: Path
) -> None:
    # Another checkout's environment preloads its own modules at startup.
    foreign = tmp_path / "foreign-site"
    foreign.mkdir()
    (foreign / "sitecustomize.py").write_text(
        "raise SystemExit('foreign sitecustomize loaded')\n", encoding="utf-8"
    )
    spec = dataclasses.replace(
        _hooks(checkout),
        environment=ProcessEnvironment(os.environ).updated(
            {"PYTHONPATH": str(foreign)}
        ),
    )
    assert cp.ensure_input(spec).provisioned
    assert cp.input_state(spec) == "current"


def test_the_hook_input_agrees_with_the_installer_itself() -> None:
    from tools import install_git_hooks

    expected = (
        "current"
        if install_git_hooks.install(check=True, uninstall=False) == 0
        else "stale"
    )
    spec = cp.checkout_input("git-hooks", REPO_ROOT)
    assert isinstance(spec, cp.OwnedInstallation)
    assert spec.scope == "repository"
    assert cp.input_state(spec) == expected


def test_declared_inputs_name_their_locks_and_trees(tmp_path: Path) -> None:
    environ = {"PATH": os.environ.get("PATH", "")}
    acorn = cp.checkout_input("browser-acorn", tmp_path, environ)
    assert isinstance(acorn, cp.LockedTree)
    assert acorn.tool == "npm"
    assert acorn.arguments[:3] == ("ci", "--ignore-scripts", "--prefer-offline")
    assert acorn.arguments[-2:] == ("--prefix", "tools/browser_asset_graph")
    assert acorn.tree == tmp_path.resolve() / "tools/browser_asset_graph/node_modules"
    assert "tools/browser_asset_graph/package-lock.json" in acorn.lock_files

    uv = cp.checkout_input("uv-environment", tmp_path, environ)
    assert isinstance(uv, cp.LockedTree)
    assert (uv.tool, uv.arguments) == ("uv", ("sync", "--frozen"))
    assert uv.tree == tmp_path.resolve() / ".venv"
    # uv's own rule: UV_PROJECT_ENVIRONMENT, relative to the project.
    named = cp.checkout_input(
        "uv-environment", tmp_path, {**environ, "UV_PROJECT_ENVIRONMENT": "envs/a"}
    )
    assert named.tree == tmp_path.resolve() / "envs" / "a"
    assert not cp.declares(uv)
    (tmp_path / "uv.lock").write_text("", encoding="utf-8")
    assert cp.declares(uv)


def _uv_project(root: Path) -> None:
    (root / "pyproject.toml").write_text(
        '[project]\nname = "fixture"\nversion = "0"\nrequires-python = ">=3.12"\n'
        "dependencies = []\n",
        encoding="utf-8",
    )
    (root / ".python-version").write_text("3.12\n", encoding="utf-8")
    uv = os.environ.get("UV") or shutil.which("uv")
    assert uv, "the uv environment input requires uv"
    locked = _run_fast_captured_command([uv, "lock", "--offline", "--quiet"], cwd=root)
    assert locked.returncode == 0, locked.stderr


@pytest.mark.parametrize("named", [False, True], ids=["dot-venv", "named"])
def test_uv_environment_builds_the_selected_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, named: bool
) -> None:
    root = tmp_path / "project"
    _init_repo(root)
    _uv_project(root)
    # The memory guard installs dx's defaults (environment, cache) into each
    # child; keep them private, so a misdirected sync stays visible.
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "artifacts"))
    environ = dict(os.environ)
    environ.pop("VIRTUAL_ENV", None)
    environ.pop("UV_PROJECT_ENVIRONMENT", None)
    selected = root / ".venv"
    if named:
        selected = tmp_path / "managed env"
        environ["UV_PROJECT_ENVIRONMENT"] = str(selected)

    first = cp.ensure("uv-environment", root, environ)
    assert first.provisioned
    assert (selected / "pyvenv.cfg").is_file()
    assert (selected / cp.STAMP_NAME).is_file()
    if named:
        assert not (root / ".venv").exists(), "uv's rule selected the named one"
    # Neither dx default reached uv: not its environment, not its cache.
    assert not (tmp_path / "artifacts" / "uv-project-envs").exists()
    assert not (tmp_path / "artifacts" / ".uv-cache").exists()
    assert not cp.ensure("uv-environment", root, environ).provisioned


def test_integrate_child_builds_a_missing_inherited_uv_environment(
    tmp_path: Path,
) -> None:
    """A guarded child inherits an UV_PROJECT_ENVIRONMENT nobody built."""
    seed = tmp_path / "seed"
    _init_repo(seed)
    _uv_project(seed)
    gate = "python -c \"import sys; print('gate-prefix=' + sys.prefix)\""
    (seed / "tools").mkdir()
    (seed / "tools/proof_plan.toml").write_text(
        "always = [" + json.dumps(gate) + "]\n", encoding="utf-8"
    )
    _git(seed, "add", "--", ".")
    _commit_file(seed, "README.md", "seed\n", "seed")
    origin = tmp_path / "origin.git"
    _git(tmp_path, "clone", "--bare", "-q", str(seed), str(origin))
    work = tmp_path / "work"
    _git(tmp_path, "clone", "-q", str(origin), str(work))
    _git(work, "config", "user.email", "dev@molt.test")
    _git(work, "config", "user.name", "Molt Dev")
    _commit_file(work, "feature.txt", "feature\n", "feature")

    managed = tmp_path / "managed" / "dx__py3.12__src-work"
    env = dict(os.environ)
    for name in ("MOLT_VENV", "VIRTUAL_ENV"):
        env.pop(name, None)
    env["UV_PROJECT_ENVIRONMENT"] = str(managed)
    env["UV_PYTHON_DOWNLOADS"] = "never"
    result = run_guarded_test_process(
        [
            sys.executable,
            str(DRIVER),
            "integrate",
            "--repo",
            str(work),
            "--no-push",
            "--candidate-controller",
        ],
        cwd=REPO_ROOT,
        env=ProcessEnvironment(env),
        timeout=300,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert f"gate-prefix={managed.resolve()}" in result.stdout
    assert (managed / "pyvenv.cfg").is_file()
    assert (managed / cp.STAMP_NAME).is_file()


def _acorn_policy(**extra) -> dict:
    return {"name": "acorn-tree", "checkout_input": "browser-acorn", **extra}


def test_admission_provisions_only_selected_checkout_inputs(
    checkout: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tools import proof_plan

    spec = _tree(checkout)
    monkeypatch.setattr(cp, "checkout_input", lambda name, *_args, **_kwargs: spec)
    data = {"toolchain_policy": [_acorn_policy()]}
    policies = proof_plan.load_toolchain_policies(data, root=checkout)
    (policy,) = policies
    assert policy.data["setup_command"] == cp.provisioning_command("browser-acorn")

    assert proof_plan._provision_checkout_inputs(policies, (), root=checkout) is None
    assert _runs(checkout) == [], "an unselected input is not provisioned"
    assert (
        proof_plan._provision_checkout_inputs(policies, ("acorn-tree",), root=checkout)
        is None
    )
    assert cp.input_state(spec) == "current"

    (checkout / "fixture.lock").write_text("fail\n", encoding="utf-8")
    monkeypatch.setattr(proof_plan, "toolchain_fingerprints", lambda *_a, **_k: {})
    with pytest.raises(
        ValueError, match="checkout input provisioning failed"
    ) as raised:
        proof_plan.admit_toolchains(policies, ("acorn-tree",), root=checkout)
    assert "provision.py" in str(raised.value)


@pytest.mark.parametrize(
    ("row", "message"),
    [
        (_acorn_policy(setup_command="manual"), "derives its setup command"),
        (_acorn_policy(checkout_input="no-such-input"), "unknown checkout input"),
        (_acorn_policy(package_lock="other/package-lock.json"), "is not a lock file"),
    ],
)
def test_checkout_input_policies_keep_one_authority(row: dict, message: str) -> None:
    from tools import proof_plan

    with pytest.raises(ValueError, match=message):
        proof_plan.load_toolchain_policies({"toolchain_policy": [row]}, root=REPO_ROOT)


def test_the_committed_acorn_policy_is_the_checkout_input() -> None:
    import tomllib

    from tools import proof_plan

    data = tomllib.loads(
        (REPO_ROOT / "tools/proof_plan.toml").read_text("utf-8", encoding="utf-8")
    )
    policies = proof_plan.load_toolchain_policies(
        data, root=REPO_ROOT, names=("browser-acorn",)
    )
    policy = next(policy for policy in policies if policy.name == "browser-acorn")
    assert policy.data["setup_command"] == cp.provisioning_command("browser-acorn")
    assert policy.data["package_lock"] in cp.locked_input_files("browser-acorn")


# The stub reports current only after it ran: its marker is the oracle for
# whether an install happened.
_HOOK_INSTALLER_STUB = """\
import sys
from pathlib import Path
marker = Path(__file__).resolve().parents[1] / ".git" / "stub-hooks-installed"
if sys.argv[1:] == ["--check"]:
    raise SystemExit(0 if marker.exists() else 1)
marker.write_text("installed\\n", encoding="utf-8")
"""


def _hook_checkout(tmp_path: Path) -> Path:
    origin = tmp_path / "origin.git"
    _git(tmp_path, "init", "-q", "--bare", "-b", "main", str(origin))
    work = tmp_path / "work"
    _init_repo(work)
    _commit_file(work, "tools/install_git_hooks.py", _HOOK_INSTALLER_STUB, "installer")
    _commit_file(work, ".githooks/commit-msg", "#!/bin/sh\nexit 0\n", "hooks")
    _git(work, "remote", "add", "origin", str(origin))
    _git(work, "push", "-q", "origin", "HEAD:main")
    _git(work, "fetch", "-q", "origin")
    return work


def test_shared_hooks_install_only_from_sources_matching_main(tmp_path: Path) -> None:
    work = _hook_checkout(tmp_path)
    marker = work / ".git" / "stub-hooks-installed"
    environ = {"PATH": os.environ.get("PATH", "")}

    # An unlanded hook change must not reach every other worktree.
    (work / ".githooks" / "commit-msg").write_text(
        "#!/bin/sh\nexit 1\n", encoding="utf-8"
    )
    with pytest.raises(cp.ProvisioningError, match="differs from origin/main"):
        cp.ensure("git-hooks", work, environ)
    assert not marker.exists()

    _git(work, "checkout", "-q", "--", ".githooks")
    assert cp.ensure("git-hooks", work, environ).provisioned
    assert marker.exists()


def test_provisioning_guard_owner_is_distinct_from_content_and_compiler_inputs(
    checkout, tmp_path, monkeypatch
):
    foreign = tmp_path / "compiler-inputs"
    (foreign / "tools").mkdir(parents=True)
    (foreign / "pyproject.toml").write_text(
        "[project]\nname='input'\n", encoding="utf-8"
    )
    (foreign / "tools/harness_memory_guard.py").write_text("", encoding="utf-8")
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(foreign))
    spec = dataclasses.replace(_tree(checkout), guard_source_root=REPO_ROOT)
    assert spec.environment["MOLT_SOURCE_ROOT"] == str(foreign)
    digest = cp._action_digest(spec, cp._lock_digests(spec))
    alternate_owner = dataclasses.replace(spec, guard_source_root=tmp_path)
    assert (
        cp._action_digest(alternate_owner, cp._lock_digests(alternate_owner)) == digest
    )
    assert cp.ensure_input(spec).provisioned
    assert _runs(checkout) == ["v1"]
    before = spec.stamp.read_bytes()
    with pytest.raises(cp.ProvisioningError, match="absolute"):
        dataclasses.replace(spec, guard_source_root=Path("relative-owner"))
    assert spec.stamp.read_bytes() == before
    assert not cp.ensure_input(spec).provisioned
