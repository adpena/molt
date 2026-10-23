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
from tests.test_molt_dev import _commit_file, _git, _init_repo, kept_gate_stdout
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


def test_locked_tree_command_cwd_is_owned_and_changes_the_action(
    checkout: Path,
) -> None:
    spec = _tree(checkout)
    nested = checkout / "nested"
    nested.mkdir()
    (nested / "provision.py").write_text(
        "from pathlib import Path\n"
        "assert Path.cwd().name == 'nested'\n"
        "tree = Path('../tree')\n"
        "tree.mkdir(exist_ok=True)\n"
        "(tree / 'ready').write_text('nested', encoding='utf-8')\n",
        encoding="utf-8",
    )
    selected = dataclasses.replace(spec, command_cwd="nested")
    assert cp.ensure_input(selected).provisioned
    assert (checkout / "tree/ready").read_text(encoding="utf-8") == "nested"
    assert cp.input_state(selected) == "current"
    assert cp.input_state(spec) == "stale"
    assert not cp.ensure_input(selected).provisioned
    for invalid in ("../outside", str(checkout.resolve())):
        with pytest.raises(ValueError, match="inside the checkout"):
            dataclasses.replace(spec, command_cwd=invalid)


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


@pytest.mark.parametrize(
    ("caller_device", "artifact_device", "artifact_exists", "expected"),
    [
        (1, 2, True, "caller"),  # the caller's store shares the tree's volume
        (2, 1, True, "artifact"),  # another volume: dx's same-volume store
        (2, 2, True, "caller"),  # neither shares it: the tool copies
        (2, 1, False, "caller"),  # a store that does not exist is cold
    ],
)
def test_a_store_on_another_volume_yields_to_one_on_the_trees_volume(
    checkout: Path,
    monkeypatch: pytest.MonkeyPatch,
    caller_device: int,
    artifact_device: int,
    artifact_exists: bool,
    expected: str,
) -> None:
    stores = {
        "caller": checkout / "caller-store",
        "artifact": checkout / "artifact-store",
    }
    stores["caller"].mkdir()
    if artifact_exists:
        stores["artifact"].mkdir()
    devices = {stores["caller"]: caller_device, stores["artifact"]: artifact_device}
    # The tree, and everything else, sits on device 1.
    monkeypatch.setattr(cp, "_device_of", lambda path: devices.get(path, 1))
    monkeypatch.setattr(
        cp,
        "_dx_choice",
        lambda variable, spec: (
            stores["artifact"] if variable == "FIXTURE_TOOL_CACHE" else None
        ),
    )
    (checkout / "record.py").write_text(
        "import os\nfrom pathlib import Path\n"
        "Path('tree').mkdir(exist_ok=True)\n"
        "Path('tree/ready').write_text(os.environ['FIXTURE_TOOL_CACHE'])\n",
        encoding="utf-8",
    )
    spec = _tree(checkout)
    spec = dataclasses.replace(
        spec,
        arguments=("record.py",),
        environment=spec.environment.updated(
            {"FIXTURE_TOOL_CACHE": str(stores["caller"])}
        ),
        same_volume_choices=("FIXTURE_TOOL_CACHE",),
    )
    assert cp.ensure_input(spec).provisioned
    recorded = (checkout / "tree" / "ready").read_text(encoding="utf-8")
    assert recorded == str(stores[expected])


@pytest.mark.parametrize(
    ("dx_config", "expected"),
    [
        ("", ".uv-cache"),  # dx's default: the cache under its artifact root
        (
            '[tool.molt.dx.env]\nUV_CACHE_DIR = "{artifact_root}/shared-uv"\n',
            "shared-uv",
        ),  # the checkout's own dx configuration
    ],
)
def test_the_same_volume_store_is_the_one_dx_chooses(
    checkout: Path, dx_config: str, expected: str
) -> None:
    ext_root = (checkout.parent / "artifacts").resolve()
    ext_root.mkdir()
    (checkout / "pyproject.toml").write_text(dx_config, encoding="utf-8")
    spec = cp.checkout_input("uv-environment", checkout)
    spec = dataclasses.replace(
        spec,
        environment=spec.environment.updated(
            {"MOLT_EXT_ROOT": str(ext_root), "UV_CACHE_DIR": "/caller/uv-cache"}
        ),
    )
    # The caller's own value is what is being replaced, so dx never sees it.
    assert cp._dx_choice("UV_CACHE_DIR", spec) == ext_root / expected


