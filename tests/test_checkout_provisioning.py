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

from molt import python_environment_contract as _contract
from molt import checkout_provisioning as cp
from molt.process_environment import ProcessEnvironment
from tests.process_guard_common import run_guarded_test_process
from tests.test_molt_dev import _commit_file, _git, _init_repo, kept_gate_stdout
from tests.python_environment_test_support import materialize_development_environment

REPO_ROOT = Path(__file__).resolve().parents[1]
DRIVER = REPO_ROOT / "tools" / "molt_dev.py"


def test_artifact_root_command_binds_reports_and_clears(
    checkout, tmp_path, monkeypatch, capsys
):
    from molt import disk_capacity, dx

    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("MOLT_PREFER_EXTERNAL_ARTIFACTS", raising=False)
    monkeypatch.delenv("MOLT_REQUIRE_EXTERNAL_ARTIFACTS", raising=False)
    monkeypatch.setenv("MOLT_EXTERNAL_MIN_FREE_GB", "20")
    monkeypatch.setattr(
        disk_capacity, "_default_measure_free_bytes", lambda _: 64 * 1024**3
    )
    artifacts = tmp_path / "artifacts"
    artifacts.mkdir()
    arguments = ["artifact-root", "--checkout", str(checkout), "--json"]
    assert cp.main([*arguments, "--bind", str(artifacts)]) == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload == {
        "root": str(artifacts.resolve()),
        "source": "family-binding",
        "binding_record": str(checkout / ".git" / "molt" / "artifact-root.v2.json"),
    }
    record = Path(payload["binding_record"])
    saved = json.loads(record.read_text(encoding="utf-8"))
    assert saved["root"] == str(artifacts.resolve())
    from molt.durable_file_identity import file_identity

    assert saved["schema"] == "molt.artifact-root-binding.v2"
    assert saved["identity"] == file_identity(artifacts).payload()
    # Explicit environment selection is visible without destroying the binding.
    override = tmp_path / "override"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(override))
    assert cp.main(arguments) == 0
    assert json.loads(capsys.readouterr().out)["source"] == "explicit-env"
    assert json.loads(record.read_text(encoding="utf-8")) == saved
    monkeypatch.delenv("MOLT_EXT_ROOT")
    # Clearing must work when the currently bound media is unavailable.
    artifacts.rename(tmp_path / "offline")
    assert cp.main(arguments) == 1
    assert "artifact-root --clear" in capsys.readouterr().err
    assert cp.main([*arguments, "--clear"]) == 0
    assert json.loads(capsys.readouterr().out)["source"] == "plain-clone-default"
    assert not record.exists()
    assert dx.configured_artifact_root({}, relative_to=checkout) is None
    assert cp.main([*arguments, "--clear"]) == 0


@pytest.mark.parametrize(
    "target",
    ["relative", "missing", "file", "checkout", "volume", "full", "unmeasurable"],
)
def test_artifact_root_command_rejects_invalid_target_without_mutating_binding(
    checkout, tmp_path, monkeypatch, capsys, target
):
    from molt import disk_capacity, dx

    monkeypatch.setenv("MOLT_EXTERNAL_MIN_FREE_GB", "20")
    monkeypatch.setattr(
        disk_capacity, "_default_measure_free_bytes", lambda _: 64 * 1024**3
    )
    healthy = tmp_path / "healthy"
    healthy.mkdir()
    record = dx.set_artifact_root_binding(checkout, healthy, {})
    before = record.read_bytes()
    candidate = tmp_path / "candidate"
    if target in {"full", "unmeasurable"}:
        candidate.mkdir()

        def measure(_):
            if target == "unmeasurable":
                raise OSError("fixture cannot measure")
            return 1

        monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", measure)
        monkeypatch.setattr(disk_capacity, "_darwin_important_usage_bytes", measure)
    elif target == "file":
        candidate.write_bytes(b"file")
    elif target == "relative":
        candidate = Path("relative-root")
    elif target == "checkout":
        candidate = checkout
    elif target == "volume":
        candidate = Path(checkout.anchor)
    assert (
        cp.main(
            ["artifact-root", "--checkout", str(checkout), "--bind", str(candidate)]
        )
        == 1
    )
    assert "cannot bind artifact root" in capsys.readouterr().err
    assert record.read_bytes() == before
    if target == "missing":
        assert not candidate.exists()