def test_default_and_explicit_inputs_use_the_same_real_provisioner(
    checkout: Path, monkeypatch: pytest.MonkeyPatch, capsys
) -> None:
    # This fixture provisions generic trees, without an editable distribution.
    (checkout / "pyproject.toml").write_text("[tool.fixture]\n", encoding="utf-8")
    (checkout / "named_provision.py").write_text(
        "import sys\nfrom pathlib import Path\n"
        "name = sys.argv[1]\n"
        "tree = Path(name)\n"
        "tree.mkdir(exist_ok=True)\n"
        "(tree / 'ready').write_text('ready', encoding='utf-8')\n"
        "with Path('provision-runs.log').open('a', encoding='utf-8') as log:\n"
        "    log.write(name + '\\n')\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(
        cp,
        "checkout_input",
        lambda name, *_args, **_kwargs: dataclasses.replace(
            _tree(checkout),
            name=name,
            tree=checkout / name,
            arguments=("named_provision.py", name),
        ),
    )
    assert [item.name for item in cp.ensure_all(checkout)] == [
        "uv-environment",
        "browser-acorn",
        "git-hooks",
    ]
    assert len(_runs(checkout)) == 3
    assert cp.main(["status", "--checkout", str(checkout), "--json"]) == 0
    assert set(json.loads(capsys.readouterr().out)) == {
        "uv-environment",
        "browser-acorn",
        "git-hooks",
    }
    assert (
        cp.main(["ensure", "edgebox-typescript", "--checkout", str(checkout), "--json"])
        == 0
    )
    assert len(_runs(checkout)) == 4
    assert "edgebox-typescript" in capsys.readouterr().out


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
    assert cp.DEFAULT_CHECKOUT_INPUT_NAMES == (
        "uv-environment",
        "browser-acorn",
        "git-hooks",
    )
    assert "edgebox-typescript" in cp.CHECKOUT_INPUT_NAMES
    environ = {"PATH": os.environ.get("PATH", "")}
    acorn = cp.checkout_input("browser-acorn", tmp_path, environ)
    assert isinstance(acorn, cp.LockedTree)
    assert acorn.tool == "npm" and acorn.pinned
    assert acorn.package_manifest == "tools/browser_asset_graph/package.json"
    assert acorn.arguments[:3] == ("ci", "--ignore-scripts", "--prefer-offline")
    assert acorn.arguments[-2:] == ("--prefix", "tools/browser_asset_graph")
    assert acorn.tree == tmp_path.resolve() / "tools/browser_asset_graph/node_modules"
    assert "tools/browser_asset_graph/package-lock.json" in acorn.lock_files

    edgebox = cp.checkout_input("edgebox-typescript", tmp_path, environ)
    assert isinstance(edgebox, cp.LockedTree)
    assert edgebox.tool == "npm" and edgebox.pinned
    assert edgebox.package_manifest == f"{edgebox.command_cwd}/package.json"
    assert edgebox.arguments == (
        "ci",
        "--ignore-scripts",
        "--prefer-offline",
        "--no-audit",
        "--no-fund",
    )
    assert edgebox.command_cwd == "examples/edgebox/packages/edgebox-cloudflare"
    assert edgebox.tree == tmp_path.resolve() / edgebox.command_cwd / "node_modules"
    assert edgebox.sentinel == "typescript/bin/tsc"
    assert edgebox.lock_files == (
        f"{edgebox.command_cwd}/package-lock.json",
        f"{edgebox.command_cwd}/package.json",
    )

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


def _uv_project(root: Path, *, editable: bool = False) -> None:
    (root / "pyproject.toml").write_text(
        '[project]\nname = "fixture"\nversion = "0"\nrequires-python = ">=3.12"\n'
        "dependencies = []\n",
        encoding="utf-8",
    )
    if editable:
        with (root / "pyproject.toml").open("a", encoding="utf-8") as project:
            project.write(
                '[build-system]\nrequires = ["setuptools"]\nbuild-backend = "setuptools.build_meta"\n'
            )
        (root / "fixture.py").write_text("VALUE = 'owner'\n", encoding="utf-8")
    (root / ".python-version").write_text("3.12\n", encoding="utf-8")
    uv = os.environ.get("UV") or shutil.which("uv")
    assert uv, "the uv environment input requires uv"
    locked = _run_fast_captured_command([uv, "lock", "--offline", "--quiet"], cwd=root)
    assert locked.returncode == 0, locked.stderr


@pytest.mark.parametrize("editable", [False, True], ids=["manual", "editable"])
@pytest.mark.parametrize("named", [False, True], ids=["dot-venv", "named"])
def test_uv_environment_builds_the_selected_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, named: bool, editable: bool
) -> None:
    root = tmp_path / "project"
    _init_repo(root)
    _uv_project(root, editable=editable)
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

    if not editable:
        # A stamp is freshness evidence, not an ownership registry. A manual
        # environment with no editable project remains usable from another
        # checkout; the real installed metadata decides cold admission.
        other = tmp_path / "manual-other"
        _init_repo(other)
        for name in ("pyproject.toml", "uv.lock", ".python-version"):
            shutil.copyfile(root / name, other / name)
        shared = {**environ, "UV_PROJECT_ENVIRONMENT": str(selected)}
        assert cp.ensure("uv-environment", other, shared).provisioned
        assert not cp.ensure("uv-environment", other, shared).provisioned
        return
    # Read the real installed PEP 610 record independently of the provisioner.
    records = list(selected.glob("**/fixture-*.dist-info/direct_url.json"))
    assert len(records) == 1
    metadata = records[0]
    installed = metadata.read_bytes()
    assert json.loads(installed)["url"] == root.as_uri()
    # A shipped module invoked outside a checkout cannot import tools. Use its
    # actual UV-created launcher and no-site probe without exposing tools at all.
    import packaging

    isolated = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-S",
            "-c",
            "import sys, importlib.util\n"
            "sys.path[:0] = sys.argv[1:3]\n"
            "assert importlib.util.find_spec('tools') is None\n"
            "from molt import checkout_provisioning as cp\n"
            "from pathlib import Path\n"
            "spec = cp.checkout_input('uv-environment', Path(sys.argv[3]), "
            "{'UV_PROJECT_ENVIRONMENT': sys.argv[4]})\n"
            "assert cp._uv_editable_project(spec) is not None\n",
            str(Path(cp.__file__).resolve().parents[1]),
            str(Path(packaging.__file__).resolve().parents[1]),
            str(root),
            str(selected),
        ],
        cwd=tmp_path,
        capture_output=True,
        text=True,
    )
    assert isolated.returncode == 0, isolated.stderr
    stamp = (selected / cp.STAMP_NAME).read_bytes()
    other = tmp_path / "other"
    _init_repo(other)
    for name in ("pyproject.toml", "uv.lock", ".python-version", "fixture.py"):
        shutil.copyfile(root / name, other / name)
    shared = {**environ, "UV_PROJECT_ENVIRONMENT": str(selected)}
    other_spec = cp.checkout_input("uv-environment", other, shared)
    own_spec = cp.checkout_input("uv-environment", root, shared)
    assert cp.input_state(other_spec) == "stale", (
        "equal locks cannot transfer an editable source"
    )
    with pytest.raises(cp.ProvisioningError, match="MOLT_VENV.*read-only"):
        cp.ensure_input(other_spec)
    assert metadata.read_bytes() == installed
    assert (selected / cp.STAMP_NAME).read_bytes() == stamp
    from tools.molt_dev import _resolve_repo_venv

    # Explicit borrowing is a read-only contract even for this foreign source.
    borrowed = ProcessEnvironment({**shared, "MOLT_VENV": str(selected)})
    assert _resolve_repo_venv(other, borrowed) == selected.resolve()
    assert metadata.read_bytes() == installed
    assert (selected / cp.STAMP_NAME).read_bytes() == stamp
    # An unstamped legacy environment must be inspected, not blindly rewritten.
    (selected / cp.STAMP_NAME).unlink()
    with pytest.raises(cp.ProvisioningError, match="MOLT_VENV.*read-only"):
        cp.ensure_input(other_spec)
    assert metadata.read_bytes() == installed
    assert not (selected / cp.STAMP_NAME).exists()
    assert cp.ensure_input(own_spec).provisioned
    # The warm path must inspect actual metadata, not trust a matching stamp.
    foreign = {"url": other.as_uri(), "dir_info": {"editable": True}}
    metadata.write_text(json.dumps(foreign), encoding="utf-8")
    assert cp.input_state(own_spec) == "stale"
    with pytest.raises(cp.ProvisioningError, match="MOLT_VENV.*read-only"):
        cp.ensure_input(own_spec)
    assert json.loads(metadata.read_bytes()) == foreign
    metadata.write_bytes(installed)
    assert not cp.ensure_input(own_spec).provisioned
    # A malformed editable authority is not a manual environment. Exercise
    # actual installed metadata on both stamped and unstamped cold paths;
    # refusal must precede sync or any stamp/metadata mutation.
    stamp_path = selected / cp.STAMP_NAME
    current_stamp = stamp_path.read_bytes()
    for dir_info in (
        {"editable": True, "extra": "unsupported"},
        {"editable": 1},
        {"editable": 0},
        {"editable": None},
        {"editable": "false"},
        None,
    ):
        altered = json.dumps({"url": other.as_uri(), "dir_info": dir_info}).encode()
        for stamped in (True, False):
            metadata.write_bytes(altered)
            if stamped:
                stamp_path.write_bytes(current_stamp)
            else:
                stamp_path.unlink(missing_ok=True)
            with pytest.raises(cp.ProvisioningError):
                cp.ensure_input(own_spec)
            assert metadata.read_bytes() == altered
            if stamped:
                assert stamp_path.read_bytes() == current_stamp
            else:
                assert not stamp_path.exists()
    # PEP 610 explicitly defines absent and boolean-false editable markers as
    # noneditable. Inspect these real records without syncing the fixture.
    for dir_info in ({}, {"editable": False}):
        manual = json.dumps({"url": other.as_uri(), "dir_info": dir_info}).encode()
        metadata.write_bytes(manual)
        assert cp._uv_editable_project(own_spec) is None
        assert metadata.read_bytes() == manual
        assert not stamp_path.exists()
    metadata.write_bytes(installed)
    stamp_path.write_bytes(current_stamp)
    assert not cp.ensure_input(own_spec).provisioned


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
    assert kept_gate_stdout(result.stdout) == [f"gate-prefix={managed.resolve()}\n"]
    assert (managed / "pyvenv.cfg").is_file()
    assert (managed / cp.STAMP_NAME).is_file()