def test_artifact_root_command_requires_git_family(tmp_path, capsys):
    assert cp.main(["artifact-root", "--checkout", str(tmp_path), "--clear"]) == 1
    assert "requires a Git checkout family" in capsys.readouterr().err


def test_owner_launcher_carries_bytecode_policy_before_isolation(tmp_path):
    spec = cp.OwnedInstallation("owner", tmp_path, "owner.py", ProcessEnvironment({}))
    argv = cp._owner_argv(spec, "--check")
    script_index = argv.index(str(tmp_path / "owner.py"))
    child = run_guarded_test_process(
        [
            argv[0],
            *argv[1:script_index],
            "-c",
            "import json,sys;print(json.dumps([sys.flags.isolated,sys.dont_write_bytecode,sys.pycache_prefix]))",
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert child.returncode == 0, child.stderr
    assert json.loads(child.stdout) == [1, True, str(Path(sys.prefix) / "bytecode")]


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


def test_development_selection_metadata_is_available_without_site_packages(tmp_path):
    import hashlib

    checkout = tmp_path / "checkout"
    checkout.mkdir()
    contents = {
        "pyproject.toml": b'[project]\nname = "MOLT._-Fixture"\nversion = "1.2.3"\nreadme = "README.md"\n',
        "README.md": b"owned metadata\n",
        "uv.lock": b"version = 1\n",
        ".python-version": b"3.12\n",
    }
    for name, data in contents.items():
        (checkout / name).write_bytes(data)
    code = (
        "import json, sys; from pathlib import Path; "
        "sys.path[:0] = [sys.argv[1], str(Path(sys.argv[1]) / 'src')]; "
        "from tools.import_file import bind_repository_imports; "
        "bind_repository_imports(Path(sys.argv[1]) / 'tools/molt_dev.py'); "
        "from molt import checkout_provisioning as cp; "
        "from molt.source_build_environment_selection import capture_source_build_inputs; "
        "root = Path(sys.argv[2]); "
        "spec = cp.checkout_input('uv-environment', root, {}); "
        "print(json.dumps({'binding': capture_source_build_inputs(root, editable=True).editable_binding, 'locks': cp._lock_digests(spec)}))"
    )
    result = run_guarded_test_process(
        [sys.executable, "-I", "-S", "-B", "-c", code, str(REPO_ROOT), str(checkout)],
        cwd=tmp_path,
        env=ProcessEnvironment(os.environ),
        timeout=30,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    observed = json.loads(result.stdout)
    assert observed["binding"] == {
        "source_root": str(checkout),
        "project": "molt-fixture",  # PyPA name normalization, independent of the reader.
        "version": "1.2.3",
        "metadata_inputs": {
            name: hashlib.sha256(contents[name]).hexdigest()
            for name in ("pyproject.toml", "README.md")
        },
    }
    assert observed["locks"] == {
        name: hashlib.sha256(data).hexdigest() for name, data in contents.items()
    }
    assert not list(checkout.rglob("*.pyc"))


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
    assert isinstance(uv, cp.DevelopmentEnvironment)
    assert uv.lock_files == ("uv.lock", "pyproject.toml", ".python-version")
    # Explicit UV selection names a published image, never an install destination.
    named = cp.checkout_input(
        "uv-environment", tmp_path, {**environ, "UV_PROJECT_ENVIRONMENT": "envs/a"}
    )
    assert named.environment["UV_PROJECT_ENVIRONMENT"] == "envs/a"
    assert not cp.declares(uv)
    (tmp_path / "uv.lock").write_text("", encoding="utf-8")
    assert cp.declares(uv)


@pytest.mark.parametrize("editable", [False, True], ids=["unpublished", "published"])
@pytest.mark.parametrize("named", [False, True], ids=["projected", "explicit"])
def test_uv_environment_selects_published_images_and_refuses_mutable_roots(
    tmp_path: Path, named: bool, editable: bool
) -> None:
    root, artifacts = tmp_path / "project", tmp_path / "artifacts"
    _init_repo(root)
    (root / "README.md").write_text("declared editable metadata\n", encoding="utf-8")
    (root / "pyproject.toml").write_text(
        "[project]\nname='selector-fixture'\nversion='1.0'\n"
        "requires-python='>=3.12'\ndependencies=[]\nreadme='README.md'\n"
        "[dependency-groups]\n[tool.molt.dx]\ndependency_groups=[]\n",
        encoding="utf-8",
    )
    image = materialize_development_environment(root, artifacts, select=True)
    environ = (
        ProcessEnvironment(os.environ)
        .without(
            (
                "MOLT_VENV",
                "UV_PROJECT_ENVIRONMENT",
                "VIRTUAL_ENV",
                "MOLT_UV_PROJECT_PYTHON",
                "MOLT_UV_PROJECT_PURPOSE",
            )
        )
        .updated({"MOLT_EXT_ROOT": str(artifacts)})
    )
    if not editable:
        old = root / ".venv"
        old.mkdir()
        (old / "pyvenv.cfg").write_text(
            "home = old mutable environment\n", encoding="utf-8"
        )
        before = (old / "pyvenv.cfg").read_bytes()
        with pytest.raises(cp.ProvisioningError):
            cp.ensure(
                "uv-environment",
                root,
                environ.updated({"UV_PROJECT_ENVIRONMENT": str(old)}),
            )
        assert (old / "pyvenv.cfg").read_bytes() == before
        assert not (old / cp.STAMP_NAME).exists()
        return
    if named:
        environ = environ.updated({"UV_PROJECT_ENVIRONMENT": str(image.root)})
    before = {
        path.relative_to(image.root): path.read_bytes()
        for path in image.root.rglob("*")
        if path.is_file()
    }
    selected = cp.ensure("uv-environment", root, environ)
    assert not selected.provisioned and selected.environment_root == image.root
    assert (
        cp.input_state(cp.checkout_input("uv-environment", root, environ)) == "current"
    )
    assert not (image.root / cp.STAMP_NAME).exists(), "selection metadata is external"
    assert not (root / ".venv").exists()
    for name in ("uv.lock", "README.md"):
        metadata = root / name
        original = metadata.read_bytes()
        changed = (
            original.replace(b"requires-python='>=3.12'", b"requires-python='>=99'")
            if name == "uv.lock"
            else original + b"\n"
        )
        assert changed != original
        metadata.write_bytes(changed)
        spec = cp.checkout_input("uv-environment", root, environ)
        if named:
            with pytest.raises(cp.ProvisioningError):
                cp.input_state(spec)
        else:
            assert cp.input_state(spec) == "absent", (
                f"{name} must invalidate the old selection"
            )
        metadata.write_bytes(original)
    other = tmp_path / "other"
    _init_repo(other)
    for name in ("pyproject.toml", "uv.lock", ".python-version"):
        shutil.copyfile(root / name, other / name)
    with pytest.raises(cp.ProvisioningError, match="checkout generation namespace"):
        cp.ensure(
            "uv-environment",
            other,
            environ.updated({"UV_PROJECT_ENVIRONMENT": str(image.root)}),
        )
    from tools.molt_dev import _resolve_repo_venv

    assert (
        _resolve_repo_venv(other, environ.updated({"MOLT_VENV": str(image.root)}))
        == image.root
    )
    assert {
        path.relative_to(image.root): path.read_bytes()
        for path in image.root.rglob("*")
        if path.is_file()
    } == before
    manifest = image.manifest_path.read_bytes()
    altered = json.loads(manifest)
    altered["environment_id"] = "0" * 64
    image.manifest_path.write_text(json.dumps(altered), encoding="utf-8")
    corrupt = image.manifest_path.read_bytes()
    assert not cp.ensure("uv-environment", root, environ).provisioned
    with pytest.raises(_contract.SourceBuildEnvironmentError, match="address/recipe"):
        from molt.source_build_environment import load_development_environment

        load_development_environment(image.root, repo_root=root)
    assert image.manifest_path.read_bytes() == corrupt, (
        "published corruption is never repaired"
    )
    image.manifest_path.write_bytes(manifest)


@pytest.mark.parametrize("record_kind", ["absent", "malformed", "dangling-link"])
def test_development_selection_distinguishes_new_intent_from_invalid_record(
    tmp_path: Path,
    record_kind: str,
) -> None:
    root, artifacts = tmp_path / "project", tmp_path / "artifacts"
    _init_repo(root)
    image = materialize_development_environment(root, artifacts, select=True)
    environ = ProcessEnvironment({"MOLT_EXT_ROOT": str(artifacts)})
    spec = cp.checkout_input("uv-environment", root, environ)
    assert isinstance(spec, cp.DevelopmentEnvironment)
    selection = cp._development_selection_path(spec, cp._development_intent(spec))
    selection.unlink()
    if record_kind == "malformed":
        selection.write_text("{not JSON}", encoding="utf-8")
    elif record_kind == "dangling-link":
        try:
            selection.symlink_to(tmp_path / "absent-target")
        except OSError:
            pytest.skip("file symlinks unavailable")
    image_bytes = image.manifest_path.read_bytes()
    if record_kind == "absent":
        assert cp.selected_development_environment(root, environ) is None
        assert not selection.exists()
    else:
        with pytest.raises(cp.ProvisioningError, match="invalid selection"):
            cp.selected_development_environment(root, environ)
    assert image.manifest_path.read_bytes() == image_bytes


def test_development_contract_upgrade_selects_new_intent_without_repairing_old_image(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import source_build_environment as publisher

    root, artifacts = tmp_path / "project", tmp_path / "artifacts"
    _init_repo(root)
    old = materialize_development_environment(root, artifacts, select=True)
    environment = ProcessEnvironment({"MOLT_EXT_ROOT": str(artifacts)})
    spec = cp.checkout_input("uv-environment", root, environment)
    assert isinstance(spec, cp.DevelopmentEnvironment)
    old_stamp = cp._development_selection_path(spec, cp._development_intent(spec))
    old_stamp_bytes = old_stamp.read_bytes()
    old_manifest_bytes = old.manifest_path.read_bytes()

    monkeypatch.setattr(
        _contract,
        "SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION",
        _contract.SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION + 1,
    )
    new_stamp = cp._development_selection_path(spec, cp._development_intent(spec))
    assert new_stamp != old_stamp and not new_stamp.exists()
    new = materialize_development_environment(root, artifacts)
    assert new.root != old.root
    calls = []

    def publish(checkout, *, provision, **options):
        assert checkout == root.resolve() and provision is True
        assert not options["environ"].get("UV_PROJECT_ENVIRONMENT")
        calls.append(checkout)
        return new

    from molt import source_build_environment_selection as selection

    load = selection.read_development_environment_selection
    loaded = []

    def track_load(path, **options):
        loaded.append(path)
        return load(path, **options)

    monkeypatch.setattr(publisher, "development_environment", publish)
    monkeypatch.setattr(selection, "read_development_environment_selection", track_load)
    assert cp.ensure("uv-environment", root, environment).environment_root == new.root
    assert old.root not in loaded
    assert new_stamp.is_file()
    assert cp.ensure("uv-environment", root, environment).environment_root == new.root
    assert calls == [root.resolve()] and loaded == [new.root]

    # An exact caller selection is never converted into default intent or
    # an installation request, even when its producer contract is retired.
    with pytest.raises(cp.ProvisioningError):
        cp.ensure(
            "uv-environment",
            root,
            environment.updated({"UV_PROJECT_ENVIRONMENT": str(old.root)}),
        )
    assert calls == [root.resolve()]
    assert loaded == [new.root, old.root]
    assert old_stamp.read_bytes() == old_stamp_bytes
    assert old.manifest_path.read_bytes() == old_manifest_bytes


@pytest.mark.parametrize("api", ["input", "single", "all"])
def test_runtime_refresh_reselects_changed_runtime_without_rewriting_old_image(
    tmp_path, monkeypatch, api
):
    from molt import source_build_environment as publisher
    from molt.exact_json import canonical_json_sha256
    from molt.file_locks import _release_file_lock, _try_acquire_file_lock
    from tests import python_environment_test_support as fixtures

    root, artifacts = tmp_path / "project", tmp_path / "artifacts"
    _init_repo(root)
    old = materialize_development_environment(root, artifacts, select=True)
    environment = ProcessEnvironment({"MOLT_EXT_ROOT": str(artifacts)})
    spec = cp.checkout_input("uv-environment", root, environment)
    assert isinstance(spec, cp.DevelopmentEnvironment)
    selection = cp._development_selection_path(spec, cp._development_intent(spec))
    old_manifest = old.manifest_path.read_bytes()
    old_selection = selection.read_bytes()
    runtime = fixtures.runtime_identity_manifest

    def changed_runtime(**options):
        payload = runtime(**options)
        # Same version selector, independently changed runtime content identity.
        payload["file_nodes"][0]["sha256"] = "e" * 64
        payload.pop("runtime_closure_sha256")
        payload["runtime_closure_sha256"] = canonical_json_sha256(payload)
        return payload

    monkeypatch.setattr(fixtures, "runtime_identity_manifest", changed_runtime)
    published = []

    def publish(checkout, *, provision, **options):
        assert checkout == root.resolve() and provision is True
        assert not options["environ"].get("UV_PROJECT_ENVIRONMENT")
        handle = _try_acquire_file_lock(cp._lock_path(spec))
        if handle is not None:
            _release_file_lock(handle)
        assert handle is None, "runtime refresh left the provisioning owner"
        image = materialize_development_environment(root, artifacts)
        published.append(image.root)
        return image

    monkeypatch.setattr(publisher, "development_environment", publish)
    assert cp.ensure_input(spec).environment_root == old.root
    assert published == [] and selection.read_bytes() == old_selection
    if api == "input":
        refreshed = cp.ensure_input(spec, refresh_runtime=True)
    elif api == "single":
        refreshed = cp.ensure("uv-environment", root, environment, refresh_runtime=True)
    else:
        (refreshed,) = cp.ensure_all(
            root, environment, names=iter(("uv-environment",)), refresh_runtime=True
        )
    assert published == [refreshed.environment_root]
    assert refreshed.environment_root != old.root
    assert json.loads(selection.read_bytes())["generation"] == str(
        refreshed.environment_root
    )
    assert old.manifest_path.read_bytes() == old_manifest
    assert cp.ensure_input(spec).environment_root == refreshed.environment_root
    assert len(published) == 1
    exact = dataclasses.replace(
        spec,
        environment=environment.updated({"UV_PROJECT_ENVIRONMENT": str(old.root)}),
    )
    with pytest.raises(cp.ProvisioningError, match="explicit UV_PROJECT_ENVIRONMENT"):
        cp.ensure_input(exact, refresh_runtime=True)
    assert len(published) == 1 and old.manifest_path.read_bytes() == old_manifest


def test_runtime_refresh_all_consumes_names_once_and_only_refreshes_uv(
    tmp_path, monkeypatch
):
    calls = []

    def ensure(name, *_args, refresh_runtime=False, **_options):
        calls.append((name, refresh_runtime))
        return cp.Ensured(name, False, 0)

    monkeypatch.setattr(cp, "ensure", ensure)
    names = iter(("git-hooks", "uv-environment", "browser-acorn"))
    cp.ensure_all(tmp_path, {}, names=names, refresh_runtime=True)
    assert calls == [
        ("git-hooks", False),
        ("uv-environment", True),
        ("browser-acorn", False),
    ]
    calls.clear()
    with pytest.raises(cp.ProvisioningError, match="requires uv-environment"):
        cp.ensure_all(tmp_path, {}, names=iter(("git-hooks",)), refresh_runtime=True)
    with pytest.raises(cp.ProvisioningError, match="explicit UV_PROJECT_ENVIRONMENT"):
        cp.ensure_all(
            tmp_path, {"UV_PROJECT_ENVIRONMENT": "exact-image"}, refresh_runtime=True
        )
    assert calls == []


def test_runtime_refresh_refuses_other_input_before_provisioning(checkout):
    with pytest.raises(cp.ProvisioningError, match="requires uv-environment"):
        cp.ensure_input(_tree(checkout), refresh_runtime=True)
    assert _runs(checkout) == []


def test_runtime_refresh_cli_routes_and_rejects_invalid_modes(tmp_path, monkeypatch):
    calls = []

    def ensure_all(checkout, *, names, refresh_runtime=False):
        calls.append((checkout, names, refresh_runtime))
        return ()

    monkeypatch.setattr(cp, "ensure_all", ensure_all)
    assert (
        cp.main(
            [
                "ensure",
                "uv-environment",
                "--refresh-runtime",
                "--checkout",
                str(tmp_path),
            ]
        )
        == 0
    )
    assert calls == [(tmp_path, ("uv-environment",), True)]
    for arguments in (
        ["status", "uv-environment", "--refresh-runtime"],
        ["ensure", "git-hooks", "--refresh-runtime"],
    ):
        with pytest.raises(SystemExit) as raised:
            cp.main([*arguments, "--checkout", str(tmp_path)])
        assert raised.value.code == 2
    assert len(calls) == 1


def test_implicit_evicted_generation_republishes_through_existing_owner(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import source_build_environment as publisher

    root, artifacts = tmp_path / "project", tmp_path / "artifacts"
    _init_repo(root)
    image = materialize_development_environment(root, artifacts, select=True)
    environment = ProcessEnvironment({"MOLT_EXT_ROOT": str(artifacts)})
    spec = cp.checkout_input("uv-environment", root, environment)
    assert isinstance(spec, cp.DevelopmentEnvironment)
    selection = cp._development_selection_path(spec, cp._development_intent(spec))
    selection_bytes = selection.read_bytes()
    shutil.rmtree(image.root)
    assert selection.read_bytes() == selection_bytes
    assert cp.selected_development_environment(root, environment) == image.root
    assert cp.input_state(spec) == "absent"
    calls = []

    def publish(checkout, *, provision, **options):
        assert checkout == root.resolve() and provision is True
        assert not options["environ"].get("UV_PROJECT_ENVIRONMENT")
        calls.append(checkout)
        return materialize_development_environment(root, artifacts)

    monkeypatch.setattr(publisher, "development_environment", publish)
    assert cp.ensure_input(spec).environment_root == image.root
    assert image.manifest_path.is_file()
    assert cp.input_state(spec) == "current"
    assert not cp.ensure_input(spec).provisioned
    assert calls == [root.resolve()]


@pytest.mark.parametrize(
    "fault",
    [
        "explicit-absent",
        "missing-manifest",
        "missing-projection",
        "malformed-projection",
        "dangling-root",
        "dangling-ancestor",
        "foreign-namespace",
        "foreign-owner",
        "invalid-address",
        "permission",
    ],
)
def test_generation_recovery_keeps_exact_and_invalid_selections_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fault: str
) -> None:
    from molt import source_build_environment as publisher

    root, artifacts = tmp_path / "project", tmp_path / "artifacts"
    _init_repo(root)
    image = materialize_development_environment(root, artifacts, select=True)
    environment = ProcessEnvironment({"MOLT_EXT_ROOT": str(artifacts)})
    spec = cp.checkout_input("uv-environment", root, environment)
    assert isinstance(spec, cp.DevelopmentEnvironment)
    selection = cp._development_selection_path(spec, cp._development_intent(spec))
    if fault == "explicit-absent":
        shutil.rmtree(image.root)
        spec = dataclasses.replace(
            spec,
            environment=environment.updated(
                {"UV_PROJECT_ENVIRONMENT": str(image.root)}
            ),
        )
    elif fault == "missing-manifest":
        image.manifest_path.unlink()
    elif fault == "missing-projection":
        (image.root / _contract.DEVELOPMENT_ENVIRONMENT_SELECTION).unlink()
    elif fault == "malformed-projection":
        (image.root / _contract.DEVELOPMENT_ENVIRONMENT_SELECTION).write_text(
            "{not JSON}", encoding="utf-8"
        )
    elif fault in {"foreign-namespace", "foreign-owner", "invalid-address"}:
        record = json.loads(selection.read_bytes())
        missing = {
            "foreign-namespace": tmp_path / "foreign" / image.root.name,
            "foreign-owner": (
                image.root.parent.parent / "other-0123456789ab" / image.root.name
            ),
            "invalid-address": image.root.parent / "not-a-recipe",
        }[fault]
        record["generation"] = str(missing)
        selection.write_text(json.dumps(record), encoding="utf-8")
    elif fault in {"dangling-root", "dangling-ancestor"}:
        target = image.root if fault == "dangling-root" else image.root.parent
        target.rename(tmp_path / "preserved-generation")
        try:
            target.symlink_to(tmp_path / "missing-target", target_is_directory=True)
        except OSError:
            pytest.skip("directory symlinks unavailable")
    else:
        lstat = Path.lstat

        def denied(path, *args, **kwargs):
            if path == image.root:
                raise PermissionError("fixture denied generation observation")
            return lstat(path, *args, **kwargs)

        monkeypatch.setattr(Path, "lstat", denied)
    selection_bytes = selection.read_bytes()

    def no_publish(*args, **kwargs):
        pytest.fail("an exact or invalid generation must never enter publication")

    monkeypatch.setattr(publisher, "development_environment", no_publish)
    with pytest.raises(cp.ProvisioningError):
        cp.ensure_input(spec)
    assert selection.read_bytes() == selection_bytes


def test_integrate_child_refuses_a_missing_inherited_uv_environment(
    tmp_path: Path,
) -> None:
    """A guarded child inherits an UV_PROJECT_ENVIRONMENT nobody built."""
    seed = tmp_path / "seed"
    _init_repo(seed)
    materialize_development_environment(seed, tmp_path / "fixture-images")
    gate = (
        'python -c "from pathlib import Path; import sys; '
        "Path('gate-launched.txt').write_text('launched', encoding='utf-8'); "
        "print('gate-prefix=' + sys.prefix)\""
    )
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
    assert result.returncode != 0, result.stdout + result.stderr
    assert "managed development image refused" in result.stdout + result.stderr
    # Preparation refuses the exact selector before gate scheduling. No child
    # marker or output stream may exist, rather than one empty finished gate.
    events = [
        json.loads(line)
        for line in result.stdout.splitlines()
        if line.startswith('{"schema": "molt.integration-progress.v1"')
    ]
    assert any(
        event["event"] == "candidate_selected" and event["gate_total"] == 1
        for event in events
    )
    assert not any(
        event["event"] in {"gate_started", "gate_finished"} for event in events
    )
    assert kept_gate_stdout(result.stdout) == []
    assert not (work / "gate-launched.txt").exists()
    assert not list((work / ".git/molt-integrate/receipts").rglob("*.stdout.log"))
    assert not managed.exists(), (
        "an exact unpublished request must not become an install destination"
    )


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
            policies,
            ("acorn-tree",),
            root=checkout,
            environ=os.environ,
            python_executable=Path(sys.executable),
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