def _acorn_policy(**extra) -> dict:
    return {"name": "acorn-tree", "checkout_input": "browser-acorn", **extra}


def test_admission_provisions_only_selected_checkout_inputs(
    checkout: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from tools import proof_plan

    spec = _tree(checkout)
    seen_environments = []

    def selected_input(name, _checkout, environ=None, **_kwargs):
        seen_environments.append(environ)
        return spec

    monkeypatch.setattr(cp, "checkout_input", selected_input)
    marked = {**os.environ, "MOLT_ADMISSION_MARK": "selected"}
    data = {"toolchain_policy": [_acorn_policy()]}
    policies = proof_plan.load_toolchain_policies(data, root=checkout)
    (policy,) = policies
    assert policy.data["setup_command"] == cp.provisioning_command("browser-acorn")

    assert (
        proof_plan._provision_checkout_inputs(
            policies, (), root=checkout, environment=os.environ
        )
        is None
    )
    assert _runs(checkout) == [], "an unselected input is not provisioned"
    assert (
        proof_plan._provision_checkout_inputs(
            policies, ("acorn-tree",), root=checkout, environment=marked
        )
        is None
    )
    assert cp.input_state(spec) == "current"
    # Provisioning ran under the environment admission handed it.
    assert seen_environments
    assert all(env["MOLT_ADMISSION_MARK"] == "selected" for env in seen_environments)

    (checkout / "fixture.lock").write_text("fail\n", encoding="utf-8")
    monkeypatch.setattr(proof_plan, "toolchain_fingerprints", lambda *_a, **_k: {})
    with pytest.raises(
        ValueError, match="checkout input provisioning failed"
    ) as raised:
        proof_plan.admit_toolchains(
            policies, ("acorn-tree",), root=checkout, environ=os.environ
        )
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


@pytest.mark.parametrize("name", ["browser-acorn", "edgebox-typescript"])
def test_the_committed_node_policy_is_the_checkout_input(name: str) -> None:
    import tomllib

    from tools import proof_plan

    data = tomllib.loads(
        (REPO_ROOT / "tools/proof_plan.toml").read_text(encoding="utf-8")
    )
    policies = proof_plan.load_toolchain_policies(data, root=REPO_ROOT, names=(name,))
    policy = next(policy for policy in policies if policy.name == name)
    assert policy.data["setup_command"] == cp.provisioning_command(name)
    assert policy.data["package_lock"] in cp.locked_input_files(name)


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


# A pinned tool's entrypoint, run by its runtime: it records the first PATH
# entry its children would search and the arguments it received.
_PINNED_ENTRYPOINT = """\
import os
import sys
from pathlib import Path
if sys.argv[1:] == ["--version"]:
    print("9.9.9")
    raise SystemExit(0)
first = os.environ["PATH"].split(os.pathsep)[0]
Path("pinned-run.log").write_text(first + "\\n" + " ".join(sys.argv[1:]) + "\\n")
tree = Path("tree")
tree.mkdir(exist_ok=True)
(tree / "ready").write_text("ok")
"""


def test_a_pinned_tool_runs_on_its_runtime_never_from_path(
    checkout: Path, monkeypatch
) -> None:
    from molt import tool_releases

    entrypoint = checkout / "npm-cli.py"
    entrypoint.write_text(_PINNED_ENTRYPOINT, encoding="utf-8")
    manifest = checkout / "package.json"
    manifest.write_text(json.dumps({"packageManager": "npm@9.9.9"}), encoding="utf-8")
    runtime = Path(sys.executable)

    def installed(name: str, path: Path) -> tool_releases.ToolDiscovery:
        release = tool_releases.ToolRelease(
            name=name,
            version="9.9.9",
            executable=name,
            provenance=tool_releases.ToolProvenance(
                "checksum-manifest", "https://example.org/SHASUMS256.txt"
            ),
            assets={},
        )
        return tool_releases.ToolDiscovery(
            release=release,
            prefix=path.parent,
            executable=path,
            executable_sha256="0" * 64,
            asset=tool_releases.ToolAsset("any", "https://example.org/a", 1, "0" * 64),
        )

    command = tool_releases.PinnedCommand(
        installed("npm", entrypoint), installed("node", runtime)
    )
    requested = []

    def pinned(name, root, **kwargs):
        requested.append((name, kwargs["provision"]))
        return command

    monkeypatch.setattr(tool_releases, "pinned_command", pinned)
    monkeypatch.setattr(
        tool_releases, "tool_release", lambda name, root: command.tool.release
    )
    # No npm and no runtime on PATH: a PATH lookup could only fail.
    tree = dataclasses.replace(
        _tree(checkout),
        tool="npm",
        pinned=True,
        package_manifest="package.json",
        arguments=("ci",),
        environment=ProcessEnvironment(os.environ).updated({"PATH": "/usr/bin"}),
    )
    assert cp.ensure_input(tree)
    assert requested and all(entry == ("npm", True) for entry in requested)
    first, arguments = (
        (checkout / "pinned-run.log").read_text(encoding="utf-8").splitlines()
    )
    assert (first, arguments) == (str(runtime.parent), "ci")
    stamp = json.loads(tree.stamp.read_text(encoding="utf-8"))
    assert stamp["tool"] == {
        "name": "npm",
        "command": list(command.argv),
        "version": "9.9.9",
    }

    # A packageManager that names another npm is refused before anything runs.
    manifest.write_text(json.dumps({"packageManager": "npm@1.0.0"}), encoding="utf-8")
    (checkout / "fixture.lock").write_text("v2\n", encoding="utf-8")
    (checkout / "pinned-run.log").unlink()
    with pytest.raises(cp.ProvisioningError, match="pins npm@9.9.9"):
        cp.ensure_input(tree)
    assert not (checkout / "pinned-run.log").exists()
