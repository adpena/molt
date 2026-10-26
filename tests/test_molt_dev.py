"""Failure-mode tests for the canonical integration driver (tools/molt_dev.py).

Driver-owned countermeasures are exercised on synthetic git repositories
with independent failure controls. Compiler source/content admission belongs
to the existing CLI artifact-custody tests, not a timestamp predicate here. The repos
are real git repositories with a bare "origin" remote, so the fetch / rebase /
push / ancestor / patch-id flows are exercised end-to-end, not mocked.

Hazard -> proving test(s):
  1 rebase drops commits        test_integrate_detects_dropped_commit
                                test_integrate_accepts_already_upstream_by_patch_id
  2 push exit codes lie         test_verify_push_catches_non_landed_tip
                                test_verify_push_confirms_landed_tip
  3 cleanup loses work          test_cleanup_refuses_unpushed
                                test_cleanup_refuses_dirty_tracked
                                test_cleanup_allows_clean_pushed
                                test_cleanup_force_requires_matching_sha
  4 partial WIP salvage         test_secure_wip_captures_staged_and_unstaged
                                test_secure_wip_honors_ignore_set
                                test_secure_wip_excludes_untracked_by_default
  5 diff/ls/stat lie            (verdicts use plumbing/python throughout)
  6 stale-binary misattribution CLI compiler content-admission tests; selected
                                gate failures still prevent integration
  7 .venv interpreter flips     test_python_oracle_pins_current_version
                                test_python_oracle_refuses_unavailable_version
  8 content-marker verify       test_integrate_marker_mismatch_fails
                                test_integrate_marker_match_passes
  9 liveness/recovery probes    test_probe_file_and_pid
                                test_probe_missing_file_is_nonzero
 10 gate selection by class     test_gate_manifest_selection_backend
                                test_gate_manifest_selection_python
                                test_gate_manifest_always_runs
                                test_committed_gate_manifest_is_valid
                                test_integrate_runs_selected_gate_and_halts_on_failure
 11 backgrounded runs die       test_detached_run_completes_and_records_rc
    silently                    test_detached_run_nonzero_rc_is_loud
                                test_detached_daemon_survives_spawner_and_runs_in_new_session
                                test_detached_run_refuses_live_duplicate_and_never_kills
                                test_detached_verify_detects_died_silent
                                test_detached_verify_too_young_is_not_trusted
                                test_detached_run_missing_command_is_usage_error
                                test_detached_run_exec_failure_records_sentinel_rc
                                test_land_wait_fails_when_its_runner_supervisor_dies_without_rc
"""

from __future__ import annotations

import json
import re
import shlex
import shutil
import os
import sys
import sysconfig
import subprocess
import textwrap
import time
import venv
from pathlib import Path
from tests.process_guard_common import (
    HUNG_CHILD_SOURCE,
    guard_custody_env,
    install_module_os_view,
    install_thread_module_view,
    hung_child_marker,
    processes_carrying,
    run_guarded_test_process,
    stop_test_processes_carrying,
)

import pytest

from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder

REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = REPO_ROOT / "tools"


from tools import harness_memory_guard, memory_guard, molt_dev_gates  # noqa: E402
from tools import dirty_tree_policy, molt_dev, molt_dev_retire  # noqa: E402
from tools.molt_dev_policy import (  # noqa: E402
    GateConfig,
    GateEnvironment,
    PreparedGates,
    SelectedGate,
)
from tools.molt_dev_common import (  # noqa: E402
    CandidateFailure,
    GateFailure,
    _run_fast_captured_command,
)
from tests.integration_driver_fixture import (  # noqa: E402
    clone_tree,
    copy_driver_trees,
    isolated_checkout_artifacts as isolated_checkout_artifacts,
)

SCRIPT_PATH = REPO_ROOT / "tools" / "molt_dev.py"
RUNTIME_GENERATION_MANIFEST = "wasm/molt_runtime.generation.json"
CUSTOM_IGNORED_PATH = "tmp/operator-scratch.txt"


def _gate_outcome(rc, stdout, stderr, peak=None, **guard_kwargs):
    """Supply the real guard boundary, including explicit terminal closure."""
    fields = {
        "args": ["synthetic-gate"],
        "returncode": rc,
        "stdout": stdout,
        "stderr": stderr,
        "elapsed_s": 0.1,
        "child_stderr": stderr,
        "descendants_closed": True,
        "peak_total": (
            None
            if peak is None
            else memory_guard.RssViolation(0, peak // 1024, "synthetic-gate", "tree")
        ),
    }
    fields.update(guard_kwargs)
    return molt_dev_gates.GateOutcome(
        harness_memory_guard.GuardedCompletedProcess(**fields)
    )


def _captured_gate(fake):
    """Adapt an exit-code gate fake to the captured runner every gate uses."""

    def run(command, repo, env, cancelled, streams=None):
        del cancelled, streams
        return _gate_outcome(fake(command, repo, env), "", "")

    return run


def _writing_gate(outcomes):
    """A captured-gate fake that writes each stream to its file, as the
    guarded child does, and returns the outcome."""

    def run(command, repo, env, cancelled, streams=None):
        outcome = outcomes[command]
        if streams is not None:
            streams[0].write_text(outcome.stdout, encoding="utf-8")
            streams[1].write_text(outcome.stderr, encoding="utf-8")
        return outcome

    return run


@pytest.fixture
def drv():
    return molt_dev


@pytest.mark.parametrize("command", ["fix", "difftest", "probe", "detached-run"])
def test_driver_dispatcher_leaves_maintenance_to_selection_owner(
    drv, monkeypatch, command
):
    from types import SimpleNamespace
    from molt import dx

    events = []

    def completed(_args):
        events.append("returned")
        return 7

    args = SimpleNamespace(command=command, func=completed, dry_run=False)
    parser = SimpleNamespace(parse_args=lambda _argv: args)
    monkeypatch.setattr(drv, "build_parser", lambda: parser)
    monkeypatch.setattr(
        drv.molt_dev_controller, "resume_controller_cwd", lambda *_a, **_k: None
    )
    monkeypatch.setattr(
        drv.molt_dev_controller, "runs_from_pinned_controller", lambda _a: False
    )

    def collect(_env, *, cwd):
        pytest.fail("the dispatcher has no captured command selection")

    monkeypatch.setattr(dx, "collect_checkout_state_after_result", collect)
    assert drv.main([]) == 7
    assert events == ["returned"]


def test_completed_driver_failure_publishes_before_selected_maintenance(
    drv, monkeypatch, tmp_path, capsys
):
    from types import SimpleNamespace
    from molt import dx

    selected = ProcessEnvironment({"MOLT_EXT_ROOT": str(tmp_path / "selected")})
    monkeypatch.setenv("MOLT_EXT_ROOT", str(tmp_path / "ambient"))

    def completed(_args):
        raise drv.DriverError(
            "published failed differential",
            code=7,
            maintenance_selection=(tmp_path, selected),
        )

    args = SimpleNamespace(command="difftest", func=completed)
    monkeypatch.setattr(
        drv, "build_parser", lambda: SimpleNamespace(parse_args=lambda _argv: args)
    )
    monkeypatch.setattr(
        drv.molt_dev_controller, "resume_controller_cwd", lambda *_a, **_k: None
    )
    monkeypatch.setattr(
        drv.molt_dev_controller, "runs_from_pinned_controller", lambda _a: False
    )
    observed = []

    def collect(env, *, cwd):
        assert "published failed differential" in capsys.readouterr().err
        assert env is selected and cwd == tmp_path
        observed.append("published")

    monkeypatch.setattr(dx, "collect_checkout_state_after_result", collect)
    assert drv.main([]) == 7
    assert observed == ["published"]


def test_integrate_maintenance_retains_cwd_before_command_changes_it(
    drv, origin_and_clone, tmp_path, monkeypatch, capsys
):
    from molt import dx

    _, work = origin_and_clone
    selected = ProcessEnvironment({"MOLT_EXT_ROOT": "selected"})
    duplicate = ProcessEnvironment({"MOLT_EXT_ROOT": str(work / "selected")})
    other = ProcessEnvironment({"MOLT_EXT_ROOT": "other"})
    monkeypatch.chdir(work.parent)
    monkeypatch.setattr(
        drv.molt_dev_controller, "integration_controller", lambda _a: "fixture"
    )

    def exclusive(_args, *, progress):
        progress.environments = {
            (): GateEnvironment(selected, None, False),
            ("duplicate",): GateEnvironment(duplicate, None, False),
            ("other",): GateEnvironment(other, None, False),
        }
        monkeypatch.chdir(tmp_path)
        return 7

    monkeypatch.setattr(drv, "_integrate_exclusive", exclusive)
    observed = []

    def collect(env, *, cwd):
        output = capsys.readouterr().out
        if not observed:
            terminal = json.loads(output.splitlines()[-1])
            assert terminal["event"] == "integration_finished"
            assert terminal["returncode"] == 7
        else:
            assert output == ""
        assert cwd == work and Path.cwd() == tmp_path
        observed.append(env)

    monkeypatch.setattr(dx, "collect_checkout_state_after_result", collect)
    args = _integrate_ns(drv, repo=work.name)
    assert drv.cmd_integrate(args) == 7
    assert observed == [selected, other]


# --------------------------------------------------------------------------
# git repo scaffolding (real repos, bare origin) — exercises the real flows
# --------------------------------------------------------------------------


def _git(repo: Path, *args: str, check: bool = True, input_text: str | None = None):
    env = ProcessEnvironment(os.environ).updated({"GIT_TERMINAL_PROMPT": "0"})
    proc = _run_fast_captured_command(
        ["git", "-C", str(repo), *args],
        cwd=REPO_ROOT,
        env=env,
        input_text=input_text,
    )
    if check and proc.returncode != 0:
        raise AssertionError(
            f"git {' '.join(args)} failed ({proc.returncode}): {proc.stderr}"
        )
    return proc


def _record_driver_git_commands(drv, monkeypatch):
    commands = []
    original = drv.Git._run

    def record(self, args, **kwargs):
        commands.append(list(args))
        return original(self, args, **kwargs)

    monkeypatch.setattr(drv.Git, "_run", record)
    return commands


def _record_process_deadlines(monkeypatch):
    """Record each command's deadline where driver commands reach a process.

    Every executor is a CommandExecutor, and the driver's guarded commands go
    through harness_memory_guard; a module that bound its executor at import
    is still recorded.
    """
    from tools import command_execution, harness_memory_guard

    deadlines: list[tuple[tuple[str, ...], float | None]] = []
    real_run = command_execution.CommandExecutor.run

    def run(self, args, **kwargs):
        deadlines.append((tuple(str(part) for part in args), kwargs.get("timeout")))
        return real_run(self, args, **kwargs)

    real_guarded = harness_memory_guard.guarded_completed_process

    def guarded(cmd, **kwargs):
        deadlines.append((tuple(str(part) for part in cmd), kwargs.get("timeout")))
        return real_guarded(cmd, **kwargs)

    monkeypatch.setattr(command_execution.CommandExecutor, "run", run)
    monkeypatch.setattr(harness_memory_guard, "guarded_completed_process", guarded)
    return deadlines


def _assert_no_patch_diff_work(commands):
    assert not any(args[0] == "patch-id" for args in commands)
    assert not any(args[0] == "diff-tree" and "-p" in args for args in commands)


def _init_repo(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    _git(path, "init", "-q", "-b", "main")
    _git(path, "config", "user.email", "dev@molt.test")
    _git(path, "config", "user.name", "Molt Dev")
    # Deterministic identity so committer epochs are stable in tests.
    _git(path, "config", "commit.gpgsign", "false")


def _commit_file(repo: Path, rel: str, content: str, msg: str) -> str:
    target = repo / rel
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")
    _git(repo, "add", "--", rel)
    _git(repo, "commit", "-q", "--no-verify", "-m", msg)
    return _git(repo, "rev-parse", "HEAD").stdout.strip()


@pytest.fixture(scope="module")
def _origin_seed(tmp_path_factory):
    """Provision immutable starting history once; every test gets its own copy."""
    root = tmp_path_factory.mktemp("integration-seed")
    seed = root / "seed"
    _init_repo(seed)
    (seed / "tools").mkdir()
    (seed / "tools/proof_plan.toml").write_text(
        'always = ["echo integration-policy"]\n', encoding="utf-8"
    )
    _git(seed, "add", "tools/proof_plan.toml")
    _commit_file(seed, "README.md", "seed\n", "seed: initial commit")
    origin = root / "origin.git"
    _git(root, "clone", "--bare", "-q", str(seed), str(origin))
    return origin


def _clone_origin(tmp_path, seed: Path, monkeypatch) -> tuple[Path, Path]:
    # These repositories exercise generic shell gates, not the caller's venv.
    # Interpreter-selection tests construct and select their own environments.
    for key in (
        "MOLT_VENV",
        "UV_PROJECT_ENVIRONMENT",
        "VIRTUAL_ENV",
        "MOLT_UV_PROJECT_PYTHON",
        "MOLT_UV_PROJECT_PURPOSE",
    ):
        monkeypatch.delenv(key, raising=False)
    origin = tmp_path / "origin.git"
    clone_tree(seed, origin)
    work = tmp_path / "work"
    _git(
        tmp_path,
        "clone",
        "-q",
        "-c",
        "user.email=dev@molt.test",
        "-c",
        "user.name=Molt Dev",
        str(origin),
        str(work),
    )
    return origin, work


@pytest.fixture
def origin_and_clone(tmp_path, _origin_seed, monkeypatch, isolated_checkout_artifacts):
    """Independent real bare origin and working clone for each integration.

    Copy files, never hardlink them: tests mutate refs, hooks, config and objects.
    Subject fetch/rebase/push operations and clone creation still use real Git.
    """
    return _clone_origin(tmp_path, _origin_seed, monkeypatch)


@pytest.fixture
def borrowed_gate_python(origin_and_clone, monkeypatch):
    """Host-tool gates explicitly borrow the interpreter named in their argv."""
    monkeypatch.setenv("MOLT_VENV", sys.prefix)


def _build_driver_origin(root: Path, driver: Path) -> None:
    """The seed history, with this checkout's driver trees committed on main."""
    seed = root / "seed"
    _init_repo(seed)
    copy_driver_trees(seed, seed=driver)
    (seed / "tools/proof_plan.toml").write_text(
        'always = ["echo integration-policy"]\n', encoding="utf-8"
    )
    (seed / "README.md").write_text("seed\n", encoding="utf-8")
    _git(seed, "add", "-A", "--", ".")
    _git(seed, "commit", "-q", "--no-verify", "-m", "seed: main driver")
    _git(root, "clone", "--bare", "-q", str(seed), str(root / "origin.git"))


@pytest.fixture(scope="session")
def _driver_origin_seed(tmp_path_factory, worker_id, driver_seed):
    """A bare origin whose main carries the driver, built once per session.

    A landing runs main's pinned driver, so a test that lands through it needs
    the driver committed on main. Each such test copied the driver trees
    (1,088 files, 25 MB) into its clone and committed them; the session now
    makes that commit once and each test clones it.
    """
    from tests.integration_driver_fixture import shared_session_tree

    root = shared_session_tree(
        tmp_path_factory,
        worker_id,
        "driver-origin",
        lambda root: _build_driver_origin(root, driver_seed),
    )
    return root / "origin.git"


@pytest.fixture
def driver_origin_and_clone(
    tmp_path, _driver_origin_seed, monkeypatch, isolated_checkout_artifacts
):
    """`origin_and_clone` whose main already carries this checkout's driver."""
    return _clone_origin(tmp_path, _driver_origin_seed, monkeypatch)


def test_driver_seed_keeps_runtime_dependencies_without_optional_groups(
    driver_seed: Path,
) -> None:
    import tomllib

    from molt.dx import DxProject

    expected = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    expected["tool"]["molt"]["dx"]["dependency_groups"] = []
    actual = tomllib.loads((driver_seed / "pyproject.toml").read_text(encoding="utf-8"))
    assert actual == expected
    assert (driver_seed / "uv.lock").read_bytes() == (
        REPO_ROOT / "uv.lock"
    ).read_bytes()
    assert DxProject(driver_seed).dependency_groups() == ()


def test_private_driver_seed_retains_its_declared_groups(tmp_path: Path) -> None:
    from molt.dx import DxProject

    private = tmp_path / "private"
    private.mkdir()
    (private / "pyproject.toml").write_text(
        '[tool.molt.dx]\ndependency_groups = ["fixture-workload"]\n'
        "[dependency-groups]\nfixture-workload = []\n",
        encoding="utf-8",
    )
    copied = tmp_path / "copied"
    copy_driver_trees(copied, seed=private)
    assert DxProject(copied).dependency_groups() == ("fixture-workload",)
    assert (copied / "pyproject.toml").read_bytes() == (
        private / "pyproject.toml"
    ).read_bytes()


def test_origin_fixture_copies_history_without_mutable_aliases(
    drv, tmp_path, _origin_seed, monkeypatch, request
):
    from molt import checkout_state, dx
    from molt.build_state_layout import build_state_root
    from tools import disk_guard

    foreign_source = tmp_path / "foreign-source"
    foreign_source.mkdir()
    foreign_artifacts = tmp_path / "foreign-artifacts"
    foreign_artifacts.mkdir()
    foreign_env = dx.RunContext(foreign_source).canonical_env(
        {dx.ARTIFACT_ROOT_ENV: str(foreign_artifacts)}, create_dirs=False
    )
    target, uv_environment = _shared_checkout_state(foreign_artifacts, foreign_source)
    control = build_state_root(
        project_root=foreign_source, cargo_target=target, environment=foreign_env
    )
    control.mkdir(parents=True)
    (control / "sentinel").write_bytes(b"foreign build control")
    with checkout_state.checkout_state_use(
        (
            (target, checkout_state.CheckoutOwner(foreign_source, "cargo-target")),
            (
                uv_environment,
                checkout_state.CheckoutOwner(foreign_source, "python-environment"),
            ),
            (control, checkout_state.CheckoutOwner(target, "build-control")),
        )
    ):
        pass
    foreign_files = {
        path.relative_to(foreign_artifacts): path.read_bytes()
        for path in foreign_artifacts.rglob("*")
        if path.is_file()
    }
    caches = {
        key: os.environ[key]
        for key in dx.CANONICAL_ROOT_ENV_KEYS
        if key in os.environ
        and key not in dx.MOLT_ROOT_ENV_KEYS
        and key != "UV_PROJECT_ENVIRONMENT"
    }
    custody = guard_custody_env()
    for key in dx.MOLT_ROOT_ENV_KEYS:
        monkeypatch.setenv(key, foreign_env[key])
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(control))
    monkeypatch.setenv(
        "MOLT_BACKEND_DAEMON_SOCKET_DIR", str(foreign_artifacts / "sockets")
    )
    for key in ("MOLT_VENV", "UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV"):
        monkeypatch.setenv(key, str(tmp_path / "missing-caller-environment"))
    # Request after poisoning the caller: fixture admission must reset it, not
    # merely assume that the enclosing pytest session selected suitable roots.
    origin, work = request.getfixturevalue("origin_and_clone")

    foreign_reads = []
    read_owner = disk_guard.read_checkout_owner

    def observed_owner(path):
        if path.is_relative_to(foreign_artifacts):
            foreign_reads.append(path)
        return read_owner(path)

    monkeypatch.setattr(disk_guard, "read_checkout_owner", observed_owner)
    initial = _git(_origin_seed, "rev-parse", "HEAD").stdout.strip()
    original_config = (_origin_seed / "config").read_bytes()
    (origin / "config").write_bytes(original_config + b"\n# private test mutation\n")
    assert (_origin_seed / "config").read_bytes() == original_config
    # A hardlink copy can appear isolated for Git's atomic ref replacement;
    # exercise direct object-byte mutation as well, after the real push.
    new_tip = _commit_file(work, "private.txt", "private\n", "private mutation")
    assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work))) == drv.EXIT_OK
    assert _git(origin, "rev-parse", "HEAD").stdout.strip() == new_tip
    assert _git(_origin_seed, "rev-parse", "HEAD").stdout.strip() == initial
    wt = _worktree_off(work, "wt_isolated_artifacts")
    assert (
        molt_dev_retire.cleanup_worktree(
            drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
        )
        == drv.EXIT_OK
    )
    assert not wt.exists()
    relative = Path("objects") / initial[:2] / initial[2:]
    original_object = (_origin_seed / relative).read_bytes()
    (origin / relative).chmod(0o600)
    (origin / relative).write_bytes(b"deliberately corrupt private copy")
    assert (_origin_seed / relative).read_bytes() == original_object
    assert _git(_origin_seed, "show", "HEAD:README.md").stdout == "seed\n"
    assert not foreign_reads, "synthetic retirement consulted foreign host artifacts"
    assert {
        path.relative_to(foreign_artifacts): path.read_bytes()
        for path in foreign_artifacts.rglob("*")
        if path.is_file()
    } == foreign_files
    assert dx.artifact_root(work, os.environ) == tmp_path / "artifacts"
    assert all(os.environ.get(key) == value for key, value in caches.items())
    assert guard_custody_env() == custody
    assert "MOLT_BUILD_STATE_DIR" not in os.environ
    assert "MOLT_BACKEND_DAEMON_SOCKET_DIR" not in os.environ


def _make_gate_venv(path: Path, token: str | None) -> Path:
    # POSIX venvs share the immutable interpreter image; prefixes, packages and
    # marker modules remain private. Windows keeps CPython's copy/redirector path.
    venv.EnvBuilder(with_pip=False, symlinks=os.name != "nt").create(path)
    # EnvBuilder uses this interpreter's venv scheme. Actual selected-Python
    # execution below proves prefix, imports, encoding and shell activation;
    # fixture setup needs no guarded interpreter just to repeat this path query.
    site = Path(
        sysconfig.get_path(
            "purelib",
            scheme="venv",
            vars={
                "base": str(path),
                "platbase": str(path),
                "installed_base": str(path),
                "installed_platbase": str(path),
            },
        )
    )
    if token is not None:
        (site / "molt_gate_selection_marker.py").write_text(
            f"TOKEN = {token!r}\n", encoding="utf-8"
        )
    return site


def _gate_shell_command(*argv: str) -> str:
    return subprocess.list2cmdline(argv) if os.name == "nt" else shlex.join(argv)


def _gate_caller_env(hostile: Path) -> ProcessEnvironmentBuilder:
    env = ProcessEnvironmentBuilder(os.environ)
    for key in (
        "MOLT_VENV",
        "UV_PROJECT_ENVIRONMENT",
        "VIRTUAL_ENV",
        "MOLT_UV_PROJECT_PYTHON",
        "MOLT_UV_PROJECT_PURPOSE",
    ):
        env.pop(key, None)
    env.update(
        PATH=str(hostile / ("Scripts" if os.name == "nt" else "bin"))
        + os.pathsep
        + env.get("PATH", ""),
        VIRTUAL_ENV=str(hostile),
        PYTHONUTF8="0",
        PYTHONIOENCODING="ascii",
        MOLT_GATE_CALLER_VALUE="retained λ",
    )
    return env


@pytest.mark.parametrize(
    "selection",
    [
        "checkout",
        "MOLT_VENV",
        "UV_PROJECT_ENVIRONMENT",
        "worktree-main",
        "worktree-own",
    ],
)
def test_run_gate_activates_selected_project_python(drv, tmp_path, selection):
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    repo = tmp_path / "main"
    repo.mkdir()
    main_repo = repo
    if selection.startswith("worktree-"):
        _init_repo(repo)
        _commit_file(repo, "README.md", "fixture\n", "fixture")
        repo = tmp_path / "worktree"
        _git(main_repo, "worktree", "add", "-q", "-b", "gate-test", str(repo))
    hostile = tmp_path / "hostile"
    _make_gate_venv(hostile, None)
    env = _gate_caller_env(hostile)
    if selection == "worktree-main":
        _make_gate_venv(main_repo / ".venv", "main must not run")
        with pytest.raises(drv.DriverError, match="development image not selected"):
            drv.run_gate(
                "python -c 'raise SystemExit(0)'",
                repo,
                drv.prepare_gate_environment(
                    repo, ProcessEnvironment(env), requires_python=True
                ),
            )
        return
    managed = selection != "MOLT_VENV"
    if managed:
        image = materialize_development_environment(
            repo, tmp_path / "artifacts", runnable=True, select=True
        )
        selected = image.root
        env["MOLT_EXT_ROOT"] = str(tmp_path / "artifacts")
        if selection == "UV_PROJECT_ENVIRONMENT":
            env["UV_PROJECT_ENVIRONMENT"] = os.path.relpath(selected, repo)
    else:
        selected = tmp_path / "explicit environment"
        _make_gate_venv(selected, "selected λ")
        env["MOLT_VENV"] = str(selected)
        env["UV_PROJECT_ENVIRONMENT"] = str(hostile)
    script = tmp_path / "observe.py"
    script.write_text(
        "import json, os, sys\nfrom pathlib import Path\n"
        "TOKEN = None\n"
        "if os.environ.get('MOLT_VENV'):\n from molt_gate_selection_marker import TOKEN\n"
        "Path(sys.argv[1]).write_text(json.dumps({\n"
        " 'token': TOKEN, 'prefix': sys.prefix,\n"
        " 'virtual_env': os.environ['VIRTUAL_ENV'],\n"
        " 'molt_venv': os.environ.get('MOLT_VENV'),\n"
        " 'caller': os.environ['MOLT_GATE_CALLER_VALUE'],\n"
        " 'utf8': sys.flags.utf8_mode, 'encoding': sys.stdout.encoding,\n"
        "}), encoding='utf-8')\n",
        encoding="utf-8",
    )
    outputs = [tmp_path / "first.json", tmp_path / "second.json"]
    command = " && ".join(
        _gate_shell_command("python", str(script), str(output)) for output in outputs
    )
    before = dict(env)
    assert (
        drv.run_gate(
            command,
            repo,
            drv.prepare_gate_environment(
                repo, ProcessEnvironment(env), requires_python=True
            ),
        )
        == 0
    )
    assert dict(env) == before
    expected = {
        "token": None if managed else "selected λ",
        "prefix": str(selected),
        "virtual_env": str(selected),
        "molt_venv": None if managed else str(selected),
        "caller": "retained λ",
        "utf8": 1,
        "encoding": "utf-8",
    }
    assert [json.loads(path.read_text(encoding="utf-8")) for path in outputs] == [
        expected,
        expected,
    ]
    assert (
        drv.run_gate(
            'python -c "raise SystemExit(23)"',
            repo,
            drv.prepare_gate_environment(
                repo, ProcessEnvironment(env), requires_python=True
            ),
        )
        == 23
    )
    if managed:
        assert (
            drv.run_gate(
                (
                    "python",
                    "-I",
                    "-E",
                    "-c",
                    "import sys; assert sys.pycache_prefix == sys.argv[1]; assert sys.dont_write_bytecode",
                    str(selected / "bytecode"),
                ),
                repo,
                drv.prepare_gate_environment(
                    repo, ProcessEnvironment(env), requires_python=True
                ),
            )
            == 0
        )
        with pytest.raises(drv.DriverError, match="requires structured argv"):
            drv._gate_environment(
                "echo before && " + _gate_shell_command("python", "-I", "-c", "pass"),
                repo,
                drv.prepare_gate_environment(
                    repo, ProcessEnvironment(env), requires_python=True
                ),
            )
        with pytest.raises(drv.DriverError, match="cache prefix conflicts"):
            drv._gate_environment(
                "echo before && "
                + _gate_shell_command(
                    "python",
                    "-X",
                    f"pycache_prefix={tmp_path / 'foreign-cache'}",
                    "-c",
                    "pass",
                ),
                repo,
                drv.prepare_gate_environment(
                    repo, ProcessEnvironment(env), requires_python=True
                ),
            )


@pytest.mark.parametrize(
    "wrapper",
    [
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    ],
)
def test_run_gate_compound_shell_preserves_durable_cargo_environment(
    drv, tmp_path, wrapper
):
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    repo = tmp_path / "source"
    repo.mkdir()
    image = materialize_development_environment(
        repo, tmp_path / "artifacts", runnable=True, select=True
    )
    env = _gate_caller_env(tmp_path / "hostile")
    env["MOLT_EXT_ROOT"] = str(tmp_path / "artifacts")
    for key in (
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "SCCACHE_SERVER_UDS",
    ):
        env.pop(key, None)
    scratch = tmp_path / "one-command-scratch"
    scratch.mkdir()
    env.update(
        SCCACHE_DIR=str(tmp_path / "persistent-cache"),
        SCCACHE_SERVER_PORT="49971",
        CARGO_INCREMENTAL="1",
        TMPDIR=str(scratch),
        TMP=str(scratch),
        TEMP=str(scratch),
    )
    env[wrapper] = str(tmp_path / "sccache")
    script = tmp_path / "observe.py"
    script.write_text(
        "import json, os, sys\n"
        "from pathlib import Path\n"
        "keys = ('TMPDIR', 'TMP', 'TEMP', 'CARGO_INCREMENTAL', "
        "'SCCACHE_SERVER_PORT', sys.argv[2])\n"
        "observed = {key: os.environ.get(key) for key in keys}\n"
        "observed.update(prefix=sys.prefix, cache=sys.pycache_prefix, "
        "no_write=sys.dont_write_bytecode)\n"
        "Path(sys.argv[1]).write_text(json.dumps(observed), encoding='utf-8')\n",
        encoding="utf-8",
    )
    outputs = [tmp_path / "first.json", tmp_path / "second.json"]
    command = " && ".join(
        _gate_shell_command("python", str(script), str(output), wrapper)
        for output in outputs
    )
    before = dict(env)
    assert (
        drv.run_gate(
            command,
            repo,
            drv.prepare_gate_environment(
                repo, ProcessEnvironment(env), requires_python=True
            ),
        )
        == 0
    )
    assert dict(env) == before
    first, second = [json.loads(path.read_text(encoding="utf-8")) for path in outputs]
    assert first == second
    assert first["prefix"] == str(image.root)
    assert first["cache"] == str(image.root / "bytecode")
    assert first["no_write"] is True
    assert first[wrapper] == env[wrapper]
    assert first["SCCACHE_SERVER_PORT"] == "49971"
    # sccache refuses an explicit nonzero CARGO_INCREMENTAL; the profile decides.
    assert first["CARGO_INCREMENTAL"] is None
    temporary = Path(first["TMPDIR"])
    assert temporary.is_dir() and temporary != scratch
    assert first["TMP"] == first["TEMP"] == str(temporary)
    scratch.rmdir()
    assert temporary.is_dir()


@pytest.mark.parametrize("selector", ["MOLT_VENV", "UV_PROJECT_ENVIRONMENT"])
def test_run_gate_invalid_selected_venv_never_launches_shell(drv, tmp_path, selector):
    repo = tmp_path / "repo"
    repo.mkdir()
    _make_gate_venv(repo / ".venv", "valid fallback must not run")
    env = _gate_caller_env(repo / ".venv")
    env[selector] = "missing-environment"
    if selector == "MOLT_VENV":
        env["UV_PROJECT_ENVIRONMENT"] = str(repo / ".venv")
    before = dict(env)
    message = (
        "virtualenv Python not found"
        if selector == "MOLT_VENV"
        else "managed development image refused"
    )
    with pytest.raises(drv.DriverError, match=message):
        drv.run_gate(
            "echo should-not-run > launched.txt",
            repo,
            drv.prepare_gate_environment(
                repo, ProcessEnvironment(env), requires_python=True
            ),
        )
    assert not (repo / "launched.txt").exists()
    assert dict(env) == before


def test_run_gate_invalid_worktree_venv_does_not_fall_back_to_main(drv, tmp_path):
    main = tmp_path / "main"
    _init_repo(main)
    _commit_file(main, "README.md", "fixture\n", "fixture")
    _make_gate_venv(main / ".venv", "main must not run")
    work = tmp_path / "work"
    _git(main, "worktree", "add", "-q", "-b", "gate-test", str(work))
    (work / ".venv").mkdir()
    with pytest.raises(drv.DriverError, match="development image not selected"):
        drv.run_gate(
            ("python", "-c", "raise SystemExit(0)"),
            work,
            drv.prepare_gate_environment(
                work,
                ProcessEnvironment(_gate_caller_env(main / ".venv")),
                requires_python=True,
            ),
        )
    assert not (work / "launched.txt").exists()


def test_run_gate_missing_project_venv_refuses_python_but_allows_generic_shell(
    drv, tmp_path
):
    env = _gate_caller_env(tmp_path / "absent")
    with pytest.raises(drv.DriverError, match="development image not selected"):
        drv.run_gate(
            'python -c "raise SystemExit(0)"',
            tmp_path,
            drv.prepare_gate_environment(
                tmp_path, ProcessEnvironment(env), requires_python=True
            ),
        )
    assert (
        drv.run_gate(
            "echo generic > observed.txt",
            tmp_path,
            drv.prepare_gate_environment(
                tmp_path, ProcessEnvironment(env), requires_python=False
            ),
        )
        == 0
    )
    assert (tmp_path / "observed.txt").read_text(encoding="utf-8").strip() == "generic"
    if os.name != "nt":
        # POSIX sh reports unmatched quotes; cmd.exe accepts that spelling.
        assert (
            drv.run_gate(
                'echo "unterminated',
                tmp_path,
                drv.prepare_gate_environment(
                    tmp_path, ProcessEnvironment(env), requires_python=False
                ),
            )
            != 0
        )


def test_run_gate_missing_selected_python_alias_never_uses_host_path(drv, tmp_path):
    selected = tmp_path / ".venv"
    _make_gate_venv(selected, "selected")
    alias = selected / ("Scripts/python3.exe" if os.name == "nt" else "bin/python3")
    alias.unlink(missing_ok=True)
    env = _gate_caller_env(tmp_path / "unused")
    env["MOLT_VENV"] = str(selected)
    with pytest.raises(
        drv.DriverError, match="virtualenv command 'python3' not executable"
    ):
        drv.run_gate(
            'python3 -c "raise SystemExit(0)"',
            tmp_path,
            drv.prepare_gate_environment(
                tmp_path, ProcessEnvironment(env), requires_python=True
            ),
        )


@pytest.mark.parametrize("head", ['"python"', '"python3"', '"python.exe"'])
def test_run_gate_quoted_windows_python_requires_project_environment(
    drv, tmp_path, monkeypatch, head
):
    install_module_os_view(monkeypatch, drv, name="nt")
    monkeypatch.setattr(
        drv,
        "_run_live_gate",
        lambda *args, **kwargs: pytest.fail("host shell launched"),
    )
    with pytest.raises(drv.DriverError, match="development image not selected"):
        drv.run_gate(
            f'{head} -c "raise SystemExit(0)"',
            tmp_path,
            drv.prepare_gate_environment(
                tmp_path,
                ProcessEnvironment(_gate_caller_env(tmp_path / "absent")),
                requires_python=True,
            ),
        )


def test_cmd_fix_uses_the_selected_environment_without_per_command_wrappers(
    drv, tmp_path, monkeypatch
):
    repo = tmp_path / "repo"
    repo.mkdir()
    selected = tmp_path / "explicit environment"
    site = _make_gate_venv(selected, "selected")
    events = tmp_path / "events.jsonl"
    recorder = (
        "import json, sys\n"
        "from pathlib import Path\n"
        f"with Path({str(events)!r}).open('a', encoding='utf-8') as stream:\n"
        " stream.write(json.dumps([sys.prefix, sys.argv[1:]]) + '\\n')\n"
    )
    (repo / "tools").mkdir()
    for name in ("encoding_gate.py", "generators.py"):
        (repo / "tools" / name).write_text(recorder, encoding="utf-8")
    (site / "ruff.py").write_text(recorder, encoding="utf-8")
    monkeypatch.setenv("MOLT_VENV", str(selected))
    monkeypatch.setenv("UV_PROJECT_ENVIRONMENT", str(tmp_path / "wrong uv"))
    assert drv.cmd_fix(_ns(drv, repo=str(repo))) == 0
    assert [
        json.loads(line) for line in events.read_text(encoding="utf-8").splitlines()
    ] == [
        [str(selected), ["--fix"]],
        [str(selected), ["format", "."]],
        [str(selected), ["write"]],
        [str(selected), ["check"]],
    ]


# --------------------------------------------------------------------------
# Hazard 10: gate manifest selection (pure, no git) — and committed-manifest lint
# --------------------------------------------------------------------------


def test_selected_rules_share_one_pytest_invocation_for_equal_requirements(
    drv, tmp_path
):
    source = """
always = ["python tools/check_a.py"]
[[rule]]
name = "first"
globs = ["src/a/**"]
gates = ["python tools/check_first.py"]
pytest = ["tests/test_a.py", "tests/test_shared.py"]
toolchains = ["python"]
[[rule]]
name = "second"
globs = ["src/b/**"]
gates = ["cargo test -p b"]
pytest = ["tests/test_shared.py", "tests/test_b.py::test_one"]
toolchains = ["python"]
[[rule]]
name = "unmatched"
globs = ["src/c/**"]
gates = []
pytest = ["tests/test_c.py"]
toolchains = ["python"]
"""
    (tmp_path / "tests").mkdir()
    for name in ("a", "b", "c", "shared"):
        (tmp_path / "tests" / f"test_{name}.py").write_text("", encoding="utf-8")
    cfg = GateConfig.parse(source, "equal-requirement pytest fixture", root=tmp_path)
    gates, matched = cfg.select(["src/a/x.py", "src/b/y.py"])
    gates = [gate.command for gate in gates]
    assert [rule.name for rule in matched] == ["first", "second"]
    # Equal envelopes and tools share one run after the explicit gates.
    assert gates == [
        "python tools/check_a.py",
        "python tools/check_first.py",
        "cargo test -p b",
        (
            "python",
            "-m",
            "pytest",
            "-q",
            "-n",
            "auto",
            "--molt-worker-memory-gb=2.5",
            "tests/test_a.py",
            "tests/test_shared.py",
            "tests/test_b.py::test_one",
        ),
    ]


@pytest.mark.parametrize("duplicate_explicit", [False, True])
def test_landing_cohorts_join_overlaps_and_all_target_owners(
    drv, tmp_path, monkeypatch, duplicate_explicit
):
    from tools import proof_plan

    (tmp_path / "tests").mkdir()
    for name in ("light", "also", "shared", "heavy", "nodes", "parameter"):
        (tmp_path / "tests" / f"{name}.py").touch()
    # Raw explicit commands reserve template braces; the structured-only case
    # independently keeps the full opaque payload, including {value}.
    payload = (
        "x::y / & 'quotes' [nested]"
        if duplicate_explicit
        else "x::y / & 'quotes' {value} [nested]"
    )
    parameter = f"tests/parameter.py::test_literal[{payload}]"
    prefix = ("python", "-m", "pytest", "-q", "-n", "auto")
    compiled = (
        *prefix,
        "--molt-worker-memory-gb=6.25",
        "tests/shared.py",
        "tests/nodes.py",
        "tests/heavy.py",
        "tests/nodes.py::test_heavy",
        parameter,
    )
    policy = """
always = []
[[command]]
pytest = ["tests/light.py", "tests/also.py", "tests/shared.py", "tests/nodes.py", "tests/parameter.py"]
pytest_worker_memory_gb = 0.5
[[command]]
pytest = ["tests/heavy.py", "tests/nodes.py::test_heavy"]
pytest_worker_memory_gb = 6.25
[[rule]]
name = "pure"
globs = ["src/**"]
gates = []
pytest = ["tests/light.py", "tests/shared.py", "tests/nodes.py", "tests/also.py"]
toolchains = ["python"]
[[rule]]
name = "compiled"
globs = ["src/**"]
gates = []
pytest = ["tests/heavy.py", "tests/shared.py", "tests/nodes.py::test_heavy", PARAMETER]
toolchains = ["python", "cargo"]
""".replace("PARAMETER", json.dumps(parameter))
    if duplicate_explicit:
        # Later deduplication must still address the existing explicit command
        # after earlier new cohorts have been selected for insertion.
        policy += (
            '\n[[rule]]\nname = "explicit"\nglobs = ["src/**"]\n'
            f"gates = [{json.dumps(shlex.join(compiled))}]\n"
            'toolchains = ["python", "sdk"]\n'
        )
    config = GateConfig.parse(policy, "target-owned resource cohorts", root=tmp_path)

    def unexpected_probe(*_args, **_kwargs):
        pytest.fail("cohort selection probed the filesystem after admission")

    with monkeypatch.context() as patch:
        patch.setattr(proof_plan.os, "listdir", unexpected_probe)
        patch.setattr(Path, "stat", unexpected_probe)
        patch.setattr(Path, "lstat", unexpected_probe)
        gates, matched = config.select(["src/changed.py"])
    assert [rule.name for rule in matched] == ["pure", "compiled"] + (
        ["explicit"] if duplicate_explicit else []
    )
    assert [(gate.command, gate.toolchains) for gate in gates] == [
        (
            (*prefix, "--molt-worker-memory-gb=0.5", "tests/light.py", "tests/also.py"),
            ("python",),
        ),
        (
            shlex.join(compiled) if duplicate_explicit else compiled,
            ("python", "sdk", "cargo") if duplicate_explicit else ("python", "cargo"),
        ),
    ]
    assert all(gate.pytest for gate in gates)
    targets = [
        arg
        for gate in gates
        for arg in (
            shlex.split(gate.command) if isinstance(gate.command, str) else gate.command
        )
        if arg.startswith("tests/")
    ]
    assert len(targets) == len(set(targets)) == 7
    assert gates[0].collection_command is None
    assert gates[1].collection_command[-3:] == (
        "tests/nodes.py::test_heavy",
        parameter,
        "--collect-only",
    )
    assert "tests/nodes.py" not in gates[1].collection_command
    assert "tests/shared.py" not in gates[1].collection_command


def test_selected_rules_run_each_pytest_selection_on_its_own(drv, tmp_path):
    source = """
[[rule]]
name = "first"
globs = ["src/a/**"]
gates = []
pytest = ["tests/test_a.py"]
pytest_select = [{ k = "fast or quick", targets = ["tests/test_b.py"] }]
toolchains = ["python"]
[[rule]]
name = "second"
globs = ["src/b/**"]
gates = []
pytest_select = [
  { k = "fast or quick", targets = ["tests/test_b.py"] },
  { m = "not slow", targets = ["tests/test_b.py"] },
]
toolchains = ["python"]
"""
    (tmp_path / "tests").mkdir()
    for name in ("a", "b"):
        (tmp_path / "tests" / f"test_{name}.py").write_text("", encoding="utf-8")
    cfg = GateConfig.parse(source, "pytest selection fixture", root=tmp_path)
    gates, _ = cfg.select(["src/a/x.py", "src/b/y.py"])
    gates = [gate.command for gate in gates]
    prefix = (
        "python",
        "-m",
        "pytest",
        "-q",
        "-n",
        "auto",
        "--molt-worker-memory-gb=2.5",
    )
    # A selection applies to every target of its run, so it never merges
    # into the shared run; an identical selection runs once.
    assert gates == [
        (*prefix, "tests/test_a.py"),
        (*prefix, "-k", "fast or quick", "tests/test_b.py"),
        (*prefix, "-m", "not slow", "tests/test_b.py"),
    ]
    assert set(cfg.prerequisites) == {
        "local.first.pytest",
        "local.first.pytest-select-0",
        "local.second.pytest-select-0",
        "local.second.pytest-select-1",
    }


@pytest.mark.parametrize(
    "target,message",
    [
        ("-k", "must be a tests/ path"),
        ("", "must be a list of test paths"),
        ("src/test_x.py", "must be a tests/ path"),
        ("tests/../outside.py", "must be a tests/ path"),
        ("tests/missing.py", "does not exist"),
    ],
)
def test_gate_driver_rejects_invalid_batch_targets(drv, tmp_path, target, message):
    (tmp_path / "outside.py").write_text("", encoding="utf-8")
    source = f"""[[rule]]
name = "invalid"
globs = ["**"]
gates = []
pytest = [{json.dumps(target)}]
"""
    with pytest.raises(drv.DriverError, match=message):
        GateConfig.parse(source, "invalid batch", root=tmp_path)


@pytest.mark.parametrize("platform_name", ["posix", "nt"])
def test_structured_gate_preserves_node_ids_without_a_shell(
    drv, tmp_path, monkeypatch, platform_name
):
    from tools import molt_dev_common

    install_module_os_view(monkeypatch, molt_dev_common, name=platform_name)
    targets = (
        "tests/test_x.py::test_one[spaces and & > 'quotes']",
        'tests/test_x.py::test_one["double quotes"]',
    )
    from tools.proof_plan import PytestMemoryUnits, local_pytest_argv

    command = local_pytest_argv(
        targets, memory_units=PytestMemoryUnits((), root=tmp_path)
    )
    observed = []

    def guarded(argv, **kwargs):
        observed.append(argv)
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(
        molt_dev_common.harness_memory_guard, "guarded_completed_process", guarded
    )
    assert (
        molt_dev_common._run_live_gate(
            command, repo=tmp_path, env=ProcessEnvironment({})
        )
        == 0
    )
    assert observed == [
        [
            "python",
            "-m",
            "pytest",
            "-q",
            "-n",
            "auto",
            "--molt-worker-memory-gb=2.5",
            *targets,
        ]
    ]


@pytest.mark.parametrize(
    "unit", ["0.0", "-1.0", "nan", "inf", "1e-100", "1e308", '"invalid"', "true"]
)
def test_gate_driver_rejects_malformed_declared_pytest_unit_before_admission(drv, unit):
    source = (
        '[[command]]\nargv = ["python", "-m", "pytest"]\n'
        'pytest = ["tests/test_x.py"]\n'
        f"pytest_worker_memory_gb = {unit}\n"
    )
    with pytest.raises(drv.DriverError, match="invalid local gate policy"):
        GateConfig.parse(source, "invalid worker unit")


def test_run_gate_structured_arguments_use_selected_python(drv, tmp_path):
    repo = tmp_path / "source"
    repo.mkdir()
    selected = repo / ".venv"
    _make_gate_venv(selected, "selected")
    observed = repo / "observed.json"
    literal = "case[spaces & > 'quotes' and \"quotes\"]"
    script = (
        "import json,sys; from pathlib import Path; "
        "from molt_gate_selection_marker import TOKEN; "
        "Path(sys.argv[1]).write_text(json.dumps([TOKEN,sys.prefix,sys.argv[2]]), encoding='utf-8')"
    )
    command = ("python", "-c", script, str(observed), literal)
    assert (
        drv.run_gate(
            command,
            repo,
            drv.prepare_gate_environment(
                repo,
                ProcessEnvironment(_gate_caller_env(tmp_path / "absent")).updated(
                    {"MOLT_VENV": str(selected)}
                ),
                requires_python=True,
            ),
        )
        == 0
    )
    assert json.loads(observed.read_text(encoding="utf-8")) == [
        "selected",
        str(selected),
        literal,
    ]
    with pytest.raises(drv.DriverError, match="development image not selected"):
        drv.run_gate(
            command,
            tmp_path,
            drv.prepare_gate_environment(
                tmp_path,
                ProcessEnvironment(_gate_caller_env(tmp_path / "absent")),
                requires_python=True,
            ),
        )


def _write_gates(tmp_path: Path) -> Path:
    cfg = tmp_path / "gates.toml"
    cfg.write_text(
        """
always = ["echo always-ran"]

[[rule]]
name = "backend-native"
globs = ["runtime/molt-backend/**/*.rs"]
gates = ["echo backend-gate"]

[[rule]]
name = "frontend-python"
globs = ["src/molt/**/*.py"]
gates = ["echo python-gate"]
""",
        encoding="utf-8",
    )
    return cfg


def test_gate_manifest_selection_backend(drv, tmp_path):
    cfg = GateConfig.load(_write_gates(tmp_path))
    gates, matched = cfg.select(["runtime/molt-backend/src/lib.rs"])
    gates = [gate.command for gate in gates]
    assert "echo always-ran" in gates
    assert "echo backend-gate" in gates
    assert "echo python-gate" not in gates
    assert [r.name for r in matched] == ["backend-native"]


def test_gate_manifest_selection_python(drv, tmp_path):
    cfg = GateConfig.load(_write_gates(tmp_path))
    gates, matched = cfg.select(["src/molt/frontend.py"])
    gates = [gate.command for gate in gates]
    assert "echo python-gate" in gates
    assert "echo backend-gate" not in gates
    assert [r.name for r in matched] == ["frontend-python"]


def test_gate_manifest_always_runs(drv, tmp_path):
    # An unmatched change-class still runs `always` gates, nothing else.
    cfg = GateConfig.load(_write_gates(tmp_path))
    gates, matched = cfg.select(["docs/notes.txt"])
    gates = [gate.command for gate in gates]
    assert gates == ["echo always-ran"]
    assert matched == []


def test_gate_manifest_dedups_across_rules(drv, tmp_path):
    cfg_path = tmp_path / "g.toml"
    cfg_path.write_text(
        """
always = []
[[rule]]
name = "a"
globs = ["x/**"]
gates = ["echo shared", "echo a-only"]
[[rule]]
name = "b"
globs = ["y/**"]
gates = ["echo shared", "echo b-only"]
""",
        encoding="utf-8",
    )
    cfg = GateConfig.load(cfg_path)
    gates, _ = cfg.select(["x/1", "y/2"])
    gates = [gate.command for gate in gates]
    # 'echo shared' appears once, preserving first-seen order.
    assert gates == ["echo shared", "echo a-only", "echo b-only"]


def test_gate_manifest_rejects_bad_toml(drv, tmp_path):
    bad = tmp_path / "bad.toml"
    bad.write_text("[[rule]]\nname = 5\nglobs = []\ngates = []\n", encoding="utf-8")
    with pytest.raises(drv.DriverError):
        GateConfig.load(bad)


def test_gate_manifest_missing_is_usage_error(drv, tmp_path):
    with pytest.raises(drv.DriverError) as exc:
        GateConfig.load(tmp_path / "does_not_exist.toml")
    assert exc.value.code == drv.EXIT_USAGE


# --------------------------------------------------------------------------
# Hazard 2: push confirmation by fetch+ancestor (not exit code)
# --------------------------------------------------------------------------


def test_verify_push_confirms_landed_tip(drv, origin_and_clone):
    origin, work = origin_and_clone
    tip = _commit_file(work, "a.py", "print(1)\n", "feat: a")
    _git(work, "push", "-q", "origin", "main")
    ns = _ns(drv, repo=str(work), remote="origin", branch="main", tip=tip, json=False)
    assert drv.cmd_verify_push(ns) == drv.EXIT_OK


def test_verify_push_catches_non_landed_tip(drv, origin_and_clone):
    # A tip that was committed but NOT pushed must be reported as not landed,
    # regardless of any (hypothetically swallowed) push exit code.
    origin, work = origin_and_clone
    tip = _commit_file(work, "a.py", "print(1)\n", "feat: a (unpushed)")
    ns = _ns(drv, repo=str(work), remote="origin", branch="main", tip=tip, json=False)
    assert drv.cmd_verify_push(ns) == drv.EXIT_FAIL


# --------------------------------------------------------------------------
# Hazard 1: rebase drops commits (patch-id survival)
# --------------------------------------------------------------------------


def test_integrate_detects_dropped_commit(drv, origin_and_clone):
    """An unmatched source patch in neither real Git range is a hard failure."""
    _, work = origin_and_clone
    git = drv.Git(work)
    upstream = git.rev_parse("origin/main")
    dropped = _commit_file(work, "feature.py", "x = 1\n", "feat: important change")
    pre = {dropped: git.patch_id(dropped)}
    assert pre[dropped] is not None
    _git(work, "checkout", "--detach", upstream)
    upstream = _commit_file(
        work, "unrelated.py", "other = 2\n", "unrelated incoming patch"
    )
    _git(work, "checkout", "main")
    assert git.patch_id(upstream) != pre[dropped]
    # Model a replay that discarded the source: candidate equals upstream.
    # Neither that candidate nor the pinned incoming span contains its patch.
    with pytest.raises(drv.DriverError) as exc:
        drv._verify_no_dropped_commits(
            git, [dropped], pre, f"{upstream}..{upstream}", f"{dropped}..{upstream}"
        )
    assert "DROPPED COMMITS" in str(exc.value)
    assert git.short(dropped) in str(exc.value)


def test_integrate_accepts_already_upstream_by_patch_id(
    drv, origin_and_clone, monkeypatch
):
    """A real replay deduplicates an independently landed patch into no commits."""
    origin, work = origin_and_clone
    git = drv.Git(work)
    source = _commit_file(work, "shared.py", "value = 42\n", "feat: shared value")
    other = work.parent / "other"
    _git(work.parent, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "dev2@molt.test")
    _git(other, "config", "user.name", "Other Dev")
    _commit_file(other, "upstream.txt", "independent\n", "advance upstream")
    _git(other, "fetch", "-q", str(work), source)
    _git(other, "cherry-pick", source)
    upstream = _git(other, "rev-parse", "HEAD").stdout.strip()
    assert upstream != source
    _git(other, "push", "-q", "origin", "main")

    commands = _record_driver_git_commands(drv, monkeypatch)
    assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work))) == drv.EXIT_OK
    assert git.head_sha() == upstream
    assert git.commits_in_range(f"{upstream}..HEAD") == []
    assert not git.is_ancestor(source, upstream)
    assert (work / "shared.py").read_text(encoding="utf-8") == "value = 42\n"
    assert (
        _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
        == upstream
    )
    replay = next(index for index, args in enumerate(commands) if args[0] == "rebase")
    # No candidate commits remain; survival must inspect the captured incoming
    # span and its distinct cherry-picked commit, not upstream..upstream.
    assert ["rev-list", f"{source}..{upstream}"] in commands[replay + 1 :]
    assert ["diff-tree", "-p", "--no-color", upstream] in commands[replay + 1 :]


def test_patch_id_stable_across_rebase(drv, origin_and_clone):
    # The same diff has the same patch-id even after the commit is replayed onto
    # a new base (the property the survival check relies on).
    origin, work = origin_and_clone
    git = drv.Git(work)
    src = _commit_file(work, "p.py", "a = 1\nb = 2\n", "feat: p")
    pid_before = git.patch_id(src)
    # Advance origin so a rebase is required, then rebase.
    other = work.parent / "other2"
    _git(work.parent, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "d@molt.test")
    _git(other, "config", "user.name", "D")
    _commit_file(other, "unrelated.py", "z = 9\n", "chore: unrelated")
    _git(other, "push", "-q", "origin", "main")
    git.fetch("origin", "main")
    _git(work, "rebase", "-q", "origin/main")
    new_head = git.head_sha()
    pid_after = git.patch_id(new_head)
    assert new_head != src  # the commit was replayed (new sha)
    assert pid_before == pid_after  # but the patch-id is invariant


# --------------------------------------------------------------------------
# Hazard 3: cleanup refuses on unpushed / dirty; --force needs the sha
# --------------------------------------------------------------------------


def _worktree_off(main_clone: Path, name: str) -> Path:
    """Link a disposable worktree to the fixture's already isolated main clone.

    Its Git configuration is shared with the linked worktree; each test's
    origin, main clone and configuration remain private to that test.
    """
    wt = main_clone.parent / name
    _git(main_clone, "worktree", "add", "-q", "--detach", str(wt), "origin/main")
    return wt


def test_cleanup_allows_clean_pushed(drv, origin_and_clone, tmp_path, monkeypatch):
    from tools import molt_dev_common

    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_clean")
    original_readme = (main / "README.md").read_bytes()
    # Model a driver loaded from the disposable checkout. The actual launch
    # boundary must leave it before removal, even on POSIX where Git may allow
    # deleting a process cwd and conceal this Windows shim failure.
    monkeypatch.setattr(molt_dev_common, "DEFAULT_REPO", wt)
    executor_type = type(molt_dev_common._COMMANDS)
    run = executor_type.run
    removal_cwds: list[Path] = []

    def check_removal_cwd(self, command, **kwargs):
        if command[3:5] == ["worktree", "remove"]:
            cwd = Path(kwargs["cwd"]).resolve()
            assert cwd == main.resolve(), (
                "retirement must launch from the surviving tree"
            )
            removal_cwds.append(cwd)
        return run(self, command, **kwargs)

    monkeypatch.setattr(executor_type, "run", check_removal_cwd)
    # No commits beyond origin/main, no dirty tree -> cleanup proceeds.
    rc = molt_dev_retire.cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_OK
    assert removal_cwds == [main.resolve()]
    assert not wt.exists()
    assert (main / "README.md").read_bytes() == original_readme
    registered = [
        Path(line.removeprefix("worktree ")).resolve()
        for line in _git(main, "worktree", "list", "--porcelain").stdout.splitlines()
        if line.startswith("worktree ")
    ]
    assert registered == [main.resolve()]


def test_cleanup_retires_the_checkout_daemons_before_removing_it(
    drv, origin_and_clone, tmp_path, monkeypatch
):
    from molt.cli import backend_execution

    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_daemons")
    retired: list[tuple[Path, bool]] = []

    def retire(project_root, env, **kwargs):
        retired.append((project_root, project_root.exists()))
        return (4321,)

    monkeypatch.setattr(backend_execution, "retire_project_backend_daemons", retire)
    rc = molt_dev_retire.cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_OK
    assert retired == [(wt, True)], "daemons retire while their checkout still exists"
    assert not wt.exists()


def _shared_checkout_state(ext: Path, checkout: Path) -> tuple[Path, Path]:
    """A checkout's Cargo target and Python generation state under a shared root."""
    from molt import dx

    target = dx.cargo_target_dir_for_artifact_root(ext, None, source_root=checkout)
    uv_environment = (
        ext
        / "build-environments"
        / "development"
        / dx.checkout_component(checkout)
        / ("a" * 64)
    )
    for root in (target, uv_environment):
        root.mkdir(parents=True)
        (root / "payload.bin").write_bytes(b"\0" * 64)
    (uv_environment / "pyvenv.cfg").write_text("home = /x\n", encoding="utf-8")
    return target, uv_environment


def test_cleanup_retires_the_checkouts_state_with_it(
    drv, origin_and_clone, tmp_path, monkeypatch, isolated_checkout_artifacts
):
    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_state")
    ext = isolated_checkout_artifacts
    target, uv_environment = _shared_checkout_state(ext, wt)
    sibling_target, sibling_uv = _shared_checkout_state(ext, main)
    rc = molt_dev_retire.cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_OK
    assert not wt.exists()
    assert not target.exists() and not uv_environment.exists()
    assert sibling_target.is_dir() and sibling_uv.is_dir()


@pytest.mark.parametrize("cwd", ["outside", "inside"])
def test_cleanup_imports_nothing_after_it_removes_the_worktree(
    origin_and_clone, tmp_path, cwd, isolated_checkout_artifacts
):
    """Integration runs from the checkout it removes, so retirement must import
    every module it needs before `git worktree remove`: afterwards a lazy import
    finds no source. A fresh process with an import guard models that. A
    landing runs `integrate --repo .` with the checkout as its working
    directory, which the removal deletes; the "inside" case runs that way."""
    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_import_order")
    ext = isolated_checkout_artifacts
    target, uv_environment = _shared_checkout_state(ext, wt)
    code = f"""
import os
import sys
from pathlib import Path
sys.path[:0] = [{str(REPO_ROOT / "src")!r}, {str(REPO_ROOT)!r}]
from tools import molt_dev as drv
from tools import molt_dev_retire


class RefuseAfterRemoval:
    armed = False

    def find_spec(self, name, path=None, target=None):
        if self.armed and name.split(".")[0] in ("molt", "tools"):
            raise ImportError(f"imported {{name}} after the worktree was removed")
        return None


guard = RefuseAfterRemoval()
sys.meta_path.insert(0, guard)
real = molt_dev_retire._run_fast_captured_command


def run(command, **kwargs):
    result = real(command, **kwargs)
    if command[3:5] == ["worktree", "remove"]:
        guard.armed = True
    return result


molt_dev_retire._run_fast_captured_command = run
wt = Path(sys.argv[1])
if sys.argv[2] == "inside":
    os.chdir(wt)
    assert Path.cwd() == wt
sys.exit(molt_dev_retire.cleanup_worktree(
    drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
))
"""
    completed = run_guarded_test_process(
        [sys.executable, "-c", code, str(wt), cwd],
        # The actual worker starts inside when selected; its redirector and
        # guard must outlive removal and therefore launch from stable scratch.
        cwd=tmp_path,
        env={**os.environ, "MOLT_EXT_ROOT": str(ext)},
        timeout=180,
    )
    assert completed.returncode == 0, completed.stderr
    assert "were not reclaimed" not in completed.stderr
    assert not wt.exists()
    assert not target.exists() and not uv_environment.exists()


@pytest.mark.parametrize("release_timing", ["before", "omitted", "after"])
def test_cleanup_makes_a_running_vm_release_its_pins_of_the_worktree_first(
    origin_and_clone, tmp_path, release_timing
):
    """A Linux VM that pins files of the retiring checkout releases them first.

    The landing runs inside the checkout it removes, with every later import
    refused, as in the test above. A strict Host command boundary models one
    running vz VM that mounts the checkout's parent and holds a file until
    release. Real release planning, lsof parsing and Git removal run in the
    fresh guarded child; fake commands need no nested guards or interpreters.
    LocalHost process execution belongs to test_linux_vm.py. Omitted and late
    releases must fail this same filesystem oracle.
    """
    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_vm_pins")
    lima_home = tmp_path / "lima"
    instance = lima_home / "vm"
    instance.mkdir(parents=True)
    # The pytest worker outlives the child, so this hostagent is live.
    (instance / "ha.pid").write_text(f"{os.getpid()}\n", encoding="utf-8")
    dropped = tmp_path / "dropped"
    row = {
        "name": "vm",
        "status": "Running",
        "dir": str(instance),
        "vmType": "vz",
        "arch": "aarch64",
        "cpus": 1,
        "memory": 1 << 30,
        "disk": 1 << 30,
        "config": {
            "mounts": [
                {
                    "location": str(tmp_path),
                    "mountPoint": str(tmp_path),
                    "writable": False,
                }
            ]
        },
    }
    code = f"""
import json
import os
import sys
from pathlib import Path
sys.path[:0] = [{str(REPO_ROOT / "src")!r}, {str(REPO_ROOT)!r}]
from tools import molt_dev as drv
from tools import molt_dev_retire
from tools import linux_vm

wt = Path(sys.argv[1])
dropped = Path({str(dropped)!r})


class Host(linux_vm.LocalHost):
    calls = []

    def which(self, name):
        assert name == "limactl", name
        return "/fixture/limactl"

    def start_service(self, argv, **kwargs):
        raise AssertionError(f"retirement must never start a service: {{argv}}")

    def run(self, argv, *, timeout, env_overrides=None):
        args = list(argv)
        assert timeout > 0 and env_overrides is None
        self.calls.append(args)
        if args == ["limactl", "list", "--json"]:
            assert len(self.calls) == 1
            return linux_vm.Completed(0, json.dumps({row!r}), "")
        if args == list(linux_vm.LSOF_VZ_FILES):
            assert len(self.calls) in (2, 4)
            lines = ["p77", "ccom.apple.Virtualization.VirtualMachine",
                     "f3", "tREG", "s1", "k1", "n{instance / "disk"}"]
            if not dropped.exists():
                lines += ["f9", "tREG", "s4096", "k1", "n{wt / "README.md"}"]
            return linux_vm.Completed(0, "\\n".join(lines), "")
        assert len(self.calls) == 3
        assert args[:9] == ["limactl", "shell", "--workdir", "/", "vm",
                           "sudo", "-n", "sh", "-c"], args
        assert len(args) == 10 and args[9].startswith(
            "echo 2 > /proc/sys/vm/drop_caches && stat -- "), args
        dropped.write_text(str(wt.exists()), encoding="utf-8")
        return linux_vm.Completed(0, "", "")


linux_vm.LocalHost = Host
release = molt_dev_retire._release_guest_caches
pending = []
if {release_timing!r} == "omitted":
    molt_dev_retire._release_guest_caches = lambda *args: None
elif {release_timing!r} == "after":
    molt_dev_retire._release_guest_caches = lambda *args: pending.append(args)


class RefuseAfterRemoval:
    armed = False

    def find_spec(self, name, path=None, target=None):
        if self.armed and name.split(".")[0] in ("molt", "tools"):
            raise ImportError(f"imported {{name}} after the worktree was removed")
        return None


guard = RefuseAfterRemoval()
sys.meta_path.insert(0, guard)
real = molt_dev_retire._run_fast_captured_command


def run(command, **kwargs):
    result = real(command, **kwargs)
    if command[3:5] == ["worktree", "remove"]:
        guard.armed = True
    return result


molt_dev_retire._run_fast_captured_command = run
os.chdir(wt)
rc = molt_dev_retire.cleanup_worktree(
    drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
)
assert rc == 0 and not wt.exists()
for args in pending:
    release(*args)
assert dropped.exists() and dropped.read_text(encoding="utf-8") == "True", \
    "VM release must precede worktree removal"
assert len(Host.calls) == 4
"""
    completed = run_guarded_test_process(
        [sys.executable, "-c", code, str(wt)],
        cwd=tmp_path,
        env={**os.environ, "LIMA_HOME": str(lima_home)},
        timeout=180,
    )
    if release_timing != "before":
        assert completed.returncode != 0
        assert "AssertionError: VM release must precede worktree removal" in (
            completed.stderr
        ), completed.stderr
        return
    assert completed.returncode == 0, completed.stderr
    assert not wt.exists()
    assert f"vm released its pins of {wt}: 1 host files released" in (
        completed.stderr
    ), completed.stderr
    assert dropped.read_text(encoding="utf-8") == "True", "release before removal"


@pytest.mark.parametrize(
    "error",
    [
        ImportError("tools.linux_vm"),
        ValueError("bad definition"),
        TypeError("bad Lima JSON"),
        KeyError("dir"),
        RuntimeError("limactl list failed"),
        OSError("lsof"),
        subprocess.TimeoutExpired("lsof", 60),
    ],
    ids=lambda error: type(error).__name__,
)
def test_the_guest_release_never_aborts_a_cleanup(
    drv, origin_and_clone, tmp_path, monkeypatch, capsys, error
):
    """B5: whatever the release raises, the checkout is still removed."""
    from tools import linux_vm

    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_release_raises")

    def fail(*_args, **_kwargs):
        raise error

    if isinstance(error, ImportError):
        # `from tools import linux_vm` itself fails.
        import tools

        monkeypatch.delattr(tools, "linux_vm")
        monkeypatch.setitem(sys.modules, "tools.linux_vm", None)
    else:
        monkeypatch.setattr(linux_vm, "release_for_paths", fail)
    rc = molt_dev_retire.cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_OK
    assert not wt.exists()
    assert f"Linux VMs may keep files of {wt}" in capsys.readouterr().err


def test_failed_worktree_removal_keeps_the_checkouts_state(
    drv, origin_and_clone, tmp_path, monkeypatch, isolated_checkout_artifacts
):
    from molt import checkout_state

    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_state_kept")
    ext = isolated_checkout_artifacts
    target, uv_environment = _shared_checkout_state(ext, wt)
    real = molt_dev_retire._run_fast_captured_command

    def refuse_removal(command, **kwargs):
        if command[3:5] == ["worktree", "remove"]:
            return subprocess.CompletedProcess(command, 1, "", "locked")
        return real(command, **kwargs)

    monkeypatch.setattr(molt_dev_retire, "_run_fast_captured_command", refuse_removal)
    rc = molt_dev_retire.cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_FAIL
    assert wt.is_dir() and target.is_dir() and uv_environment.is_dir()
    # The roots were bound while the checkout existed, so a later removal of
    # the checkout lets the disk guard reclaim them.
    assert checkout_state.read_checkout_owner(target).path == wt.resolve()


@pytest.mark.parametrize("retirement", ["cleanup", "drift-prune"])
@pytest.mark.parametrize("selection", ["foreign-managed", "caller-owned"])
@pytest.mark.parametrize("removal_succeeds", [False, True])
def test_retirement_binds_complete_checkout_inventory_independent_of_caller_selection(
    drv,
    origin_and_clone,
    tmp_path,
    monkeypatch,
    retirement,
    selection,
    removal_succeeds,
    isolated_checkout_artifacts,
):
    from molt import checkout_state
    from molt.build_state_layout import build_state_root
    from tools import drift_harvest, linux_vm

    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_retirement_inventory")
    guest_releases: list[tuple[tuple[Path, ...], bool]] = []

    def release_guests(paths, *, environment, host):
        guest_releases.append((tuple(paths), wt.exists()))
        return ()

    monkeypatch.setattr(linux_vm, "release_for_paths", release_guests)
    ext = isolated_checkout_artifacts
    target, uv_environment = _shared_checkout_state(ext, wt)
    session = target / "sessions" / "idle-session"
    session.mkdir(parents=True)
    (session / "payload.bin").write_bytes(b"session payload")
    target_control = build_state_root(
        project_root=wt, cargo_target=target, environment={"MOLT_EXT_ROOT": str(ext)}
    )
    session_control = build_state_root(
        project_root=wt, cargo_target=session, environment={"MOLT_EXT_ROOT": str(ext)}
    )
    for control in (target_control, session_control):
        control.mkdir(parents=True)
        (control / "payload.bin").write_bytes(b"control payload")

    foreign_target, foreign_uv = _shared_checkout_state(ext, main)
    foreign_control = build_state_root(
        project_root=main,
        cargo_target=foreign_target,
        environment={"MOLT_EXT_ROOT": str(ext)},
    )
    foreign_control.mkdir(parents=True)
    (foreign_control / "payload.bin").write_bytes(b"foreign control")
    with checkout_state.checkout_state_use(
        (
            (foreign_target, checkout_state.CheckoutOwner(main, "cargo-target")),
            (foreign_uv, checkout_state.CheckoutOwner(main, "python-environment")),
            (
                foreign_control,
                checkout_state.CheckoutOwner(foreign_target, "build-control"),
            ),
        )
    ):
        pass
    operator_target = tmp_path / "caller-owned-output"
    operator_target.mkdir()
    (operator_target / "payload.bin").write_bytes(b"caller owned")
    selected_target = (
        foreign_target if selection == "foreign-managed" else operator_target
    )
    environment = ProcessEnvironment(os.environ).updated(
        {
            "MOLT_EXT_ROOT": str(ext),
            "CARGO_TARGET_DIR": str(selected_target),
            "UV_PROJECT_ENVIRONMENT": str(foreign_uv),
            "MOLT_BUILD_STATE_DIR": str(foreign_control),
            "MOLT_SESSION_ID": "another-checkouts-session",
        }
    )
    for name in (
        "MOLT_EXT_ROOT",
        "CARGO_TARGET_DIR",
        "UV_PROJECT_ENVIRONMENT",
        "MOLT_BUILD_STATE_DIR",
        "MOLT_SESSION_ID",
    ):
        monkeypatch.setenv(name, environment[name])

    def foreign_bytes():
        return {
            str(path): path.read_bytes()
            for root in (foreign_target, foreign_uv, foreign_control, operator_target)
            for path in root.rglob("*")
            if path.is_file()
        }

    before = foreign_bytes()
    if retirement == "cleanup":
        real = molt_dev_retire._run_fast_captured_command

        def remove(command, **kwargs):
            if not removal_succeeds and command[3:5] == ["worktree", "remove"]:
                return subprocess.CompletedProcess(
                    command, 1, "", "independent refusal"
                )
            return real(command, **kwargs)

        monkeypatch.setattr(molt_dev_retire, "_run_fast_captured_command", remove)
        rc = molt_dev_retire.cleanup_worktree(
            drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
        )
        assert rc == (drv.EXIT_OK if removal_succeeds else drv.EXIT_FAIL)
    else:
        real = drift_harvest._git

        def remove(args, **kwargs):
            if not removal_succeeds and args[:2] == ["worktree", "remove"]:
                return subprocess.CompletedProcess(args, 1, "", "independent refusal")
            return real(args, **kwargs)

        monkeypatch.setattr(drift_harvest, "_git", remove)
        monkeypatch.setattr(
            drift_harvest.sys,
            "argv",
            [
                "drift_harvest.py",
                # The repository is named on the command line; rebinding the
                # module's global once let this prune the real worktree list.
                "--repo",
                str(main),
                "--prune",
                "--no-fetch",
                "--fresh-hours",
                "0",
                "--now",
                str(time.time() + 3600),
                "--bundle",
                str(tmp_path / "signal.bundle"),
            ],
        )
        assert drift_harvest.main() == 0

    assert wt.exists() is not removal_succeeds
    roots = {
        target: (str(wt.resolve()), "cargo-target"),
        session: (str(wt.resolve()), "cargo-target"),
        uv_environment: (str(wt.resolve()), "python-environment"),
        target_control: (str(target.resolve()), "build-control"),
        session_control: (str(session.resolve()), "build-control"),
    }
    for root, (owner, kind) in roots.items():
        assert root.exists() is not removal_succeeds
        if not removal_succeeds:
            if kind == "python-environment":
                assert checkout_state.read_checkout_owner(root) == (
                    checkout_state.CheckoutOwner(Path(owner), kind)
                )
                assert not (root / checkout_state.OWNER_RECORD).exists()
            else:
                assert json.loads(
                    (root / checkout_state.OWNER_RECORD).read_text(encoding="utf-8")
                ) == {
                    "schema": "molt.state-root-owner.v1",
                    "owner": owner,
                    "kind": kind,
                }
    assert foreign_bytes() == before
    # Linux VMs release their pins of the checkout and its state roots once,
    # just before the removal, while they still exist (HF-463).
    [(released, checkout_present)] = guest_releases
    assert checkout_present
    assert {wt, target, uv_environment} <= set(released)
    assert foreign_target not in released


def test_cleanup_refuses_unpushed(drv, origin_and_clone, tmp_path):
    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_unpushed")
    _commit_file(wt, "local.py", "n = 1\n", "feat: unpushed local")
    git = drv.Git(wt)
    rc = molt_dev_retire.cleanup_worktree(
        git, wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_FAIL
    assert wt.exists()  # NOT removed


def test_cleanup_refuses_dirty_tracked(drv, origin_and_clone, tmp_path):
    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_dirty")
    # Modify a TRACKED file without committing -> dirty -> refuse.
    (wt / "README.md").write_text("seed\nlocal edit\n", encoding="utf-8")
    rc = molt_dev_retire.cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_FAIL
    assert wt.exists()


def test_molt_dev_default_ignore_globs_use_dirty_tree_policy_authority(drv):
    assert drv.DEFAULT_IGNORE_GLOBS is dirty_tree_policy.DEFAULT_DIRTY_TREE_IGNORE_GLOBS


def test_cleanup_refuses_runtime_generation_churn(drv, origin_and_clone, tmp_path):
    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_wasm")
    generation = wt / RUNTIME_GENERATION_MANIFEST
    generation.parent.mkdir(parents=True, exist_ok=True)
    generation.write_text("{}\n", encoding="utf-8")
    _git(wt, "add", "--", RUNTIME_GENERATION_MANIFEST)
    rc = molt_dev_retire.cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_FAIL
    assert wt.exists()


def test_cleanup_force_requires_matching_sha(drv, origin_and_clone, tmp_path):
    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_force")
    _commit_file(wt, "local.py", "n = 1\n", "feat: unpushed")
    git = drv.Git(wt)
    head = git.head_sha()
    # Wrong force sha -> refused.
    rc_wrong = molt_dev_retire.cleanup_worktree(
        git, wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha="0" * 40
    )
    assert rc_wrong == drv.EXIT_FAIL
    assert wt.exists()
    # Correct force sha (the real HEAD) -> abandon proceeds.
    rc_right = molt_dev_retire.cleanup_worktree(
        git, wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=head
    )
    assert rc_right == drv.EXIT_OK
    assert not wt.exists()


def test_cleanup_refuses_main_worktree(drv, origin_and_clone, tmp_path):
    # Targeting the MAIN worktree must be refused (this is for disposable ones).
    _origin, main_clone = origin_and_clone
    rc = molt_dev_retire.cleanup_worktree(
        drv.Git(main_clone),
        main_clone,
        "origin/main",
        drv.DEFAULT_IGNORE_GLOBS,
        force_sha=None,
    )
    assert rc == drv.EXIT_FAIL
    assert main_clone.exists()


# --------------------------------------------------------------------------
# Hazard 4: secure-wip captures staged + unstaged in one recovery commit
# --------------------------------------------------------------------------


def test_secure_wip_captures_staged_and_unstaged(drv, origin_and_clone):
    origin, work = origin_and_clone
    git = drv.Git(work)
    # A STAGED modification (index) ...
    (work / "README.md").write_text("seed\nstaged edit\n", encoding="utf-8")
    _git(work, "add", "--", "README.md")
    # ... and an UNSTAGED new tracked file added then edited without re-staging.
    (work / "new_tracked.py").write_text("x = 1\n", encoding="utf-8")
    _git(work, "add", "--", "new_tracked.py")
    (work / "new_tracked.py").write_text("x = 1\ny = 2\n", encoding="utf-8")
    # Now: README.md staged, new_tracked.py has staged+unstaged hunks.

    head_before = git.head_sha()
    ns = _ns(
        drv,
        repo=str(work),
        dry_run=False,
        message=None,
        ignore=None,
        include_untracked=False,
    )
    assert drv.cmd_secure_wip(ns) == drv.EXIT_OK
    head_after = git.head_sha()
    assert head_after != head_before

    # The single recovery commit must contain BOTH paths at their FULL content
    # (the unstaged y=2 hunk too), proving staged+unstaged were both captured.
    subject = _git(work, "log", "-1", "--format=%s").stdout.strip()
    assert subject.startswith(drv.WIP_MARKER)
    files = set(_git(work, "show", "--name-only", "--format=", "HEAD").stdout.split())
    assert "README.md" in files
    assert "new_tracked.py" in files
    blob = _git(work, "show", "HEAD:new_tracked.py").stdout
    assert "y = 2" in blob  # the UNSTAGED hunk made it in
    # Tree is clean afterward (everything was committed).
    assert git.status_porcelain() == []


def test_secure_wip_honors_ignore_set(drv, origin_and_clone):
    origin, work = origin_and_clone
    git = drv.Git(work)
    # A real tracked edit ...
    (work / "README.md").write_text("seed\nedit\n", encoding="utf-8")
    # ... plus explicitly operator-ignored scratch state that must be left behind.
    scratch = work / CUSTOM_IGNORED_PATH
    scratch.parent.mkdir(parents=True, exist_ok=True)
    scratch.write_text("scratch\n", encoding="utf-8")
    _git(work, "add", "--", CUSTOM_IGNORED_PATH)

    ns = _ns(
        drv,
        repo=str(work),
        dry_run=False,
        message="salvage",
        ignore=(CUSTOM_IGNORED_PATH,),
        include_untracked=False,
    )
    assert drv.cmd_secure_wip(ns) == drv.EXIT_OK
    files = set(_git(work, "show", "--name-only", "--format=", "HEAD").stdout.split())
    assert "README.md" in files
    assert CUSTOM_IGNORED_PATH not in files  # excluded
    # The ignored file is still pending (not swept into the recovery commit).
    remaining = {p for _xy, p in git.status_porcelain()}
    assert CUSTOM_IGNORED_PATH in remaining


def test_secure_wip_excludes_untracked_by_default(drv, origin_and_clone):
    origin, work = origin_and_clone
    git = drv.Git(work)
    (work / "README.md").write_text("seed\nedit\n", encoding="utf-8")
    (work / "scratch.txt").write_text("scratch\n", encoding="utf-8")  # untracked
    ns = _ns(
        drv,
        repo=str(work),
        dry_run=False,
        message=None,
        ignore=None,
        include_untracked=False,
    )
    assert drv.cmd_secure_wip(ns) == drv.EXIT_OK
    files = set(_git(work, "show", "--name-only", "--format=", "HEAD").stdout.split())
    assert "README.md" in files
    assert "scratch.txt" not in files
    # The untracked file remains untracked (not captured, not deleted).
    assert (work / "scratch.txt").exists()
    remaining = {p for _xy, p in git.status_porcelain()}
    assert "scratch.txt" in remaining


def test_secure_wip_dry_run_does_not_commit(drv, origin_and_clone):
    origin, work = origin_and_clone
    git = drv.Git(work)
    (work / "README.md").write_text("seed\nedit\n", encoding="utf-8")
    head_before = git.head_sha()
    ns = _ns(
        drv,
        repo=str(work),
        dry_run=True,
        message=None,
        ignore=None,
        include_untracked=False,
    )
    assert drv.cmd_secure_wip(ns) == drv.EXIT_OK
    assert git.head_sha() == head_before  # no commit made


def test_secure_wip_clean_tree_is_noop(drv, origin_and_clone):
    origin, work = origin_and_clone
    git = drv.Git(work)
    head_before = git.head_sha()
    ns = _ns(
        drv,
        repo=str(work),
        dry_run=False,
        message=None,
        ignore=None,
        include_untracked=False,
    )
    assert drv.cmd_secure_wip(ns) == drv.EXIT_OK
    assert git.head_sha() == head_before


# --------------------------------------------------------------------------
# Hazard 7: python-oracle pins and verifies the interpreter version
# --------------------------------------------------------------------------


def test_python_oracle_pins_current_version(drv):
    # The running interpreter's own version must resolve+verify successfully.
    mm = "%d.%d" % sys.version_info[:2]
    request = drv.python_interpreter.PythonInterpreterRequest(*sys.version_info[:2])
    from molt.python_environment_identity import PythonIdentityVerifier

    with PythonIdentityVerifier() as verifier:
        resolved = drv.python_interpreter.resolve_python_interpreter(
            request,
            prefer_uv=False,
            prefer_current=True,
            verifier=verifier,
        )
    assert resolved.major_minor == mm
    assert resolved.executable


def test_python_oracle_refuses_unavailable_version(drv):
    # An impossible version must raise LOUDLY (never silently fall back).
    with pytest.raises(ValueError, match="unsupported Python target version"):
        drv.python_interpreter.parse_target_python_version("2.0")


def test_python_oracle_rejects_bad_format(drv):
    with pytest.raises(ValueError, match="invalid Python target version"):
        drv.python_interpreter.parse_target_python_version("not-a-version")


def test_python_interpreter_identity_excludes_wrong_minor(drv):
    # The current interpreter does NOT report a wrong version.
    from molt.python_environment_identity import PythonIdentityVerifier

    with PythonIdentityVerifier() as verifier:
        resolved = drv.python_interpreter.probe_python_command(
            [sys.executable], verifier=verifier
        )
    assert resolved.major_minor != "9.99"


# --------------------------------------------------------------------------
# Hazard 8: content-marker verification within integrate
# --------------------------------------------------------------------------


def test_marker_parse_and_check(drv, tmp_path):
    f = tmp_path / "probe.txt"
    f.write_text("hello SETATTR world\n", encoding="utf-8")
    m_exists = drv.Marker.parse("exists:probe.txt")
    ok, _ = m_exists.check(tmp_path)
    assert ok is True
    m_contains = drv.Marker.parse("contains:probe.txt::SETATTR")
    ok, _ = m_contains.check(tmp_path)
    assert ok is True
    m_absent = drv.Marker.parse("contains:probe.txt::NOPE")
    ok, _ = m_absent.check(tmp_path)
    assert ok is False
    m_missing = drv.Marker.parse("exists:nope.txt")
    ok, _ = m_missing.check(tmp_path)
    assert ok is False


def test_marker_parse_rejects_bad_spec(drv):
    with pytest.raises(drv.DriverError):
        drv.Marker.parse("contains:onlypath-no-needle")
    with pytest.raises(drv.DriverError):
        drv.Marker.parse("garbage:foo")


def test_integrate_marker_mismatch_fails(drv, origin_and_clone):
    # A declared marker that is NOT satisfied post-rebase halts integrate before
    # push, LOUDLY (a DriverError — the fail-loud contract). We isolate the
    # marker step before the committed policy runs.
    origin, work = origin_and_clone
    _commit_file(work, "impl.py", "def f():\n    return 1\n", "feat: impl")
    ns = _integrate_ns(
        drv,
        repo=str(work),
        marker=["contains:impl.py::NONEXISTENT_TOKEN"],
        no_push=True,
    )
    with pytest.raises(drv.DriverError) as exc:
        drv.cmd_integrate(ns)
    assert "content marker" in str(exc.value)
    # And origin is untouched (no push happened).
    origin_main = _git(work, "rev-parse", "origin/main").stdout.strip()
    seed = _git(work, "rev-list", "--max-parents=0", "origin/main").stdout.strip()
    assert origin_main == seed


def test_integrate_marker_mismatch_via_main_returns_fail(drv, origin_and_clone):
    # The same failure through the top-level main() must translate the
    # DriverError into EXIT_FAIL (the CI-facing exit-code contract).
    origin, work = origin_and_clone
    _commit_file(work, "impl.py", "def f():\n    return 1\n", "feat: impl")
    rc = drv.main(
        [
            "integrate",
            "--repo",
            str(work),
            "--candidate-controller",
            "--no-push",
            "--marker",
            "contains:impl.py::NONEXISTENT_TOKEN",
        ]
    )
    assert rc == drv.EXIT_FAIL


# --------------------------------------------------------------------------
# Stable change ids: citations resolve to the landed commit ids
# --------------------------------------------------------------------------

_FEATURE_CHANGE = "I" + "3a" * 20


def _commit_with_change_id(repo: Path, rel: str, content: str, change: str) -> str:
    target = repo / rel
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")
    _git(repo, "add", "--", rel)
    _git(repo, "commit", "-q", "--no-verify", "-m", f"feature\n\nChange-Id: {change}")
    return _git(repo, "rev-parse", "HEAD").stdout.strip()


def _advance_origin(origin: Path, work: Path) -> str:
    other = work.parent / "upstream"
    _git(work.parent, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "upstream@example.test")
    _git(other, "config", "user.name", "Upstream")
    tip = _commit_file(other, "other.py", "other = 1\n", "upstream")
    _git(other, "push", "-q", "origin", "main")
    return tip


def test_integrate_resolves_change_citations_to_rebased_commits(drv, origin_and_clone):
    origin, work = origin_and_clone
    source = _commit_with_change_id(work, "feature.py", "x = 1\n", _FEATURE_CHANGE)
    # Bytes outside the citation survive exactly, including CRLF line ends.
    ledger = b"| HF-1 | fixed in change:" + _FEATURE_CHANGE[:11].encode() + b" |\r\n"
    (work / "docs").mkdir()
    (work / "docs" / "ledger.md").write_bytes(ledger)
    # Citation-shaped data outside docs/*.md is never rewritten.
    (work / "tests").mkdir()
    (work / "tests" / "data.py").write_text(
        f'TOKEN = "change:{_FEATURE_CHANGE[:11]}"\n', encoding="utf-8"
    )
    _git(work, "add", "--", "docs/ledger.md", "tests/data.py")
    _git(work, "commit", "-q", "--no-verify", "-m", "ledger")
    _advance_origin(origin, work)

    assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work))) == drv.EXIT_OK

    landed = _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
    assert landed == _git(work, "rev-parse", "HEAD").stdout.strip()
    feature = _git(work, "rev-parse", f"{landed}^").stdout.strip()
    assert feature != source, "the rebase gave the cited commit a new id"
    assert (
        _git(
            work, "log", "-1", "--format=%(trailers:key=Change-Id,valueonly)", feature
        ).stdout.strip()
        == _FEATURE_CHANGE
    )
    short = _git(work, "rev-parse", "--short=9", feature).stdout.strip()
    blob = run_guarded_test_process(
        ["git", "-C", str(work), "cat-file", "blob", f"{landed}:docs/ledger.md"],
        text=False,
        check=True,
    )
    assert blob.stdout == b"| HF-1 | fixed in " + short.encode() + b" |\r\n"
    assert (
        f"change:{_FEATURE_CHANGE[:11]}"
        in _git(work, "show", f"{landed}:tests/data.py").stdout
    )


@pytest.mark.parametrize(
    ("citation", "message"),
    [
        ("change:Ideadbeef00", "names no commit of this landing"),
        ("change:I" + "3a" * 4, "names several changes"),
        ("change:I" + "5b" * 5, "cites the landing's last commit"),
    ],
)
def test_integrate_refuses_unresolved_change_citations_before_gates(
    drv, origin_and_clone, capsys, citation, message
):
    _, work = origin_and_clone
    _commit_with_change_id(work, "a.py", "a = 1\n", _FEATURE_CHANGE)
    _commit_with_change_id(work, "b.py", "b = 1\n", "I" + "3a" * 4 + "c" * 32)
    (work / "docs").mkdir()
    (work / "docs" / "ledger.md").write_text(f"cites {citation}\n", encoding="utf-8")
    _git(work, "add", "--", "docs/ledger.md")
    _git(
        work, "commit", "-q", "--no-verify", "-m", "ledger\n\nChange-Id: I" + "5b" * 20
    )
    origin_before = _git(work, "ls-remote", "origin", "refs/heads/main").stdout
    head_before = _git(work, "rev-parse", "HEAD").stdout.strip()

    with pytest.raises(drv.DriverError, match=message) as caught:
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work)))

    assert "executed=0" in str(caught.value)
    captured = capsys.readouterr()
    assert "integration-policy" not in captured.out + captured.err, "no gate may run"
    assert _git(work, "rev-parse", "HEAD").stdout.strip() == head_before
    assert _git(work, "ls-remote", "origin", "refs/heads/main").stdout == origin_before


def test_failed_landing_restores_unresolved_change_citations(drv, origin_and_clone):
    # The resolved form names commit ids the next rebase changes; a landing
    # that does not publish must leave the change: form for the next attempt.
    _, work = origin_and_clone
    (work / "tools" / "proof_plan.toml").write_text(
        'always = ["false"]\n', encoding="utf-8"
    )
    _git(work, "add", "--", "tools/proof_plan.toml")
    _git(work, "commit", "-q", "--no-verify", "-m", "a failing gate")
    _commit_with_change_id(work, "a.py", "a = 1\n", _FEATURE_CHANGE)
    (work / "docs").mkdir()
    citation = f"fixed in change:{_FEATURE_CHANGE[:9]}\n"
    (work / "docs" / "ledger.md").write_text(citation, encoding="utf-8")
    _git(work, "add", "--", "docs/ledger.md")
    _git(work, "commit", "-q", "--no-verify", "-m", "ledger")
    head = _git(work, "rev-parse", "HEAD").stdout.strip()

    with pytest.raises(GateFailure):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work)))

    assert _git(work, "rev-parse", "HEAD").stdout.strip() == head
    assert (work / "docs" / "ledger.md").read_text(encoding="utf-8") == citation


def test_integrate_dry_run_reports_change_citations_without_amending(
    drv, origin_and_clone, capsys
):
    _, work = origin_and_clone
    feature = _commit_with_change_id(work, "a.py", "a = 1\n", _FEATURE_CHANGE)
    (work / "docs").mkdir()
    (work / "docs" / "ledger.md").write_text(
        f"fixed in change:{_FEATURE_CHANGE[:9]}\n", encoding="utf-8"
    )
    _git(work, "add", "--", "docs/ledger.md")
    _git(work, "commit", "-q", "--no-verify", "-m", "ledger")
    head = _git(work, "rev-parse", "HEAD").stdout.strip()

    assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work), dry_run=True)) == 0

    short = _git(work, "rev-parse", "--short=9", feature).stdout.strip()
    assert f"change:{_FEATURE_CHANGE[:9]} -> {short}" in capsys.readouterr().err
    assert _git(work, "rev-parse", "HEAD").stdout.strip() == head
    assert "change:" in (work / "docs" / "ledger.md").read_text(encoding="utf-8")


def test_integrate_marker_match_passes(drv, origin_and_clone):
    origin, work = origin_and_clone
    _commit_file(work, "impl.py", "def f():\n    return 1\n", "feat: impl")
    ns = _integrate_ns(
        drv,
        repo=str(work),
        marker=["exists:impl.py", "contains:impl.py::return 1"],
        no_push=True,  # stop before push so the test stays local
    )
    assert drv.cmd_integrate(ns) == drv.EXIT_OK


# --------------------------------------------------------------------------
# Hazard 9: probe (file size+mtime, pid liveness)
# --------------------------------------------------------------------------


def test_probe_file_and_pid(drv, tmp_path):
    f = tmp_path / "x.log"
    f.write_text("data\n", encoding="utf-8")
    info = drv.probe_path(f)
    assert info["exists"] is True
    assert info["size"] == f.stat().st_size
    assert "mtime" in info
    live = drv.probe_pid(os.getpid())
    assert live["alive"] is True
    # A pid that is almost certainly dead.
    dead = drv.probe_pid(2_000_000_000)
    assert dead["alive"] is False


def test_probe_missing_file_is_nonzero(drv, tmp_path):
    ns = _ns(drv, file=[str(tmp_path / "nope.log")], pid=None, json=False)
    assert drv.cmd_probe(ns) == drv.EXIT_FAIL


def test_probe_live_pid_is_zero(drv):
    ns = _ns(drv, file=None, pid=[os.getpid()], json=False)
    assert drv.cmd_probe(ns) == drv.EXIT_OK


def test_probe_requires_target(drv):
    ns = _ns(drv, file=None, pid=None, json=False)
    with pytest.raises(drv.DriverError) as exc:
        drv.cmd_probe(ns)
    assert exc.value.code == drv.EXIT_USAGE


# --------------------------------------------------------------------------
# integrate: end-to-end gate selection + halt-on-failure, and full push path
# --------------------------------------------------------------------------


def test_integrate_runs_selected_gate_and_halts_on_failure(
    drv, origin_and_clone, tmp_path, capsys
):
    # A selected source check must stop the real driver before a later build
    # gate can execute or integration can push. The sentinel uses no compiler.
    origin, work = origin_and_clone
    _commit_file(work, "src/molt/m.py", "x = 1\n", "feat: py change")
    _commit_file(
        work,
        "tools/proof_plan.toml",
        """
always = []

[[rule]]
name = "source-check"
globs = ["src/molt/**/*.py"]
gates = ["exit 3"]

[[rule]]
name = "build-consumer"
globs = ["src/molt/**/*.py"]
gates = ["echo build-started > build-started.txt"]
""",
        "failing source check before build consumer",
    )
    ns = drv.build_parser().parse_args(
        ["integrate", "--repo", str(work), "--candidate-controller", "--no-push"]
    )
    assert not drv.molt_dev_controller.runs_from_pinned_controller(ns)
    # A failing gate halts integrate BEFORE push, as a candidate failure.
    with pytest.raises(GateFailure) as exc:
        drv.cmd_integrate(ns)
    assert re.search(r"gate failed \(exit 3\) after \d+\.\d s: exit 3", str(exc.value))
    assert re.search(r"gates: 1 in \d+\.\d s; slowest:", str(exc.value))
    assert str(exc.value).count("exit 3") == 3  # status, failed command, timing row
    assert not (work / "build-started.txt").exists()
    # Nothing was pushed (origin unchanged).
    origin_main = _git(work, "rev-parse", "origin/main").stdout.strip()
    seed = _git(work, "rev-list", "--max-parents=0", "origin/main").stdout.strip()
    assert origin_main == seed  # still just the seed commit
    events = [
        json.loads(line)
        for line in capsys.readouterr().out.splitlines()
        if line.startswith('{"schema": "molt.integration-progress.v1"')
    ]
    assert events and {event["controller"] for event in events} == {"candidate"}


def _wait_for(marker: str, *, then: str) -> str:
    """A gate that needs another gate to run beside it.

    Serially it waits forever, and the session's own deadline ends the test:
    a wall-clock bound here failed it on a loaded host.
    """
    return _integration_python(
        "import pathlib, sys, time\n"
        f"while not pathlib.Path({marker!r}).exists():\n"
        "    time.sleep(0.05)\n" + then
    )


def kept_gate_stdout(out: str) -> list[str]:
    """Each finished gate's stdout, read from the file its event names, in
    event order. A passing gate's output goes only to that file."""
    events = [
        json.loads(line)
        for line in out.splitlines()
        if line.startswith("{") and '"gate_finished"' in line
    ]
    assert all("output_error" not in event for event in events), events
    return [Path(event["stdout_path"]).read_text(encoding="utf-8") for event in events]


def test_integrate_always_gates_run_together_and_report_in_declared_order(
    drv, origin_and_clone, monkeypatch, capfd, borrowed_gate_python
):
    _, work = origin_and_clone
    marker = str(work.parent / "second-ran")
    first = _wait_for(marker, then="print('first' + '-gate-output')")
    second = _integration_python(
        f"import pathlib; pathlib.Path({marker!r}).touch(); print('second' + '-gate-output')"
    )
    _commit_integration_policy(work, [first, second])
    monkeypatch.setattr(drv, "concurrent_gate_limit", lambda: 2)
    assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True)) == 0
    output = capfd.readouterr()
    # The first gate finished last, yet its event comes first, and each
    # event names the file that holds that gate's own output.
    finished = [
        json.loads(line)["gate_ordinal"]
        for line in output.out.splitlines()
        if line.startswith("{") and '"gate_finished"' in line
    ]
    assert finished == [1, 2]
    assert kept_gate_stdout(output.out) == [
        "first-gate-output\n",
        "second-gate-output\n",
    ]
    assert "first-gate-output" not in output.out, (
        "a passing gate's output stays in its file"
    )
    # The proof executor ran the block and left a landing receipt.
    summary = next(
        line for line in output.err.splitlines() if "gate block: 2 gates in" in line
    )
    receipt = json.loads(
        Path(summary.rsplit("receipt ", 1)[1]).read_text(encoding="utf-8")
    )
    assert receipt["status"] == "success"
    assert receipt["execution"]["peak_active_commands"] == 2
    assert [row["status"] for row in receipt["commands"]] == ["success", "success"]
    assert all(row["duration_seconds"] > 0 for row in receipt["commands"])


@pytest.mark.parametrize("failure_ordinal", [1, 2], ids=["earlier", "later"])
def test_integrate_failure_cancels_live_sibling_and_skips_pending_gate(
    drv, origin_and_clone, monkeypatch, capfd, borrowed_gate_python, failure_ordinal
):
    _, work = origin_and_clone
    ready = work.parent / "sibling-ready"
    grandchild_ready = work.parent / "grandchild-ready"
    forbidden = work.parent / "third-started"
    marker = hung_child_marker()
    grandchild = (
        "from pathlib import Path\n"
        f"Path({str(grandchild_ready)!r}).touch()\n" + HUNG_CHILD_SOURCE
    )
    # Neither process exits naturally. The failing sibling waits until both
    # exist, so returning requires the actual guard's cancellation and closure.
    hung = shlex.join(
        [
            sys.executable,
            "-c",
            "import pathlib, subprocess, sys, time\n"
            f"subprocess.Popen([sys.executable, '-c', {grandchild!r}, {marker!r}])\n"
            f"while not pathlib.Path({str(grandchild_ready)!r}).exists():\n"
            "    time.sleep(0.01)\n"
            f"pathlib.Path({str(ready)!r}).touch()\n" + HUNG_CHILD_SOURCE,
            marker,
        ]
    )
    failure = _wait_for(
        str(ready), then="print('independent exit six', flush=True); sys.exit(6)"
    )
    first_two = [failure, hung] if failure_ordinal == 1 else [hung, failure]
    _commit_integration_policy(
        work,
        [
            *first_two,
            _integration_python(
                f"import pathlib; pathlib.Path({str(forbidden)!r}).touch()"
            ),
        ],
    )
    monkeypatch.setattr(drv, "concurrent_gate_limit", lambda: 2)
    try:
        with pytest.raises(GateFailure, match=r"gate failed \(exit 6\)"):
            drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))
        still_running = processes_carrying(marker)
    finally:
        stop_test_processes_carrying(marker)
    assert ready.exists() and grandchild_ready.exists()
    assert still_running == [], "the cancelled gate or grandchild still runs"
    assert not forbidden.exists()
    common = Path(
        _git(
            work, "rev-parse", "--path-format=absolute", "--git-common-dir"
        ).stdout.strip()
    )
    head = _git(work, "rev-parse", "HEAD").stdout.strip()
    receipt = json.loads(
        (common / "molt-integrate/receipts" / f"{head}.gates.json").read_text(
            encoding="utf-8"
        )
    )
    assert receipt["execution"]["failure_policy"] == "cancel_remaining"
    assert receipt["execution"]["failure_trigger"] == {
        "command_id": f"local.always.{failure_ordinal}",
        "failure_scope": "partition",
        "reason": "exit 6",
    }
    records = {row["id"]: row for row in receipt["commands"]}
    failed = records[f"local.always.{failure_ordinal}"]
    cancelled = records[f"local.always.{3 - failure_ordinal}"]
    assert (failed["status"], failed["returncode"]) == ("failure", 6)
    assert (cancelled["status"], cancelled["returncode"]) == ("cancelled", 137)
    assert records["local.always.3"]["status"] == "skipped"
    assert Path(failed["stdout_path"]).read_text(encoding="utf-8") == (
        "independent exit six\n"
    )
    assert "independent exit six" in capfd.readouterr().err


def test_integrate_cancels_later_gates_after_an_earlier_failure(
    drv, origin_and_clone, monkeypatch, borrowed_gate_python
):
    _, work = origin_and_clone
    started = str(work.parent / "second-started")
    marker = hung_child_marker()
    # The second gate never ends by itself: only its cancellation ends it.
    hung = shlex.join(
        [
            sys.executable,
            "-c",
            f"import pathlib\npathlib.Path({started!r}).touch()\n" + HUNG_CHILD_SOURCE,
            marker,
        ]
    )
    _commit_integration_policy(work, [_wait_for(started, then="sys.exit(4)"), hung])
    monkeypatch.setattr(drv, "concurrent_gate_limit", lambda: 2)
    try:
        with pytest.raises(GateFailure, match=r"gate failed \(exit 4\)"):
            drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))
        still_running = processes_carrying(marker)
    finally:
        stop_test_processes_carrying(marker)
    assert still_running == [], "the cancelled gate still runs"


@pytest.mark.parametrize("cause", ["gate exit", "outside writer"])
def test_gate_block_blames_the_candidate_only_for_a_gate_exit(drv, tmp_path, cause):
    # The landing queue bisects on CandidateFailure. A gate's nonzero exit is
    # one; a runner stop (here the executor sees an outside writer dirty the
    # source and stops scheduling) must stay a plain DriverError.

    changes: list[tuple[str, ...]] = [()]
    ran: list[object] = []

    def run_captured_gate(command, repo, environment, cancelled, streams=None):
        ran.append(command)
        if cause == "outside writer":
            changes[0] = ("?? stray.txt",)
            return _gate_outcome(0, "", "", None)
        return _gate_outcome(3, "", "", None)

    class Progress:
        def emit(self, event, **details):
            pass

    environment = ProcessEnvironment(os.environ)
    runtime = molt_dev_gates.GateRuntime(
        repo=tmp_path,
        environments={("python",): GateEnvironment(environment, None, False)},
        cargo_outputs={},
        capacity_requirements={},
        progress=Progress(),
        check_source=lambda: None,
        source_changes=lambda: changes[0],
        source_identity=lambda: {"commit": "0" * 40, "tree": "1" * 40},
        policy_text="",
        executor_max_workers=None,
        resource_limits={},
        toolchains={},
        receipt_path=tmp_path / "receipt.json",
        output_dir=tmp_path / "receipt.d" / "run",
        run_captured_gate=run_captured_gate,
        observe_capacity=lambda *args, **kwargs: (),
        say=lambda message: None,
        ok=lambda message: None,
    )
    gates = [
        SelectedGate(
            command=("gate", str(index)), toolchains=("python",), concurrent=True
        )
        for index in (1, 2)
    ]
    with pytest.raises(drv.DriverError) as exc:
        molt_dev_gates.run_selected_gates(gates, runtime, [], host_limit=lambda: 1)
    if cause == "gate exit":
        assert isinstance(exc.value, GateFailure)
        assert "gate failed (exit 3)" in str(exc.value)
    else:
        assert not isinstance(exc.value, CandidateFailure)
        assert "gate block stopped" in str(exc.value)
        assert "stray.txt" in str(exc.value)
    # One gate at a time: the second never starts after either cause.
    assert ran == [("gate", "1")]


def test_gate_block_keeps_every_stream_and_shows_a_failure_briefly(
    drv, tmp_path, capsys
):
    # A failure must say why, and a passing gate must cost the caller nothing:
    # every stream is kept beside the receipt, which names it, and the
    # terminal gets the failed gate's bounded tail and the file's path.

    huge = "x" * 34_000
    streams = {
        ("gate", "1"): _gate_outcome(0, "chatty\n" * 1000, "", None),
        ("gate", "2"): _gate_outcome(
            1, "", f"{huge}\nruff format failed: timed out after 60 s\n", None
        ),
    }

    class Progress:
        def emit(self, event, **details):
            pass

    receipt = tmp_path / "receipt.json"
    runtime = molt_dev_gates.GateRuntime(
        repo=tmp_path,
        environments={
            ("python",): GateEnvironment(ProcessEnvironment(os.environ), None, False)
        },
        cargo_outputs={},
        capacity_requirements={},
        progress=Progress(),
        check_source=lambda: None,
        source_changes=lambda: (),
        source_identity=lambda: {"commit": "0" * 40, "tree": "1" * 40},
        policy_text="",
        executor_max_workers=None,
        resource_limits={},
        toolchains={},
        receipt_path=receipt,
        output_dir=tmp_path / "receipt.d" / "run",
        run_captured_gate=_writing_gate(streams),
        observe_capacity=lambda *args, **kwargs: (),
        say=lambda message: None,
        ok=lambda message: None,
    )
    gates = [
        SelectedGate(
            command=("gate", str(index)), toolchains=("python",), concurrent=True
        )
        for index in (1, 2)
    ]
    with pytest.raises(GateFailure, match=r"gate failed \(exit 1\)"):
        molt_dev_gates.run_selected_gates(gates, runtime, [], host_limit=lambda: 1)

    records = json.loads(receipt.read_text(encoding="utf-8"))["commands"]
    kept = {
        tuple(r["argv"]): (Path(r["stdout_path"]), Path(r["stderr_path"]))
        for r in records
    }
    for command, outcome in streams.items():
        stdout, stderr = kept[command]
        assert stdout.read_text(encoding="utf-8") == outcome.stdout
        assert stderr.read_text(encoding="utf-8") == outcome.stderr
    shown = capsys.readouterr().err
    assert f"full stderr: {kept[('gate', '2')][1]}" in shown
    assert "ruff format failed: timed out after 60 s" in shown
    assert "... (34000 chars)" in shown and huge not in shown
    assert "chatty" not in shown and len(shown) < 4096


def _gate_runtime_fixture(molt_dev_gates, tmp_path, outcomes, *, output_dir):
    class Progress:
        def emit(self, event, **details):
            pass

    return molt_dev_gates.GateRuntime(
        repo=tmp_path,
        environments={
            ("python",): GateEnvironment(ProcessEnvironment(os.environ), None, False)
        },
        cargo_outputs={},
        capacity_requirements={},
        progress=Progress(),
        check_source=lambda: None,
        source_changes=lambda: (),
        source_identity=lambda: {"commit": "0" * 40, "tree": "1" * 40},
        policy_text="",
        executor_max_workers=None,
        resource_limits={},
        toolchains={},
        receipt_path=tmp_path / "receipt.json",
        output_dir=output_dir,
        run_captured_gate=_writing_gate(outcomes),
        observe_capacity=lambda *args, **kwargs: (),
        say=lambda message: None,
        ok=lambda message: None,
    )


def test_gate_adapter_preserves_real_outcomes_after_shared_cancellation(
    tmp_path, monkeypatch
):
    from dataclasses import replace
    import threading

    # Use the real executor, with all four runners admitted before one fails.
    # Observe its actual shared event so no sleep/order guess supplies the race.
    entered = threading.Barrier(4)
    events = {}
    execute = molt_dev_gates.proof_executor.execute_commands

    def observe_executor(*args, **kwargs):
        run = kwargs["_run_command"]

        def observe(plan, command, cancel):
            events[command.gate.command] = cancel
            return run(plan, command, cancel)

        return execute(*args, **{**kwargs, "_run_command": observe})

    monkeypatch.setattr(
        molt_dev_gates.proof_executor, "execute_commands", observe_executor
    )
    outcomes = {
        ("race", "1"): _gate_outcome(7, "independent seven\n", ""),
        ("race", "2"): _gate_outcome(6, "trigger six\n", ""),
        ("race", "3"): _gate_outcome(137, "", "closed cancellation\n", cancelled=True),
        ("race", "4"): _gate_outcome(0, "completed successfully\n", ""),
    }

    def run(command, repo, environment, cancelled, streams=None):
        entered.wait(timeout=30)
        if command != ("race", "2"):
            assert events[command].wait(timeout=30), "executor did not cancel siblings"
            assert cancelled()
        outcome = outcomes[command]
        if streams is not None:
            streams[0].write_text(outcome.stdout, encoding="utf-8")
            streams[1].write_text(outcome.stderr, encoding="utf-8")
        return outcome

    runtime = replace(
        _gate_runtime_fixture(
            molt_dev_gates, tmp_path, {}, output_dir=tmp_path / "gate-output"
        ),
        run_captured_gate=run,
    )
    gates = [
        SelectedGate(command=command, toolchains=("python",), concurrent=True)
        for command in outcomes
    ]
    with pytest.raises(GateFailure, match=r"gate failed \(exit 6\)"):
        molt_dev_gates.run_selected_gates(gates, runtime, [], host_limit=lambda: 4)
    receipt = json.loads((tmp_path / "receipt.json").read_text(encoding="utf-8"))
    assert receipt["execution"]["failure_trigger"] == {
        "command_id": "local.always.2",
        "failure_scope": "partition",
        "reason": "exit 6",
    }
    assert [row["status"] for row in receipt["commands"]] == [
        "failure",
        "failure",
        "cancelled",
        "success",
    ]
    assert [row["returncode"] for row in receipt["commands"]] == [7, 6, 137, 0]
    assert [row["guard_outcome"]["cancelled"] for row in receipt["commands"]] == [
        False,
        False,
        True,
        False,
    ]
    assert all(
        row["guard_outcome"]["descendants_closed"] is True
        for row in receipt["commands"]
    )
    for row in receipt["commands"]:
        expected = outcomes[tuple(row["argv"])]
        assert Path(row["stdout_path"]).read_text(encoding="utf-8") == expected.stdout
        assert Path(row["stderr_path"]).read_text(encoding="utf-8") == expected.stderr
    assert outcomes[("race", "1")].guard_result.cancelled is False
    assert outcomes[("race", "2")].guard_result.cancelled is False


@pytest.mark.parametrize(
    "defect",
    [
        "missing",
        "wrong-type",
        "unclosed",
        "infrastructure",
        "invalid-cancel",
        "cancelled-success",
    ],
)
@pytest.mark.parametrize("concurrent", [True, False], ids=["executor", "serial"])
def test_gate_adapter_refuses_unsafe_or_missing_guard_outcomes(
    tmp_path, defect, concurrent
):
    from dataclasses import replace

    progress = []

    class Progress:
        def emit(self, event, **details):
            progress.append((event, details))

    if defect == "missing":
        outcome = None
    elif defect == "wrong-type":
        outcome = molt_dev_gates.GateOutcome(
            subprocess.CompletedProcess(["synthetic-gate"], 7, "", "")
        )
    elif defect == "unclosed":
        outcome = _gate_outcome(137, "", "", cancelled=True, descendants_closed=False)
    elif defect == "infrastructure":
        outcome = _gate_outcome(
            137,
            "",
            "",
            cancelled=True,
            infrastructure_failure=memory_guard.GuardInfrastructureFailure(
                "temporary_artifact_custody", ("independent custody refusal",)
            ),
        )
    else:
        outcome = _gate_outcome(
            0 if defect == "cancelled-success" else 7, "", "", cancelled=True
        )

    def run(command, repo, environment, cancelled, streams=None):
        assert command == ("gate", "1"), "unsafe outcome scheduled pending gate"
        # Keep the stream writer from dereferencing the deliberately malformed
        # outcome: the production adapter must own its boundary refusal.
        if streams is not None:
            streams[0].write_text("", encoding="utf-8")
            streams[1].write_text("", encoding="utf-8")
        return outcome

    runtime = replace(
        _gate_runtime_fixture(
            molt_dev_gates, tmp_path, {}, output_dir=tmp_path / "gate-output"
        ),
        run_captured_gate=run,
        progress=Progress(),
    )
    gates = [
        SelectedGate(
            command=("gate", "1"), toolchains=("python",), concurrent=concurrent
        )
    ]
    if concurrent:
        # A single concurrent gate takes the serial path. Two selected gates
        # enter the executor; host=1 keeps the second pending after refusal.
        gates.append(
            SelectedGate(command=("gate", "2"), toolchains=("python",), concurrent=True)
        )
    with pytest.raises(molt_dev.DriverError) as caught:
        molt_dev_gates.run_selected_gates(gates, runtime, [], host_limit=lambda: 1)
    assert not isinstance(caught.value, CandidateFailure)
    if concurrent:
        receipt = json.loads((tmp_path / "receipt.json").read_text(encoding="utf-8"))
        record, skipped = receipt["commands"]
        assert skipped["id"] == "local.always.2" and skipped["status"] == "skipped"
        assert record["status"] == "failure"
        assert record["failure_scope"] == "global"
        assert receipt["execution"]["failure_trigger"]["failure_scope"] == "global"
    else:
        finished = [details for event, details in progress if event == "gate_finished"]
        (record,) = finished
    if defect not in {"missing", "wrong-type"}:
        assert record["status"] == "failure"
        assert record["failure_scope"] == "global"
        guard = record["guard_outcome"]
        assert guard["command"] == ["synthetic-gate"]
        assert guard["cancelled"] is True
        assert guard["descendants_closed"] is (defect != "unclosed")
        assert (
            guard["returncode"]
            == {
                "unclosed": 137,
                "infrastructure": 137,
                "invalid-cancel": 7,
                "cancelled-success": 0,
            }[defect]
        )
        assert guard["infrastructure_failure"] == (
            {
                "phase": "temporary_artifact_custody",
                "details": ["independent custody refusal"],
            }
            if defect == "infrastructure"
            else None
        )


@pytest.mark.parametrize("concurrent", [True, False], ids=["executor", "serial"])
@pytest.mark.parametrize(
    ("declared", "sibling_linker"),
    [(True, True), (False, True), (True, False)],
    ids=["declared-beside-linker", "undeclared", "missing-linker"],
)
def test_gate_adapter_admits_windows_helper_only_from_selected_policy(
    tmp_path, concurrent, declared, sibling_linker
):
    from dataclasses import replace
    from tools import proof_plan, win_job

    image = tmp_path / "MSVC" / "bin" / "vctip.exe"
    image.parent.mkdir(parents=True)
    image.write_bytes(b"")
    if sibling_linker:
        image.with_name("link.exe").write_bytes(b"")
    # Real canonical payload types: no platform switch or classifier mock.
    accounting = win_job.WindowsJobAccounting(1, 0, 1, 4096, 0, 0, 0)
    resources = win_job.WindowsSystemResources(1, 1, 1, 1, 1, 2, 1, 2, 1)
    cleanup = win_job.WindowsJobCleanup(
        before=replace(accounting, active_processes=1),
        after=accounting,
        system_before=resources,
        system_after=resources,
        terminated_remaining_processes=True,
        remaining_processes=((720, str(image)),),
        elapsed_s=0.01,
        initial_process_ids=(720,),
        escalation_process_ids=(),
        natural_exit_wait_s=0.0,
    )
    policy = proof_plan.ToolchainPolicy(
        "rustc",
        {"linker_process_helpers": {"link.exe": ["vctip.exe"]}},
        tmp_path,
    )
    outcomes = {
        ("gate", "1"): _gate_outcome(0, "", "", windows_job_cleanup=cleanup),
        ("gate", "2"): _gate_outcome(0, "", ""),
    }
    progress = []

    class Progress:
        def emit(self, event, **details):
            progress.append((event, details))

    runtime = replace(
        _gate_runtime_fixture(
            molt_dev_gates, tmp_path, outcomes, output_dir=tmp_path / "gate-output"
        ),
        progress=Progress(),
        toolchain_policies=(policy,) if declared else (),
    )
    gates = [
        SelectedGate(
            command=("gate", "1"), toolchains=("python",), concurrent=concurrent
        )
    ]
    if concurrent:
        gates.append(
            SelectedGate(command=("gate", "2"), toolchains=("python",), concurrent=True)
        )
    admitted = declared and sibling_linker
    if admitted:
        molt_dev_gates.run_selected_gates(gates, runtime, [], host_limit=lambda: 1)
    else:
        with pytest.raises(molt_dev.DriverError) as caught:
            molt_dev_gates.run_selected_gates(gates, runtime, [], host_limit=lambda: 1)
        assert not isinstance(caught.value, CandidateFailure)
    if concurrent:
        receipt = json.loads((tmp_path / "receipt.json").read_text(encoding="utf-8"))
        record, following = receipt["commands"]
        assert following["status"] == ("success" if admitted else "skipped")
    else:
        (record,) = [details for event, details in progress if event == "gate_finished"]
    assert record["status"] == ("success" if admitted else "failure")
    assert record["failure_scope"] == ("none" if admitted else "global")
    evidence = record["guard_outcome"]["windows_job_cleanup"]
    assert evidence["completed"] is True
    assert evidence["remaining_processes"] == [{"pid": 720, "image": str(image)}]
    assert evidence["initial_process_ids"] == [720]


@pytest.mark.parametrize("defect", ["missing", "wrong-type", "wrong-argv"])
def test_captured_gate_refuses_missing_or_mismatched_guard_result(
    tmp_path, monkeypatch, defect
):
    from tools import molt_dev_common

    if defect == "missing":
        result = None
    elif defect == "wrong-type":
        result = subprocess.CompletedProcess(["synthetic-gate"], 0, "", "")
    else:
        result = _gate_outcome(0, "", "", args=["a-different-launch"]).guard_result
    monkeypatch.setattr(
        harness_memory_guard,
        "guarded_completed_process",
        lambda *args, **kwargs: result,
    )
    with pytest.raises(molt_dev.DriverError) as caught:
        molt_dev_common._run_captured_gate(
            ("synthetic-gate",),
            repo=tmp_path,
            env=ProcessEnvironment(os.environ),
            cancelled=lambda: False,
        )
    assert not isinstance(caught.value, CandidateFailure)


@pytest.mark.parametrize("concurrent", [True, False])
def test_unkept_gate_output_never_changes_the_verdict_or_hides_the_reason(
    drv, tmp_path, capsys, concurrent
):
    # A log that cannot be written must not turn a pass into a runner fault
    # or a failure into silence: the failed gate's streams print whole.

    blocked = tmp_path / "not-a-directory"
    blocked.write_text("", encoding="utf-8")
    outcomes = {
        ("gate", "1"): _gate_outcome(0, "fine\n", "", None),
        ("gate", "2"): _gate_outcome(2, "", "the real reason\n", None),
    }
    runtime = _gate_runtime_fixture(
        molt_dev_gates, tmp_path, outcomes, output_dir=blocked / "run"
    )
    gates = [
        SelectedGate(
            command=("gate", str(index)), toolchains=("python",), concurrent=concurrent
        )
        for index in (1, 2)
    ]
    with pytest.raises(GateFailure, match=r"gate failed \(exit 2\)"):
        molt_dev_gates.run_selected_gates(gates, runtime, [], host_limit=lambda: 2)
    shown = capsys.readouterr().err
    assert "gate output not kept (" in shown and "the real reason" in shown


def test_serial_gate_keeps_its_streams_and_names_them_in_the_failure(
    drv, tmp_path, capsys
):

    outcomes = {
        ("gate", "1"): _gate_outcome(1, "", "x" * 5000 + "\nserial reason\n", None)
    }
    output_dir = tmp_path / "receipt.d" / "run"
    runtime = _gate_runtime_fixture(
        molt_dev_gates, tmp_path, outcomes, output_dir=output_dir
    )
    gate = SelectedGate(command=("gate", "1"), toolchains=("python",), concurrent=False)
    with pytest.raises(GateFailure) as failure:
        molt_dev_gates.run_selected_gates([gate], runtime, [], host_limit=lambda: 1)
    log = output_dir / "gate-001.stderr.log"
    assert "serial reason" in log.read_text(encoding="utf-8")
    assert f"full stderr: {log}" in str(failure.value)
    shown = capsys.readouterr().err
    assert "serial reason" in shown and "... (5000 chars)" in shown


def test_a_gate_writes_its_streams_to_files_while_it_runs(drv, tmp_path):
    # The guarded child writes each stream to its file; the outcome holds
    # only the bounded end, and the files hold everything.
    from tools import molt_dev_common

    streams, stream_error = molt_dev_gates.prepare_gate_streams(tmp_path, 1)
    assert streams is not None and stream_error is None
    lines = 20_000
    script = (
        f"import sys\nfor i in range({lines}): print('line', i)\n"
        "print('the reason', file=sys.stderr)\nraise SystemExit(3)\n"
    )
    outcome = drv.run_captured_gate(
        (sys.executable, "-c", script),
        tmp_path,
        drv.prepare_gate_environment(
            tmp_path,
            ProcessEnvironment(os.environ).updated({"MOLT_VENV": sys.prefix}),
            requires_python=True,
        ),
        lambda: False,
        streams,
    )
    assert outcome.returncode == 3
    full = streams[0].read_text(encoding="utf-8")
    assert full.count("\n") == lines and full.endswith(f"line {lines - 1}\n")
    assert len(outcome.stdout) <= molt_dev_common.GATE_STREAM_TAIL_BYTES
    assert outcome.stdout.endswith(f"line {lines - 1}\n")
    assert streams[1].read_text(encoding="utf-8") == "the reason\n"


def test_idle_gate_runs_retire_and_recent_ones_stay(tmp_path):

    now = 10_000_000.0
    window = molt_dev_gates.NAMED_RECORD_MIN_AGE_S
    runs = {"old": window + 60, "recent": window - 60}
    for name, age in runs.items():
        run = tmp_path / "abc.gates.d" / name
        run.mkdir(parents=True)
        (run / "gate-001.stdout.log").write_text("x", encoding="utf-8")
        os.utime(run, (now - age, now - age))
    gone = tmp_path / "def.precheck.d" / "old"
    gone.mkdir(parents=True)
    os.utime(gone, (now - window - 60, now - window - 60))
    molt_dev_gates.retire_gate_logs(tmp_path, now=now)
    assert [p.name for p in (tmp_path / "abc.gates.d").iterdir()] == ["recent"]
    # A receipt directory its last run leaves empty goes with it.
    assert not (tmp_path / "def.precheck.d").exists()


def test_a_gate_log_that_cannot_retire_says_so(tmp_path, monkeypatch, capsys):

    now = 10_000_000.0
    run = tmp_path / "abc.gates.d" / "old"
    run.mkdir(parents=True)
    age = molt_dev_gates.NAMED_RECORD_MIN_AGE_S + 60
    os.utime(run, (now - age, now - age))

    def refuse(path):
        raise PermissionError(13, "Permission denied", str(path))

    from tests.process_guard_common import install_module_view

    install_module_view(monkeypatch, "shutil", shutil, molt_dev_gates, rmtree=refuse)
    molt_dev_gates.retire_gate_logs(tmp_path, now=now)
    err = capsys.readouterr().err
    assert "1 idle gate log run(s) could not be retired" in err
    assert f"{run}: PermissionError" in err


def test_the_keepalive_label_names_only_the_gate_commands_first_words(drv, monkeypatch):
    seen = []

    def fake(cmd, **kwargs):
        seen.append(kwargs["label"])
        return _gate_outcome(0, "", "").guard_result

    monkeypatch.setattr(drv, "_run_captured_gate", fake)
    monkeypatch.setattr(
        drv, "_gate_environment", lambda cmd, repo, context: (cmd, context.environment)
    )
    drv.run_captured_gate(
        "python tools/a.py --one --two --three",
        Path("."),
        GateEnvironment(ProcessEnvironment(os.environ), None, False),
        lambda: False,
    )
    drv.run_captured_gate(
        ("cargo", "test"),
        Path("."),
        GateEnvironment(ProcessEnvironment(os.environ), None, False),
        lambda: False,
    )
    assert seen == ["gate python tools/a.py --one --two ...", "gate cargo test"]


def test_gates_overlap_across_classes_and_cargo_gates_stay_serial(drv, tmp_path):
    # Read-only checks run first; then the heavy gates share one executor: a
    # pytest session and a Cargo gate overlap, Cargo gates (one shared target)
    # never overlap each other, and each gate gets the memory slice its pytest
    # -n auto sizes within.
    import threading

    from molt import host_capacity

    lock = threading.Lock()
    active: dict[str, int] = {}
    peak: dict[str, int] = {}
    overlap = {"cargo_and_pytest": False}
    slices: list[str | None] = []

    def run_captured_gate(command, repo, environment, cancelled, streams=None):
        assert environment.python_root == tmp_path / "planned-image"
        assert environment.managed is True
        name = command[0]
        with lock:
            active[name] = active.get(name, 0) + 1
            peak[name] = max(peak.get(name, 0), active[name])
            if active.get("cargo", 0) and active.get("pytest", 0):
                overlap["cargo_and_pytest"] = True
            slices.append(environment.environment.get(host_capacity.MEMORY_SLICE_ENV))
        time.sleep(0.3)
        with lock:
            active[name] -= 1
        return _gate_outcome(0, "", "", None)

    class Progress:
        def emit(self, event, **details):
            pass

    # The base environment carries no slice (a guard running this test may
    # export one); the executor must add its own.
    environment = ProcessEnvironment(os.environ).without(
        (host_capacity.MEMORY_SLICE_ENV,)
    )
    gates = [
        SelectedGate(command=("static", "1"), toolchains=("python",), concurrent=True),
        SelectedGate(command=("static", "2"), toolchains=("python",), concurrent=True),
        SelectedGate(command=("cargo", "a"), toolchains=("python", "cargo")),
        SelectedGate(command=("pytest", "a"), toolchains=("python",), pytest=True),
        SelectedGate(command=("cargo", "b"), toolchains=("python", "cargo")),
        SelectedGate(command=("pytest", "b"), toolchains=("python",), pytest=True),
    ]
    runtime = molt_dev_gates.GateRuntime(
        repo=tmp_path,
        environments={
            gate.toolchains: GateEnvironment(
                environment, tmp_path / "planned-image", True
            )
            for gate in gates
        },
        cargo_outputs={},
        capacity_requirements={},
        progress=Progress(),
        check_source=lambda: None,
        source_changes=lambda: (),
        source_identity=lambda: {"commit": "0" * 40, "tree": "1" * 40},
        policy_text="",
        executor_max_workers=4,
        resource_limits={
            "repository-policy": 4,
            "python-tests": 2,
            "compiler-build-resource": 1,
        },
        toolchains={},
        receipt_path=tmp_path / "receipt.json",
        output_dir=tmp_path / "receipt.d" / "run",
        run_captured_gate=run_captured_gate,
        observe_capacity=lambda *args, **kwargs: (),
        say=lambda message: None,
        ok=lambda message: None,
    )
    seconds: list[tuple[float, str]] = []
    molt_dev_gates.run_selected_gates(gates, runtime, seconds, host_limit=lambda: 4)

    assert overlap["cargo_and_pytest"]
    assert peak["cargo"] == 1
    assert peak["pytest"] == 2
    assert [shown for _elapsed, shown in seconds] == [
        molt_dev_gates.display(gate.command) for gate in gates
    ]
    # The read-only checks ran (and passed) before any heavy gate started.
    light = json.loads((tmp_path / "receipt.json").read_text(encoding="utf-8"))
    heavy = json.loads((tmp_path / "receipt.heavy.json").read_text(encoding="utf-8"))
    assert [row["id"] for row in light["commands"]] == [
        "local.always.1",
        "local.always.2",
    ]
    assert [row["id"] for row in heavy["commands"]] == [
        "local.gate.3",
        "local.gate.4",
        "local.gate.5",
        "local.gate.6",
    ]
    # Each block exports one slice to its gates: the read-only block's two
    # gates first, then the heavy block's four.
    assert len(slices) == len(gates) and all(int(value) > 0 for value in slices)
    assert len(set(slices[:2])) == 1 and len(set(slices[2:])) == 1


def _heavy_runtime(molt_dev_gates, tmp_path, gates, run_captured_gate):
    class Progress:
        def emit(self, event, **details):
            pass

    return molt_dev_gates.GateRuntime(
        repo=tmp_path,
        environments={
            gate.toolchains: GateEnvironment(
                ProcessEnvironment(os.environ), None, False
            )
            for gate in gates
        },
        cargo_outputs={},
        capacity_requirements={},
        progress=Progress(),
        check_source=lambda: None,
        source_changes=lambda: (),
        source_identity=lambda: {"commit": "0" * 40, "tree": "1" * 40},
        policy_text="",
        executor_max_workers=8,
        resource_limits={
            "repository-policy": 4,
            "python-tests": 2,
            "compiler-build-resource": 1,
        },
        toolchains={},
        receipt_path=tmp_path / "receipt.json",
        output_dir=tmp_path / "receipt.d" / "run",
        run_captured_gate=run_captured_gate,
        observe_capacity=lambda *args, **kwargs: (),
        say=lambda message: None,
        ok=lambda message: None,
    )


def test_a_building_pytest_session_counts_as_a_session_and_waits_its_cargo_turn(
    tmp_path,
):
    # A pytest session whose rules declare Cargo is still a pytest session
    # (at most two at once), and it never overlaps another gate that builds.
    import threading

    lock = threading.Lock()
    active: set[tuple[str, ...]] = set()
    peak = {"pytest": 0, "cargo": 0}

    def run_captured_gate(command, repo, environment, cancelled, streams=None):
        with lock:
            active.add(command)
            peak["pytest"] = max(peak["pytest"], sum(c[0] == "pytest" for c in active))
            peak["cargo"] = max(peak["cargo"], sum("build" in c for c in active))
        time.sleep(0.2)
        with lock:
            active.discard(command)
        return _gate_outcome(0, "", "", None)

    # The building session comes first, so it starts at once: admitted outside
    # the session class it would run beside both other sessions.
    gates = [
        SelectedGate(
            command=("pytest", "build"), toolchains=("python", "cargo"), pytest=True
        ),
        SelectedGate(command=("pytest", "a"), toolchains=("python",), pytest=True),
        SelectedGate(command=("pytest", "b"), toolchains=("python",), pytest=True),
        SelectedGate(command=("cargo", "build"), toolchains=("python", "cargo")),
    ]
    runtime = _heavy_runtime(molt_dev_gates, tmp_path, gates, run_captured_gate)
    molt_dev_gates.run_selected_gates(gates, runtime, [], host_limit=lambda: 8)
    assert peak == {"pytest": 2, "cargo": 1}


def test_a_gate_runner_error_keeps_its_streams_in_the_receipt(drv, tmp_path):

    def run_captured_gate(command, repo, environment, cancelled, streams=None):
        if command[1] == "raises":
            streams[0].write_text("partial\n", encoding="utf-8")
            raise drv.DriverError("the runner lost its child")
        return _gate_outcome(0, "", "", None)

    gates = [
        SelectedGate(command=("pytest", "raises"), toolchains=("python",), pytest=True),
        SelectedGate(command=("pytest", "ok"), toolchains=("python",), pytest=True),
    ]
    runtime = _heavy_runtime(molt_dev_gates, tmp_path, gates, run_captured_gate)
    with pytest.raises(drv.DriverError, match="the runner lost its child"):
        molt_dev_gates.run_selected_gates(gates, runtime, [], host_limit=lambda: 8)
    heavy = json.loads((tmp_path / "receipt.heavy.json").read_text(encoding="utf-8"))
    record = next(r for r in heavy["commands"] if r["id"] == "local.gate.1")
    assert Path(record["stdout_path"]).read_text(encoding="utf-8") == "partial\n"
    assert record["stderr_path"] and record["duration_seconds"] is not None
    assert "the runner lost its child" in record["executor_error"]


def test_concurrent_gate_limit_comes_from_host_capacity(drv, monkeypatch):
    from molt import host_capacity

    calls = []

    def worker_count(*, bytes_per_worker, headroom_bytes, cpus=None):
        calls.append(bytes_per_worker)
        return 3

    monkeypatch.setattr(host_capacity, "worker_count", worker_count)
    assert drv.concurrent_gate_limit() == 3
    assert calls == [molt_dev_gates.CONCURRENT_GATE_BYTES]


def test_integrate_gates_never_write_bytecode_into_the_checkout(
    drv, origin_and_clone, monkeypatch
):
    # Bytecode a gate writes under src/ changes the checkout's import topology
    # while later gates read it: a fresh worktree's first landing failed with
    # "Python import topology changed during operation: <worktree>/src/molt".
    # The caller's shell may already set the policy; integrate must not rely on it.
    monkeypatch.delenv("PYTHONDONTWRITEBYTECODE", raising=False)
    origin, work = origin_and_clone
    observed = work.parent / "bytecode.json"
    _commit_file(work, "src/probe_import.py", "VALUE = 1\n", "feat: probe")
    # The gate runs the host `python3`; Python 3.15 removed module `__cached__`,
    # so read the bytecode path from the module spec.
    probe = (
        "import json, sys; sys.path.insert(0, 'src'); import probe_import; "
        f"open({str(observed)!r}, 'w').write(json.dumps([sys.pycache_prefix, "
        "sys.dont_write_bytecode, probe_import.__spec__.cached]))"
    )
    _commit_file(
        work,
        "tools/proof_plan.toml",
        f"""
always = []

[[rule]]
name = "imports-source"
globs = ["src/*.py"]
gates = [{json.dumps(shlex.join(["env", "python3", "-c", probe]))}]
""",
        "a gate that imports checkout source",
    )
    ns = _integrate_ns(drv, repo=str(work), no_push=True)
    drv.cmd_integrate(ns)
    assert not list((work / "src").rglob("__pycache__"))
    # Gates share one bytecode prefix outside the checkout instead.
    prefix, no_write, cached = json.loads(observed.read_text(encoding="utf-8"))
    assert prefix and not Path(prefix).resolve().is_relative_to(work.resolve())
    assert no_write is False
    assert Path(cached).is_file() and Path(cached).is_relative_to(prefix)


@pytest.mark.parametrize("pin_upstream", [False, True])
def test_integrate_full_push_and_confirm(
    drv, origin_and_clone, monkeypatch, pin_upstream
):
    # The happy path end-to-end: a passing gate, real push, fetch+ancestor
    # confirm. Proves the push-confirm gate accepts a genuinely-landed tip.
    origin, work = origin_and_clone
    feature = _commit_file(work, "src/molt/ok.py", "ok = True\n", "feat: ok")
    _git(work, "commit", "--allow-empty", "-qm", "intentional empty source commit")
    git = drv.Git(work)
    tip = git.head_sha()
    upstream = git.rev_parse("origin/main")
    commands = _record_driver_git_commands(drv, monkeypatch)
    ns = _integrate_ns(
        drv,
        repo=str(work),
        no_push=False,
        expected_upstream=upstream if pin_upstream else None,
    )
    assert drv.cmd_integrate(ns) == drv.EXIT_OK
    _assert_no_patch_diff_work(commands)
    assert not any(args[0] == "rebase" for args in commands)
    assert git.head_sha() == tip
    assert _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0] == tip
    assert _git(work, "show", f"{tip}:src/molt/ok.py").stdout == "ok = True\n"
    assert git.is_ancestor(feature, tip)
    # origin/main now contains the tip (confirmed by the same ancestor check).
    git.fetch("origin", "main")
    assert git.is_ancestor(tip, "origin/main")


@pytest.mark.parametrize("needs_rebase", [False, True])
def test_integrate_dry_run_mutates_nothing(
    drv, origin_and_clone, monkeypatch, needs_rebase
):
    origin, work = origin_and_clone
    _commit_file(work, "src/molt/d.py", "d = 1\n", "feat: d")
    if needs_rebase:
        other = work.parent / "dry-upstream"
        _git(work.parent, "clone", "-q", str(origin), str(other))
        _git(other, "config", "user.email", "other@example.test")
        _git(other, "config", "user.name", "Other")
        _commit_file(other, "upstream.txt", "advance\n", "upstream advance")
        _git(other, "push", "-q", "origin", "main")
        _git(work, "fetch", "-q", "origin", "main")
    commands = _record_driver_git_commands(drv, monkeypatch)
    git = drv.Git(work)
    head_before = git.head_sha()
    origin_before = _git(work, "rev-parse", "origin/main").stdout.strip()
    ns = _integrate_ns(
        drv,
        repo=str(work),
        dry_run=True,
        no_push=False,
    )
    assert drv.cmd_integrate(ns) == drv.EXIT_OK
    assert git.head_sha() == head_before
    assert _git(work, "rev-parse", "origin/main").stdout.strip() == origin_before
    _assert_no_patch_diff_work(commands)
    assert not any(args[0] == "rebase" for args in commands)


@pytest.mark.parametrize("has_source", [False, True])
def test_integrate_no_rebase_rejects_head_drift_before_selection(
    drv, origin_and_clone, monkeypatch, has_source
):
    _, work = origin_and_clone
    upstream = _git(work, "rev-parse", "origin/main").stdout.strip()
    if has_source:
        _commit_file(work, "feature.py", "value = 1\n", "feature")
    captured = _git(work, "rev-parse", "HEAD").stdout.strip()
    trigger = f"{captured}..{upstream}" if has_source else f"{upstream}..{captured}"
    original = drv.Git.commits_in_range
    changed = False

    def move_head(self, range_expr):
        nonlocal changed
        result = original(self, range_expr)
        if range_expr == trigger and not changed:
            changed = True
            _git(work, "commit", "--allow-empty", "-qm", "concurrent source drift")
        return result

    monkeypatch.setattr(drv.Git, "commits_in_range", move_head)
    commands = _record_driver_git_commands(drv, monkeypatch)
    with pytest.raises(drv.DriverError, match="source HEAD changed"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work)))
    assert changed
    assert _git(work, "rev-parse", "HEAD").stdout.strip() != captured
    assert (
        _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
        == upstream
    )
    _assert_no_patch_diff_work(commands)


def test_integrate_nothing_to_do_is_green(drv, origin_and_clone):
    # HEAD already on origin/main -> integrate is a green no-op.
    origin, work = origin_and_clone
    ns = _integrate_ns(drv, repo=str(work), no_push=True)
    assert drv.cmd_integrate(ns) == drv.EXIT_OK


@pytest.mark.parametrize(
    "primary_name",
    [
        "primary with spaces",
        pytest.param(
            "primary\rwith control character",
            marks=pytest.mark.skipif(os.name == "nt", reason="invalid Windows path"),
        ),
    ],
)
def test_integration_lock_path_is_shared_by_real_sibling_worktrees(
    drv, origin_and_clone, tmp_path, primary_name
):
    _, work = origin_and_clone
    moved = tmp_path / primary_name
    work.rename(moved)
    work = moved
    sibling = tmp_path / "sibling worktree"
    _git(work, "worktree", "add", "-q", "--detach", str(sibling))
    assert (work / ".git").is_dir()
    assert (sibling / ".git").is_file()
    # The primary checkout's actual Git directory is an independent oracle;
    # deriving this expected path through the helper would hide per-worktree locks.
    expected = work / ".git" / "molt-integrate.lock"
    assert drv._integration_lock_path(work) == expected.resolve()
    assert drv._integration_lock_path(sibling) == expected.resolve()
    assert not expected.exists(), "resolving a lock path must not create its file"


def test_integrate_cross_process_sibling_contention_and_release(
    drv, origin_and_clone, tmp_path, monkeypatch
):
    _, work = origin_and_clone
    sibling = tmp_path / "competing worktree"
    _git(work, "worktree", "add", "-q", "--detach", str(sibling))
    entered = tmp_path / "contender-entered.txt"
    child_source = """
import argparse
import json
import sys
from pathlib import Path
from tools import molt_dev

assert Path(molt_dev.__file__).resolve() == Path(sys.argv[3]).resolve()
def exclusive(args, *, progress):
    Path(sys.argv[2]).write_text("entered", encoding="utf-8")
    return 73
molt_dev._integrate_exclusive = exclusive
try:
    code = molt_dev.cmd_integrate(argparse.Namespace(
        repo=sys.argv[1], dry_run=False, pinned_controller=None, wait_for_lock=False
    ))
except molt_dev.DriverError as error:
    result = {"busy": True, "message": str(error)}
else:
    result = {"busy": False, "code": code}
print(json.dumps(result))
"""
    env = dict(os.environ)
    env["PYTHONPATH"] = os.pathsep.join(
        (str(REPO_ROOT / "src"), str(REPO_ROOT), env.get("PYTHONPATH", ""))
    )

    def contend():
        result = run_guarded_test_process(
            [
                sys.executable,
                "-c",
                child_source,
                str(sibling),
                str(entered),
                str(SCRIPT_PATH),
            ],
            cwd=REPO_ROOT,
            env=ProcessEnvironment(env),
            timeout=30,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        return json.loads(result.stdout.splitlines()[-1])

    def exclusive(args, *, progress):
        assert Path(args.repo) == work
        observed = contend()
        assert observed["busy"] is True
        assert observed["message"] == (
            "another integration owns this Git repository; no fetch or gates ran. "
            "Retry after it finishes; do not remove the integration lock file."
        )
        # Any admitted exclusive body would be able to fetch and run gates.
        assert not entered.exists()
        return drv.EXIT_OK

    monkeypatch.setattr(drv, "_integrate_exclusive", exclusive)
    assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work))) == drv.EXIT_OK
    lock_path = work / ".git" / "molt-integrate.lock"
    identity = lock_path.stat()
    assert contend() == {"busy": False, "code": 73}
    assert entered.read_text(encoding="utf-8") == "entered"
    after = lock_path.stat()
    assert (after.st_dev, after.st_ino) == (identity.st_dev, identity.st_ino)


# A stand-in for main's driver: it reports how it was launched, then exits 41.
_CONTROLLER_STUB = """\
import json, os, sys
from pathlib import Path
Path(os.environ["MOLT_TEST_CONTROLLER_REPORT"]).write_text(json.dumps({{
    "role": {role!r}, "file": __file__, "argv": sys.argv[1:],
    "isolated": sys.flags.isolated, "prefix": sys.prefix,
}}), encoding="utf-8")
raise SystemExit(41)
"""


def _publish_controller_stub(work: Path, role: str) -> str:
    (work / "pyproject.toml").write_text(
        '[project]\nname = "fixture"\nversion = "0"\nrequires-python = ">=3.12"\n'
        "dependencies = []\n",
        encoding="utf-8",
    )
    (work / ".python-version").write_text("3.12\n", encoding="utf-8")
    uv = os.environ.get("UV") or shutil.which("uv")
    assert uv, "the pinned controller requires uv"
    locked = _run_fast_captured_command([uv, "lock", "--offline", "--quiet"], cwd=work)
    assert locked.returncode == 0, locked.stderr
    _git(work, "add", "--", "pyproject.toml", ".python-version", "uv.lock")
    # Every controller path exists on main (molt_dev_controller.CONTROLLER_PATHS).
    _commit_file(work, "config/tool_releases.toml", "", "config")
    _commit_file(work, "src/molt/__init__.py", "", "molt package")
    # This stand-in has an empty package and no startup behavior; real driver
    # origins copy the genuine startup source through the shared file owner.
    _commit_file(work, "src/sitecustomize.py", "", "fixture Python startup")
    tip = _commit_file(
        work, "tools/molt_dev.py", _CONTROLLER_STUB.format(role=role), "main driver"
    )
    _git(work, "push", "-q", "origin", "HEAD:main")
    return tip


def _land_through_real_driver(
    work: Path, report: Path, command: tuple[str, ...] = ("integrate", "--no-push")
) -> tuple[int, dict]:
    env = dict(os.environ)
    env["MOLT_TEST_CONTROLLER_REPORT"] = str(report)
    name, *options = command
    result = run_guarded_test_process(
        [sys.executable, str(SCRIPT_PATH), name, "--repo", str(work), *options],
        cwd=REPO_ROOT,
        env=ProcessEnvironment(env),
        timeout=60,
    )
    assert report.is_file(), result.stdout + result.stderr
    return result.returncode, json.loads(report.read_text(encoding="utf-8"))


@pytest.mark.parametrize(
    "command",
    [("integrate", "--no-push"), ("land", "--wait")],
    ids=["integrate", "land"],
)
def test_landing_commands_run_main_pinned_controller_not_the_candidate(
    origin_and_clone, tmp_path, command
):
    _, work = origin_and_clone
    main_tip = _publish_controller_stub(work, "main")
    # The candidate changes the driver; its version must not run its own
    # landing, nor write its queue request and start its runner.
    _commit_file(
        work, "tools/molt_dev.py", _CONTROLLER_STUB.format(role="candidate"), "edit"
    )
    returncode, report = _land_through_real_driver(
        work, tmp_path / "report.json", command
    )

    assert report["role"] == "main"
    assert returncode == 41, "exec hands the landing's exit status through"
    assert report["isolated"] == 1
    controllers = work / ".git" / "molt" / "controllers"
    (root,) = controllers.iterdir()
    assert Path(report["file"]) == (root / "tools" / "molt_dev.py").resolve()
    # It runs in the environment built from main's lock, not the caller's.
    assert Path(report["prefix"]).resolve() == (root / ".venv").resolve()
    expected_argv = [
        command[0],
        "--repo",
        str(work),
        *command[1:],
        "--repo",
        str(work.resolve()),
        "--pinned-controller",
        root.name,
    ]
    if os.name == "nt":
        expected_argv.extend(["--controller-cwd", str(REPO_ROOT.resolve())])
    assert report["argv"] == expected_argv
    record = json.loads((root / ".molt-controller.json").read_text(encoding="utf-8"))
    assert record == {
        "schema": "molt.integration-controller.v1",
        "key": root.name,
        "commit": main_tip,
    }


def test_pinned_controller_is_reused_per_tree_and_replaced_when_main_changes(
    origin_and_clone, tmp_path
):
    _, work = origin_and_clone
    _publish_controller_stub(work, "main")
    _land_through_real_driver(work, tmp_path / "first.json")
    controllers = work / ".git" / "molt" / "controllers"
    (first,) = controllers.iterdir()
    planted = first / "tools" / "planted.txt"
    planted.write_text("reuse proof\n", encoding="utf-8")

    # A landing that changes nothing the controller runs reuses its extraction.
    _commit_file(work, "README.md", "unrelated\n", "docs")
    _git(work, "push", "-q", "origin", "HEAD:main")
    _land_through_real_driver(work, tmp_path / "second.json")
    assert planted.read_text(encoding="utf-8") == "reuse proof\n"
    assert list(controllers.iterdir()) == [first]

    # A change to main's tools tree selects a new controller.
    _commit_file(work, "tools/helper.py", "VALUE = 1\n", "tools change")
    _git(work, "push", "-q", "origin", "HEAD:main")
    _, report = _land_through_real_driver(work, tmp_path / "third.json")
    second = Path(report["file"]).parents[1]
    assert second != first.resolve() and second.parent == controllers.resolve()
    assert (second / "tools" / "helper.py").read_text(encoding="utf-8") == "VALUE = 1\n"
    assert not (second / "tools" / "planted.txt").exists()

    # Startup code is executable controller input even outside the package tree.
    startup = "# changed startup authority\n"
    _commit_file(work, "src/sitecustomize.py", startup, "Python startup change")
    _git(work, "push", "-q", "origin", "HEAD:main")
    _, report = _land_through_real_driver(work, tmp_path / "fourth.json")
    third = Path(report["file"]).parents[1]
    assert third != second and third.parent == controllers.resolve()
    assert (third / "src" / "sitecustomize.py").read_text(encoding="utf-8") == startup


def test_real_driver_lands_and_retires_a_worktree_as_pinned_controller(
    driver_origin_and_clone, tmp_path
):
    _, work = driver_origin_and_clone
    (work / "tools" / "proof_plan.toml").write_text(
        'always = ["echo controller-gate"]\n', encoding="utf-8"
    )
    _git(work, "add", "--", "tools/proof_plan.toml")
    _git(work, "commit", "-q", "--no-verify", "-m", "main driver")
    _git(work, "push", "-q", "origin", "HEAD:main")
    lane = tmp_path / "lane"
    _git(work, "worktree", "add", "-q", "-b", "lane/controller", str(lane))
    lane_tip = _commit_file(lane, "feature.txt", "landed\n", "feature")

    result = run_guarded_test_process(
        [
            sys.executable,
            str(SCRIPT_PATH),
            "integrate",
            "--repo",
            str(lane),
            "--cleanup-worktree",
        ],
        cwd=REPO_ROOT,
        env=ProcessEnvironment(os.environ),
        timeout=300,
    )
    output = result.stdout + result.stderr
    assert result.returncode == molt_dev.EXIT_OK, output
    events = [
        json.loads(line)
        for line in result.stdout.splitlines()
        if line.startswith('{"schema": "molt.integration-progress.v1"')
    ]
    (root,) = (work / ".git" / "molt" / "controllers").iterdir()
    assert {event["controller"] for event in events} == {f"pinned:{root.name}"}
    assert events[-1]["status"] == "completed"
    assert "controller-gate" in output
    origin_main = _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
    assert origin_main == lane_tip
    assert not lane.exists(), "the controller survives the worktree it retires"


def test_lane_start_and_retire_run_through_mains_controller_by_default(
    driver_origin_and_clone,
):
    """Main's pinned controller starts and retires a lane in a default shell.

    A POSIX shell wires sccache by default, which reads the controller's
    config/tool_releases.toml; the controller once lacked config/ and every
    lane start crashed after creating its worktree (review v2, B-1, B-2).
    The fixture cannot provision the Acorn tree or the Git hooks (it carries
    neither), so the start may report those inputs; it must record the lane.
    """
    _, work = driver_origin_and_clone
    env = dict(os.environ)
    env.pop("MOLT_USE_SCCACHE", None)

    def lane(*argv: str):
        result = run_guarded_test_process(
            [sys.executable, str(SCRIPT_PATH), "lane", *argv, "--repo", str(work)],
            cwd=REPO_ROOT,
            env=ProcessEnvironment(env),
            timeout=300,
        )
        output = result.stdout + result.stderr
        assert "Traceback" not in output, output
        return result.returncode, output

    returncode, output = lane("start", "handoff", "--root", str(work.parent / "lanes"))
    (controller,) = (work / ".git" / "molt" / "controllers").iterdir()
    assert f"pinned controller {controller.name}" in output, output
    assert returncode in {molt_dev.EXIT_OK, molt_dev.EXIT_FAIL}, output
    if returncode == molt_dev.EXIT_FAIL:
        # Only the inputs the fixture lacks may fail; a failed start prints
        # failures alone, so any other input's name means it failed.
        assert "the lane exists; finish provisioning" in output, output
        assert "uv-environment:" not in output, output
    record_path = work / "logs" / "agents" / "lane-handoff" / "coordination.json"
    assert record_path.is_file(), output
    record = json.loads(record_path.read_text(encoding="utf-8"))
    assert record["status"] == "running", output
    worktree = Path(record["lane"]["worktree"])
    assert worktree.is_dir()

    returncode, output = lane("retire", "handoff")
    assert returncode == molt_dev.EXIT_OK, output
    assert f"pinned controller {controller.name}" in output, output
    # A slot lane's retirement keeps the slot, detached, for the next lane
    # (HF-491).
    assert worktree.name == "slot-01"
    assert _git(worktree, "status", "--porcelain", "--branch").stdout.startswith(
        "## HEAD (no branch)"
    )
    assert _git(work, "branch", "--list", "lane/handoff").stdout.strip() == ""
    record = json.loads(record_path.read_text(encoding="utf-8"))
    assert record["status"] == "landed"


def test_main_without_a_controller_refuses_before_any_gate(origin_and_clone, tmp_path):
    _, work = origin_and_clone
    env = dict(os.environ)
    result = run_guarded_test_process(
        [
            sys.executable,
            str(SCRIPT_PATH),
            "integrate",
            "--repo",
            str(work),
            "--no-push",
        ],
        cwd=REPO_ROOT,
        env=ProcessEnvironment(env),
        timeout=60,
    )
    assert result.returncode == molt_dev.EXIT_FAIL
    assert "git rev-parse" in result.stdout + result.stderr
    assert "integration-policy" not in result.stdout, "no gate may run"
    assert not (work / ".git" / "molt" / "controllers").exists()


def test_pinned_controller_claim_must_match_this_process(drv, origin_and_clone):
    _, work = origin_and_clone
    key = "0" * 32
    with pytest.raises(drv.DriverError, match="has no record"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), pinned_controller=key))
    record = work / ".git" / "molt" / "controllers" / key / ".molt-controller.json"
    record.parent.mkdir(parents=True)
    record.write_text("{}\n", encoding="utf-8")
    with pytest.raises(drv.DriverError, match="but this driver runs from"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), pinned_controller=key))


def test_controller_retention_keeps_recent_and_retires_idle(tmp_path):
    from tools import molt_dev_controller as controller

    now = 10_000_000.0
    day = 24 * 3600.0

    def entry(name: str, age: float) -> Path:
        root = tmp_path / name
        root.mkdir()
        record = root / controller.RECORD_NAME
        record.write_text("{}\n", encoding="utf-8")
        os.utime(record, (now - age, now - age))
        return root

    current = entry("current", 5 * day)
    entry("recent-a", 10)
    entry("recent-b", 20)
    entry("old-a", 2 * day)
    entry("old-b", 3 * day)
    entry("old-c", 4 * day)
    controller._retire_idle(tmp_path, current=current, now=now)
    # KEEP=4 counts the current controller; older entries go once idle a day.
    assert sorted(path.name for path in tmp_path.iterdir()) == [
        "current",
        "old-a",
        "recent-a",
        "recent-b",
    ]

    for index in range(4):
        entry(f"busy-{index}", 60.0 * (index + 1))
    controller._retire_idle(tmp_path, current=current, now=now)
    # Entries beyond KEEP stay while used within the idle window.
    assert {path.name for path in tmp_path.iterdir()} >= {
        "busy-0",
        "busy-1",
        "busy-2",
        "busy-3",
        "recent-a",
        "recent-b",
    }
    assert not (tmp_path / "old-a").exists()


# --------------------------------------------------------------------------
# Landing queue: batches, bisection, conflicts and races
# --------------------------------------------------------------------------


def _counting_policy(work: Path, counter: Path, check: str = "true") -> None:
    """main's gate policy: each run appends one line, then runs `check`."""
    (work / "tools" / "proof_plan.toml").write_text(
        f"always = [{json.dumps(f'echo run >> {shlex.quote(str(counter))} && {check}')}]\n",
        encoding="utf-8",
    )
    _git(work, "add", "--", "tools/proof_plan.toml")
    _git(work, "commit", "-q", "--no-verify", "-m", "counting gate")
    _git(work, "push", "-q", "origin", "HEAD:main")


def _lane(work: Path, name: str, files: dict[str, str]) -> str:
    # An ordinary source checkout; managed lane custody is exercised by
    # test_molt_dev_lanes using lane start and its real coordination owner.
    lane = work.parent / f"lane-{name}"
    _git(work, "fetch", "-q", "origin")
    _git(work, "worktree", "add", "-q", "-b", f"codex/{name}", str(lane), "origin/main")
    for rel, content in files.items():
        tip = _commit_file(lane, rel, content, f"lane {name}: {rel}")
    return tip


def _enqueue(drv, work: Path, *heads: str):
    from tools import molt_dev_queue as queue

    paths = queue.queue_paths(work)
    return paths, [
        queue.enqueue(paths, head=head, worktree=work, branch=None) for head in heads
    ]


def _run_queue(drv, work: Path) -> int:
    args = drv.build_parser().parse_args(
        ["queue", "run", "--repo", str(work), "--candidate-controller"]
    )
    return args.func(args)


def _statuses(paths, requests) -> list[str]:
    from tools import molt_dev_queue as queue

    return [queue.read_result(paths, request.id)["status"] for request in requests]


def _origin_tree(work: Path) -> set[str]:
    _git(work, "fetch", "-q", "origin")
    return set(_git(work, "ls-tree", "-r", "--name-only", "origin/main").stdout.split())


def test_queue_lands_ready_lanes_in_one_batch_with_one_gate_run(
    drv, origin_and_clone, tmp_path
):
    _, work = origin_and_clone
    counter = tmp_path / "gate-runs.txt"
    _counting_policy(work, counter)
    heads = [_lane(work, name, {f"{name}.py": f"{name} = 1\n"}) for name in "abc"]
    paths, requests = _enqueue(drv, work, *heads)

    assert _run_queue(drv, work) == drv.EXIT_OK

    assert _statuses(paths, requests) == ["landed"] * 3
    assert counter.read_text(encoding="utf-8").splitlines() == ["run"]
    assert {"a.py", "b.py", "c.py"} <= _origin_tree(work)
    from tools import molt_dev_queue as queue

    landed = {queue.read_result(paths, r.id)["landed"] for r in requests}
    assert landed == {_git(work, "rev-parse", "origin/main").stdout.strip()}
    assert queue.pending(paths) == [] and not paths.plan.exists()


def test_queue_retry_preserves_unchanged_inputs_and_ignored_products(
    drv, origin_and_clone
):
    from tools import molt_dev_queue as queue

    _, work = origin_and_clone
    upstream = _git(work, "rev-parse", "origin/main").stdout.strip()
    first = _lane(work, "incremental", {"runtime.rs": "pub fn runtime() {}\n"})
    paths, requests = _enqueue(drv, work, first)
    git = drv.Git(work)

    def run(arguments):
        result = git._run(list(arguments), check=False)
        return queue.GitResult(result.returncode, result.stdout, result.stderr)

    assert queue.build_candidate(run, upstream, requests).included == tuple(requests)
    prior_candidate = git.head_sha()
    source = work / "runtime.rs"
    before = source.stat()
    (work / ".git/info/exclude").write_text("target/\n", encoding="utf-8")
    product = work / "target/retained"
    product.parent.mkdir()
    product.write_bytes(b"unchanged build output")
    second = _commit_file(
        work.parent / "lane-incremental",
        "fixture.py",
        "assert True\n",
        "repair only a test fixture",
    )
    _, retry = _enqueue(drv, work, second)

    assert queue.build_candidate(run, upstream, retry).included == tuple(retry)

    after = source.stat()
    assert (after.st_dev, after.st_ino, after.st_mtime_ns, after.st_size) == (
        before.st_dev,
        before.st_ino,
        before.st_mtime_ns,
        before.st_size,
    )
    assert source.read_text(encoding="utf-8") == "pub fn runtime() {}\n"
    assert product.read_bytes() == b"unchanged build output"
    assert git.head_sha() != second
    assert run(["merge-base", "--is-ancestor", second, "HEAD"]).returncode == 0
    assert run(["merge-base", "--is-ancestor", upstream, "HEAD"]).returncode == 0
    assert run(["merge-base", "--is-ancestor", prior_candidate, "HEAD"]).returncode == 1
    assert not run(["status", "--porcelain"]).stdout.strip()


@pytest.mark.parametrize(
    "hook", ["reject", "stage-unrequested", "ancestry-error", "extra-parent"]
)
def test_queue_direct_materialization_keeps_hooks_and_enrolled_tree(
    drv, origin_and_clone, hook
):
    from tools import molt_dev_queue as queue

    _, work = origin_and_clone
    upstream = _git(work, "rev-parse", "origin/main").stdout.strip()
    source = _lane(work, "hooked", {"feature.py": "value = 1\n"})
    _, requests = _enqueue(drv, work, source)
    foreign = None
    if hook == "extra-parent":
        foreign = _lane(work, "foreign", {"foreign.py": "unrequested = True\n"})
        commit_hook = work / ".git/hooks/post-commit"
        body = (
            "set -eu\n"
            "original=$(git rev-parse HEAD)\n"
            "parent=$(git rev-parse HEAD^)\n"
            "tree=$(git rev-parse 'HEAD^{tree}')\n"
            f'rewritten=$(printf \'unrequested parent\\n\' | git commit-tree "$tree" -p "$parent" -p {foreign})\n'
            'git update-ref HEAD "$rewritten" "$original"\n'
        )
    else:
        commit_hook = work / ".git/hooks/pre-commit"
        body = (
            "exit 31\n"
            if hook == "reject"
            else "printf 'unexpected\\n' > unrequested.py\ngit add -- unrequested.py\n"
        )
    commit_hook.write_text("#!/bin/sh\n" + body, encoding="utf-8")
    commit_hook.chmod(0o755)
    git = drv.Git(work)

    def run(arguments):
        if hook == "ancestry-error" and list(arguments[:2]) == [
            "merge-base",
            "--is-ancestor",
        ]:
            return queue.GitResult(2, "", "cannot read commit graph")
        result = git._run(list(arguments), check=False)
        return queue.GitResult(result.returncode, result.stdout, result.stderr)

    diagnostic = {
        "reject": "git commit .*failed",
        "stage-unrequested": "enrolled source tree",
        "ancestry-error": "cannot establish enrolled lane's upstream ancestry",
        "extra-parent": "enrolled source tree",
    }[hook]
    with pytest.raises(drv.DriverError, match=diagnostic):
        queue.build_candidate(run, upstream, requests)
    if hook == "extra-parent":
        assert _git(work, "rev-list", "--parents", "-n", "1", "HEAD").stdout.split()[
            1:
        ] == [source, foreign]
        assert (
            _git(work, "rev-parse", "HEAD^{tree}").stdout
            == _git(work, "rev-parse", f"{source}^{{tree}}").stdout
        )


@pytest.mark.parametrize("advance_before_integrate", [False, True])
def test_queue_preserves_source_merges_and_resolves_their_citations(
    drv, origin_and_clone, tmp_path, monkeypatch, advance_before_integrate
):
    _, work = origin_and_clone
    counter = tmp_path / "gate-runs.txt"
    _counting_policy(work, counter)
    ordinary = _lane(work, "merged", {"feature.py": "value = 1\n"})
    lane = work.parent / "lane-merged"
    _commit_file(work, "incoming.txt", "main advanced\n", "advance main")
    _git(work, "push", "-q", "origin", "HEAD:main")
    _git(lane, "fetch", "-q", "origin")
    upstream = _git(lane, "rev-parse", "origin/main").stdout.strip()
    _git(lane, "merge", "--no-ff", "--no-commit", upstream)
    change = "I" + "72" * 20
    note = lane / "docs" / "merge.md"
    note.parent.mkdir()
    note.write_text(f"The source merge: change:{change}\n", encoding="utf-8")
    _git(lane, "add", "docs/merge.md")
    merged = _commit_file(
        lane,
        "resolution-only.txt",
        "manual resolution absent from both parents\n",
        f"resolve the source merge\n\nChange-Id: {change}",
    )
    for parent in (ordinary, upstream):
        assert (
            _git(
                work, "cat-file", "-e", f"{parent}:resolution-only.txt", check=False
            ).returncode
            != 0
        )
    paths, requests = _enqueue(drv, work, merged)
    if advance_before_integrate:
        integrate = drv.cmd_integrate
        moved = False

        def advance_before_integration(args):
            nonlocal moved
            if not moved:
                moved = True
                _commit_file(
                    work,
                    "late-main.txt",
                    "published after batch selection\n",
                    "late main",
                )
                _git(work, "push", "-q", "origin", "HEAD:main")
            return integrate(args)

        monkeypatch.setattr(drv, "cmd_integrate", advance_before_integration)
    commands = _record_driver_git_commands(drv, monkeypatch)

    assert _run_queue(drv, work) == drv.EXIT_OK

    assert _statuses(paths, requests) == ["landed"]
    _git(work, "fetch", "-q", "origin")
    assert (
        _git(work, "merge-base", "--is-ancestor", merged, "origin/main").returncode == 0
    )
    assert _git(work, "show", "origin/main:resolution-only.txt").stdout == (
        "manual resolution absent from both parents\n"
    )
    assert _git(work, "show", "origin/main:docs/merge.md").stdout == (
        f"The source merge: {merged[:9]}\n"
    )
    assert counter.read_text(encoding="utf-8").splitlines() == ["run"]
    if advance_before_integrate:
        assert moved
        assert _git(work, "show", "origin/main:late-main.txt").stdout == (
            "published after batch selection\n"
        )
    assert not any(args[0] in {"cherry-pick", "rebase"} for args in commands)
    _assert_no_patch_diff_work(commands)


def test_queue_bisects_a_failed_batch_and_lands_the_rest(
    drv, origin_and_clone, tmp_path, capsys
):
    _, work = origin_and_clone
    counter = tmp_path / "gate-runs.txt"
    _counting_policy(work, counter, check="test ! -e broken.txt")
    heads = [
        _lane(work, "a", {"a.py": "a = 1\n"}),
        _lane(work, "b", {"b.py": "b = 1\n"}),
        _lane(work, "c", {"c.py": "c = 1\n"}),
        _lane(work, "d", {"d.py": "d = 1\n"}),
    ]
    # The failing payload exists only in a real source merge, so flattening
    # ordinary commits would incorrectly admit this lane instead of bisecting.
    lane = work.parent / "lane-c"
    _git(lane, "checkout", "-qb", "source-side", "origin/main")
    side = _commit_file(lane, "side.txt", "side\n", "source side")
    _git(lane, "checkout", "-q", "codex/c")
    _git(lane, "merge", "--no-ff", "--no-commit", side)
    heads[2] = _commit_file(
        lane, "broken.txt", "breaks the gate\n", "manual resolution"
    )
    paths, requests = _enqueue(drv, work, *heads)

    assert _run_queue(drv, work) == drv.EXIT_OK

    assert _statuses(paths, requests) == ["landed", "landed", "failed", "landed"]
    # The runner's own output names the failed request and why, not only its
    # result file.
    assert f"FAIL: {requests[2].id} failed: " in capsys.readouterr().err
    # [a b c d] fails, [a b] lands, [c d] fails, [c] fails alone, [d] lands.
    assert len(counter.read_text(encoding="utf-8").splitlines()) == 5
    tree = _origin_tree(work)
    assert {"a.py", "b.py", "d.py"} <= tree and "broken.txt" not in tree
    from tools import molt_dev_queue as queue

    assert "gate failed" in queue.read_result(paths, requests[2].id)["detail"]


def test_queue_leaves_a_conflicting_lane_out_of_its_batch(
    drv, origin_and_clone, tmp_path
):
    _, work = origin_and_clone
    _counting_policy(work, tmp_path / "gate-runs.txt")
    first = _lane(work, "first", {"shared.txt": "first\n"})
    second = _lane(work, "second", {"shared.txt": "second\n"})
    third = _lane(work, "third", {"third.txt": "third\n"})
    paths, requests = _enqueue(drv, work, first, second, third)

    assert _run_queue(drv, work) == drv.EXIT_OK

    assert _statuses(paths, requests) == ["landed", "conflict", "landed"]
    _git(work, "fetch", "-q", "origin")
    assert _git(work, "show", "origin/main:shared.txt").stdout == "first\n"
    assert _git(work, "show", "origin/main:third.txt").stdout == "third\n"
    from tools import molt_dev_queue as queue

    detail = queue.read_result(paths, requests[1].id)["detail"]
    assert "does not apply" in detail


def test_queue_rebuilds_a_batch_when_main_moves_under_it(
    drv, origin_and_clone, tmp_path
):
    origin, work = origin_and_clone
    other = tmp_path / "other"
    _git(tmp_path, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "o@molt.test")
    _git(other, "config", "user.name", "Other")
    once = tmp_path / "moved-once"
    # The first gate run publishes an unrelated commit before the push.
    move = (
        f"if [ ! -e {shlex.quote(str(once))} ]; then touch {shlex.quote(str(once))}"
        f" && git -C {shlex.quote(str(other))} pull -q --rebase"
        f" && echo x > {shlex.quote(str(other / 'moved.txt'))}"
        f" && git -C {shlex.quote(str(other))} add moved.txt"
        f" && git -C {shlex.quote(str(other))} commit -qm moved"
        f" && git -C {shlex.quote(str(other))} push -q origin HEAD:main; fi"
    )
    _counting_policy(work, tmp_path / "gate-runs.txt", check=move)
    head = _lane(work, "a", {"a.py": "a = 1\n"})
    paths, requests = _enqueue(drv, work, head)

    assert _run_queue(drv, work) == drv.EXIT_OK

    assert _statuses(paths, requests) == ["landed"]
    assert {"a.py", "moved.txt"} <= _origin_tree(work)
    runs = (tmp_path / "gate-runs.txt").read_text(encoding="utf-8")
    assert len(runs.splitlines()) == 2


def _land_without_runner(drv, monkeypatch, lane: Path, *extra: str) -> int:
    """`land` in process; the detached runner it would start is not needed."""
    from tools import molt_dev_queue as queue

    started: list[Path] = []
    monkeypatch.setattr(
        queue, "_start_queue_runner", lambda paths, repo, args: started.append(repo)
    )
    args = drv.build_parser().parse_args(
        ["land", "--repo", str(lane), "--candidate-controller", *extra]
    )
    return args.func(args)


def test_land_runs_static_gates_in_the_lane_before_queueing(
    drv, origin_and_clone, tmp_path, monkeypatch
):
    from tools import molt_dev_queue as queue

    _, work = origin_and_clone
    counter = tmp_path / "gate-runs.txt"
    _counting_policy(work, counter)
    head = _lane(work, "pre", {"a.py": "a = 1\n"})
    lane = work.parent / "lane-pre"
    # The precheck admits under the gates' baseline, which alone sets this key.
    monkeypatch.delenv("PYTHONPYCACHEPREFIX", raising=False)
    admitted = []
    prepare = GateConfig.prepare

    def recorded_admission(self, gates, *, baseline, prepare_python):
        admitted.append(baseline)
        return prepare(self, gates, baseline=baseline, prepare_python=prepare_python)

    monkeypatch.setattr(GateConfig, "prepare", recorded_admission)

    assert _land_without_runner(drv, monkeypatch, lane) == drv.EXIT_OK
    assert admitted and all("PYTHONPYCACHEPREFIX" in env for env in admitted)

    assert counter.read_text(encoding="utf-8").splitlines() == ["run"]
    assert [item.head for item in queue.pending(queue.queue_paths(lane))] == [head]


def test_land_queues_nothing_when_a_static_gate_fails_in_the_lane(
    drv, origin_and_clone, tmp_path, monkeypatch
):
    from tools import molt_dev_queue as queue

    _, work = origin_and_clone
    counter = tmp_path / "gate-runs.txt"
    _counting_policy(work, counter, check="test ! -e broken.txt")
    _lane(work, "broken", {"broken.txt": "the gate refuses this file\n"})
    lane = work.parent / "lane-broken"

    with pytest.raises(CandidateFailure):
        _land_without_runner(drv, monkeypatch, lane)

    assert counter.read_text(encoding="utf-8").splitlines() == ["run"]
    assert queue.pending(queue.queue_paths(lane)) == []


def test_land_no_precheck_queues_without_running_gates(
    drv, origin_and_clone, tmp_path, monkeypatch
):
    from tools import molt_dev_queue as queue

    _, work = origin_and_clone
    counter = tmp_path / "gate-runs.txt"
    _counting_policy(work, counter, check="false")
    head = _lane(work, "skip", {"b.py": "b = 2\n"})
    lane = work.parent / "lane-skip"

    assert _land_without_runner(drv, monkeypatch, lane, "--no-precheck") == (
        drv.EXIT_OK
    )

    assert not counter.exists()
    assert [item.head for item in queue.pending(queue.queue_paths(lane))] == [head]


def test_land_waits_for_a_real_runner_through_the_pinned_controller(
    driver_origin_and_clone, tmp_path
):
    # The production path: `land` spawns a detached runner, which hands off
    # to main's pinned driver and lands the request from the queue sandbox.
    _, work = driver_origin_and_clone
    (work / "tools" / "proof_plan.toml").write_text(
        'always = ["echo queue-gate"]\n', encoding="utf-8"
    )
    _git(work, "add", "--", "tools/proof_plan.toml")
    _git(work, "commit", "-q", "--no-verify", "-m", "main driver")
    _git(work, "push", "-q", "origin", "HEAD:main")
    lane = tmp_path / "lane"
    _git(work, "worktree", "add", "-q", "-b", "codex/queued", str(lane))
    tip = _commit_file(lane, "queued.txt", "queued\n", "queued work")

    # Integrate's gates export the candidate's import roots, as here. `land`
    # runs as main's `python -I` controller; the runner and supervisor it
    # starts must keep that isolation, or they import the candidate's modules
    # and die at startup while `land --wait` waits.
    env = dict(os.environ)
    env["PYTHONPATH"] = os.pathsep.join((str(REPO_ROOT / "src"), str(REPO_ROOT)))
    result = run_guarded_test_process(
        [sys.executable, str(SCRIPT_PATH), "land", "--wait", "--repo", str(lane)],
        cwd=REPO_ROOT,
        env=ProcessEnvironment(env),
        timeout=600,
    )

    runner_diagnostics = (
        "\n".join(
            str(path) + ":\n" + path.read_text(encoding="utf-8")
            for path in (work / ".git/molt/landing-queue/runners").glob("*/run.log")
        )
        if result.returncode
        else ""
    )
    assert result.returncode == molt_dev.EXIT_OK, (
        result.stdout + result.stderr + runner_diagnostics
    )
    assert "landed in" in result.stderr
    _git(work, "fetch", "-q", "origin")
    landed = _git(work, "show", "origin/main:queued.txt").stdout
    assert landed == "queued\n"
    assert _git(work, "rev-parse", "origin/main").stdout.strip() != tip, "replayed"
    from molt.checkout_provisioning import selected_development_environment

    # Both checkouts declare the real driver project, but the selected echo
    # policy grants no source-Python environment. Observe persistent selection
    # state after the real child protocol, not a mocked provisioner call count.
    for checkout in (lane, work / ".git/molt/landing-sandbox"):
        assert selected_development_environment(checkout, os.environ) is None


def test_land_wait_fails_when_its_runner_exits_without_landing(tmp_path, monkeypatch):
    from tools import molt_dev_queue as queue

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    request = queue.enqueue(paths, head="1" * 40, worktree=tmp_path, branch=None)
    runner = paths.runners / "runner-1"
    runner.mkdir(parents=True)
    # A runner that has not finished may still be starting: keep waiting.
    assert queue._runner_exited_without(paths, request.id) is None
    (runner / "rc").write_text("1\n", encoding="utf-8")
    monkeypatch.setattr(queue, "_AWAIT_POLL_MIN_S", 0.01)
    monkeypatch.setattr(queue, "_AWAIT_POLL_MAX_S", 0.01)

    assert queue._await_landing(paths, request.id) == molt_dev.EXIT_FAIL
    assert queue._runner_exited_without(paths, request.id) == runner


@pytest.mark.parametrize("recorded", ["0", "-7", str(2**32 + 1)])
def test_land_wait_reads_a_pid_that_names_no_process_as_unwritten(tmp_path, recorded):
    from tools import molt_dev_queue as queue

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    request = queue.enqueue(paths, head="1" * 40, worktree=tmp_path, branch=None)
    older, newest = paths.runners / "runner-1", paths.runners / "runner-2"
    for runner in (older, newest):
        runner.mkdir(parents=True)
        (runner / "pid").write_text(recorded, encoding="utf-8")
    # A corrupt pid proves no exit: the newest runner may still be starting.
    assert queue._runner_exited_without(paths, request.id) is None
    # An older runner with the same pid file is not the one still starting.
    (newest / "rc").write_text("1\n", encoding="utf-8")
    assert queue._runner_exited_without(paths, request.id) == newest


def test_land_wait_looks_again_at_a_doubling_interval(tmp_path, monkeypatch):
    from tools import molt_dev_queue as queue
    from tests.process_guard_common import install_module_view

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    request = queue.enqueue(paths, head="1" * 40, worktree=tmp_path, branch=None)
    waits: list[float] = []

    def wait(seconds: float) -> None:
        waits.append(seconds)
        if len(waits) == 7:
            queue.record_result(
                paths, request, status="landed", detail="", landed="2" * 40
            )

    install_module_view(monkeypatch, "time", time, queue, sleep=wait)

    assert queue._await_landing(paths, request.id) == molt_dev.EXIT_OK
    # From 0.05 s, each wait doubles, up to one second.
    assert waits == pytest.approx([0.05, 0.1, 0.2, 0.4, 0.8, 1.0, 1.0])


# Resolve the launcher's recorded supervisor, never the possibly adopted PPID.
# Explicit release and a bounded lifetime also close failed test assertions.
_ORPHAN_EXITING_SLEEPER = (
    "import os, sys, time\n"
    "from pathlib import Path\n"
    "pid_path = Path(sys.argv[1])\n"
    "state = pid_path.parent\n"
    "release = state / 'command-release'\n"
    "deadline = time.monotonic() + 30\n"
    "(state / 'command-pid').write_text(str(os.getpid()), encoding='utf-8')\n"
    "try:\n"
    "    while not pid_path.exists() and not release.exists():\n"
    "        assert time.monotonic() < deadline, 'supervisor pid publication deadline'\n"
    "        time.sleep(0.01)\n"
    "    if not release.exists():\n"
    "        supervisor = int(pid_path.read_text(encoding='utf-8'))\n"
    "        while os.getppid() == supervisor and not release.exists():\n"
    "            assert time.monotonic() < deadline, 'test command release deadline'\n"
    "            time.sleep(0.01)\n"
    "finally:\n"
    "    (state / 'command-done').touch()\n"
)


def _release_orphan_sleeper(runner, supervisor):
    from molt.process_model import pid_alive

    (runner / "command-release").touch()
    deadline = time.monotonic() + 30
    while not (runner / "command-done").exists():
        assert time.monotonic() < deadline, "test command did not release"
        time.sleep(0.01)
    command = int((runner / "command-pid").read_text(encoding="utf-8"))
    while pid_alive(command):
        assert time.monotonic() < deadline, "test command did not exit"
        time.sleep(0.01)
    while os.waitpid(supervisor, os.WNOHANG)[0] == 0:
        assert time.monotonic() < deadline, "test supervisor did not exit"
        time.sleep(0.01)


@pytest.mark.parametrize("adoptive_parent", [1, 4321])
def test_orphan_sleeper_exits_when_started_after_reparenting(tmp_path, adoptive_parent):
    import builtins
    from tests.process_guard_common import ModuleView

    # The detached launcher already recorded the original supervisor, but the
    # command's first Python instruction runs after adoption by init/a subreaper.
    (tmp_path / "pid").write_text("9999", encoding="utf-8")

    def unexpected_sleep(_seconds):
        raise AssertionError("late-start command waited on its adoptive parent")

    imports = {
        "os": ModuleView(os, getppid=lambda: adoptive_parent),
        "sys": ModuleView(sys, argv=["-c", str(tmp_path / "pid")]),
        "time": ModuleView(time, sleep=unexpected_sleep),
    }

    def selected_import(name, *args, **kwargs):
        if name in imports:
            return imports[name]
        return builtins.__import__(name, *args, **kwargs)

    exec(
        _ORPHAN_EXITING_SLEEPER,
        {"__builtins__": {**vars(builtins), "__import__": selected_import}},
    )


@pytest.mark.skipif(
    os.name != "posix", reason="an unreaped child (a zombie) is a POSIX state"
)
def test_land_wait_counts_every_runner_that_may_still_land(tmp_path):
    """An older runner waiting for the lock may land the request after the
    newest runner exits; a pid that another process now holds does not count.
    """
    import signal

    from molt import process_model
    from tools import molt_dev_queue as queue

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    request = queue.enqueue(paths, head="1" * 40, worktree=tmp_path, branch=None)
    # A stale record whose pid names a live process that is not its supervisor.
    stale = paths.runners / "runner-0"
    stale.mkdir(parents=True)
    (stale / "pid").write_text(str(os.getpid()), encoding="utf-8")
    # An older runner whose real detached supervisor is alive (waiting).
    runner = paths.runners / "runner-1"
    molt_dev.cmd_detached_run(
        _dr_ns(
            molt_dev,
            "runner-1",
            paths.runners,
            [sys.executable, "-c", _ORPHAN_EXITING_SLEEPER, str(runner / "pid")],
        )
    )
    supervisor = int((runner / "pid").read_text(encoding="utf-8"))
    try:
        deadline = time.monotonic() + 30
        while not (runner / "command-pid").exists():
            assert time.monotonic() < deadline, "test command did not start"
            time.sleep(0.01)
        # The newest runner has exited.
        newest = paths.runners / "runner-2"
        newest.mkdir()
        (newest / "pid").write_text(str(os.getpid()), encoding="utf-8")
        (newest / "rc").write_text("1\n", encoding="utf-8")

        assert queue._runner_exited_without(paths, request.id) is None

        os.kill(supervisor, signal.SIGKILL)  # This test owns exactly this child.
        # The signal is delivered asynchronously; the kernel then lists an
        # unreaped zombie, which counts as gone.
        while process_model.pid_alive(supervisor):
            assert time.monotonic() < deadline, "test supervisor did not exit"
            time.sleep(0.01)
        assert queue._runner_exited_without(paths, request.id) == newest
    finally:
        _release_orphan_sleeper(runner, supervisor)


@pytest.mark.skipif(
    os.name != "posix", reason="an unreaped child (a zombie) is a POSIX state"
)
@pytest.mark.parametrize("failure_stage", [None, "before-kill", "after-kill"])
def test_land_wait_fails_when_its_runner_supervisor_dies_without_rc(
    tmp_path, monkeypatch, failure_stage
):
    import signal

    from tools import molt_dev_queue as queue
    from tests.process_guard_common import install_module_view

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    request = queue.enqueue(paths, head="1" * 40, worktree=tmp_path, branch=None)
    # A real detached supervisor, started by this process as `land` starts it.
    runner = paths.runners / "runner-1"
    molt_dev.cmd_detached_run(
        _dr_ns(
            molt_dev,
            "runner-1",
            paths.runners,
            [sys.executable, "-c", _ORPHAN_EXITING_SLEEPER, str(runner / "pid")],
        )
    )
    supervisor = int((runner / "pid").read_text(encoding="utf-8"))
    injected = AssertionError("injected test failure")
    observed_failure = None
    try:
        # Observe the actual command, rather than assuming the supervisor has
        # exec'd it when its own readiness record becomes available.
        deadline = time.monotonic() + 30
        while not (runner / "command-pid").exists():
            assert time.monotonic() < deadline, "test command did not start"
            time.sleep(0.01)
        # A live supervisor without rc may still land the request: keep waiting.
        assert queue._runner_exited_without(paths, request.id) is None
        if failure_stage == "before-kill":
            raise injected

        # Dead before it wrote rc. It stays this process's unreaped child, as the
        # supervisor stays `land`'s, so `kill(pid, 0)` still finds it.
        os.kill(supervisor, signal.SIGKILL)
        if failure_stage == "after-kill":
            raise injected
        deadline = time.monotonic() + 30

        def bounded_sleep(seconds: float) -> None:
            assert time.monotonic() < deadline, "land --wait waits on a dead runner"
            time.sleep(seconds)

        install_module_view(monkeypatch, "time", time, queue, sleep=bounded_sleep)
        monkeypatch.setattr(queue, "_AWAIT_POLL_MIN_S", 0.01)
        monkeypatch.setattr(queue, "_AWAIT_POLL_MAX_S", 0.01)

        assert queue._await_landing(paths, request.id) == molt_dev.EXIT_FAIL
        assert not (runner / "rc").exists()
        assert queue._runner_exited_without(paths, request.id) == runner
        # The kernel still lists the supervisor: the wait saw a zombie, unreaped.
        os.kill(supervisor, 0)
    except AssertionError as error:
        assert failure_stage is not None and error is injected
        observed_failure = error
    finally:
        _release_orphan_sleeper(runner, supervisor)
    assert (observed_failure is injected) == (failure_stage is not None)


def test_queue_plan_survives_between_runners_and_drops_closed_requests(tmp_path):
    from tools import molt_dev_queue as queue

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    requests = [
        queue.enqueue(paths, head=f"{index:040x}", worktree=tmp_path, branch=None)
        for index in range(1, 6)
    ]
    batch = queue.next_batch(paths, queue.pending(paths))
    assert batch == requests
    # requests[4] conflicted and left; the four that applied failed together.
    queue.record_result(
        paths, requests[4], status=queue.LandingStatus.CONFLICT, detail=""
    )
    queue.bisect(paths, batch, requests[:4])
    batch = queue.next_batch(paths, queue.pending(paths))
    assert batch == requests[:2]
    for request in batch:
        queue.record_result(
            paths, request, status=queue.LandingStatus.LANDED, detail="", landed=None
        )
    queue.retire_batch_plan(paths, batch)
    assert queue.next_batch(paths, queue.pending(paths)) == requests[2:4]
    # A request closed outside the runner drops out of the plan.
    queue.record_result(
        paths, requests[2], status=queue.LandingStatus.FAILED, detail=""
    )
    assert queue.next_batch(paths, queue.pending(paths)) == requests[3:4]
    queue.record_result(
        paths, requests[3], status=queue.LandingStatus.LANDED, detail="", landed=None
    )
    assert queue.next_batch(paths, queue.pending(paths)) == []
    assert not paths.plan.exists()


@pytest.mark.parametrize("recent", [0, 1, 3, 100])
def test_queue_status_bounds_result_bodies_and_live_inventory(
    drv, tmp_path, monkeypatch, capsys, recent
):
    import weakref
    from datetime import UTC, datetime, timedelta

    from tools import molt_dev_queue as queue

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    waiting = queue.enqueue(paths, head="f" * 40, worktree=tmp_path, branch=None)
    queue._write_plan(paths, [[waiting.id]])
    paths.results.mkdir()
    # File names, enqueue order and completion order deliberately disagree.
    records = {
        f"{index:03d}.json": {
            "result": index,
            "recorded_at": (
                datetime(2026, 10, 21, tzinfo=UTC)
                + timedelta(seconds=(index * 29 % 64) // 2)
            ).isoformat(),
            "request": {"id": f"{63 - index:040x}"},
        }
        for index in range(64)
    }
    for index in range(64):
        name = f"{index * 17 % 64:03d}.json"
        (paths.results / name).write_text(json.dumps(records[name]), encoding="utf-8")
    (paths.results / ".unpublished.json").write_text("not JSON", encoding="utf-8")
    live = weakref.WeakSet()
    max_live = 0
    glob_calls = 0
    reads = []
    original_loads = json.loads

    class ObservedBody(dict):
        __hash__ = object.__hash__

    def observe_loads(value, *args, **kwargs):
        nonlocal max_live
        parsed = original_loads(value, *args, **kwargs)
        if isinstance(parsed, dict) and set(parsed) == {
            "result",
            "recorded_at",
            "request",
        }:
            parsed = ObservedBody(parsed)
            live.add(parsed)
            max_live = max(max_live, len(live))
        return parsed

    class ObservedResult:
        def __init__(self, path):
            self.path = path
            self.name = path.name

        def read_text(self, *, encoding):
            reads.append(self.name)
            return self.path.read_text(encoding=encoding)

    class ObservedQueuePath(type(tmp_path)):
        def glob(self, pattern):
            nonlocal glob_calls
            entries = super().glob(pattern)
            if self != paths.results:
                return entries
            glob_calls += 1
            return (ObservedResult(path) for path in entries)

    paths = queue.QueuePaths(ObservedQueuePath(paths.root))
    monkeypatch.setattr(queue, "queue_paths", lambda _repo: paths)
    monkeypatch.setattr(queue.json, "loads", observe_loads)
    args = drv.build_parser().parse_args(
        ["queue", "status", "--repo", str(tmp_path), "--recent", str(recent)]
    )
    assert args.func(args) == drv.EXIT_OK
    report = original_loads(capsys.readouterr().out)
    expected = sorted(
        records.values(),
        key=lambda row: (
            datetime.fromisoformat(row["recorded_at"]),
            row["request"]["id"],
        ),
    )
    assert report == {
        "pending": [waiting.id],
        "plan": [[waiting.id]],
        "recent": expected[-recent:] if recent else [],
    }
    # A positive selection must read every completion timestamp exactly once.
    assert sorted(reads) == (sorted(records) if recent else [])
    assert glob_calls == (1 if recent else 0)
    # Observe actual retained bodies, rather than temporary path wrappers.
    assert max_live <= min(recent, len(records)) + 2


@pytest.mark.parametrize("recent", ["-1", "-100"])
def test_queue_status_rejects_negative_recent_before_queue_access(
    drv, tmp_path, monkeypatch, capsys, recent
):
    from tools import molt_dev_queue as queue

    def unexpected_queue_access(_repo):
        pytest.fail("negative result count inspected the queue")

    monkeypatch.setattr(queue, "queue_paths", unexpected_queue_access)
    assert (
        drv.main(["queue", "status", "--repo", str(tmp_path), "--recent", recent])
        == drv.EXIT_USAGE
    )
    output = capsys.readouterr()
    assert "--recent must be non-negative" in output.err
    assert output.out == ""


def test_queue_status_preserves_selected_result_parse_errors(
    drv, tmp_path, monkeypatch
):
    from tools import molt_dev_queue as queue

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    paths.results.mkdir(parents=True)
    (paths.results / "newest.json").write_text("not JSON", encoding="utf-8")
    monkeypatch.setattr(queue, "queue_paths", lambda _repo: paths)
    args = drv.build_parser().parse_args(
        ["queue", "status", "--repo", str(tmp_path), "--recent", "1"]
    )
    with pytest.raises(json.JSONDecodeError):
        args.func(args)


def test_queue_status_rejects_noninteger_recent(drv, capsys):
    with pytest.raises(SystemExit) as error:
        drv.build_parser().parse_args(["queue", "status", "--recent", "many"])
    assert error.value.code == drv.EXIT_USAGE
    assert "invalid int value" in capsys.readouterr().err


def test_queue_retires_old_runner_logs_and_results_only(tmp_path):
    from tools import molt_dev_queue as queue

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    now = 50_000_000.0
    for index in range(queue.RUNNER_LOGS_KEPT + 3):
        runner = paths.runners / f"runner-{index:03d}"
        runner.mkdir(parents=True)
        (runner / "rc").write_text("0\n", encoding="utf-8")
        os.utime(runner / "rc", (now - index, now - index))
    live = paths.runners / "runner-live"
    live.mkdir()
    old = queue.enqueue(paths, head="1" * 40, worktree=tmp_path, branch=None)
    recent = queue.enqueue(paths, head="2" * 40, worktree=tmp_path, branch=None)
    waiting = queue.enqueue(paths, head="3" * 40, worktree=tmp_path, branch=None)
    for request, age_days in ((old, queue.RESULT_DAYS + 1), (recent, 1)):
        queue.record_result(
            paths, request, status=queue.LandingStatus.LANDED, detail="", landed=None
        )
        result = paths.results / f"{request.id}.json"
        os.utime(result, (now - age_days * 86400, now - age_days * 86400))

    held = queue.enqueue(paths, head="4" * 40, worktree=tmp_path, branch=None)
    queue.record_result(
        paths,
        held,
        status=queue.LandingStatus.LANDED_RETIREMENT_FAILED,
        detail="unharvested signal",
        landed="4" * 40,
    )
    held_path = paths.results / f"{held.id}.json"
    os.utime(held_path, (0, 0))
    queue.retire_old_records(paths, now=now)
    assert queue.read_result(paths, held.id)["detail"] == "unharvested signal"

    kept = sorted(entry.name for entry in paths.runners.iterdir())
    expected = [f"runner-{index:03d}" for index in range(queue.RUNNER_LOGS_KEPT)]
    assert kept == sorted(expected + ["runner-live"])
    assert queue.read_result(paths, old.id) is None
    assert queue.read_result(paths, recent.id)["status"] == "landed"
    assert queue.pending(paths) == [waiting]


@pytest.mark.parametrize("outcome", ["success", "driver-error", "interrupt"])
def test_integrate_lock_released_on_every_exclusive_exit(
    drv, origin_and_clone, monkeypatch, outcome
):
    from molt import file_locks

    _, work = origin_and_clone
    lock_path = work / ".git" / "molt-integrate.lock"
    held_identity = None

    def exclusive(args, *, progress):
        nonlocal held_identity
        assert Path(args.repo) == work
        stat = lock_path.stat()
        held_identity = (stat.st_dev, stat.st_ino)
        unexpected = file_locks._try_acquire_file_lock(lock_path, create=False)
        if unexpected is not None:
            file_locks._release_file_lock(unexpected)
            pytest.fail("exclusive body entered without its integration lock")
        if outcome == "driver-error":
            raise drv.DriverError("independent integration failure")
        if outcome == "interrupt":
            raise KeyboardInterrupt("independent integration interruption")
        return 73

    monkeypatch.setattr(drv, "_integrate_exclusive", exclusive)
    args = _integrate_ns(drv, repo=str(work))
    if outcome == "success":
        assert drv.cmd_integrate(args) == 73
    elif outcome == "driver-error":
        with pytest.raises(drv.DriverError, match="independent integration failure"):
            drv.cmd_integrate(args)
    else:
        with pytest.raises(
            KeyboardInterrupt, match="independent integration interruption"
        ):
            drv.cmd_integrate(args)
    # A stable file remains; only the OS handle's lifetime governs ownership.
    stat = lock_path.stat()
    assert (stat.st_dev, stat.st_ino) == held_identity
    handle = file_locks._try_acquire_file_lock(lock_path, create=False)
    assert handle is not None, "terminal integration leaked its lock"
    file_locks._release_file_lock(handle)


def test_integrate_lock_covers_fetch_gates_push_confirmation_and_cleanup(
    drv, origin_and_clone, monkeypatch
):
    from molt import file_locks

    _, work = origin_and_clone
    _commit_file(work, "new-source.txt", "source\n", "source for complete integration")
    lock_path = work / ".git" / "molt-integrate.lock"
    observed = []

    def require_held(phase):
        handle = file_locks._try_acquire_file_lock(lock_path, create=False)
        if handle is not None:
            file_locks._release_file_lock(handle)
            pytest.fail(f"integration lock released before {phase}")
        observed.append(phase)

    original_git = drv.Git._run
    original_gate = drv.run_gate

    def git_run(self, args, **kwargs):
        if args[0] in {"fetch", "push"}:
            require_held(args[0])
        return original_git(self, args, **kwargs)

    def gate(command, repo, env):
        require_held("gate")
        return original_gate(command, repo, env)

    def cleanup(*args, **kwargs):
        require_held("cleanup")
        return drv.EXIT_OK

    monkeypatch.setattr(drv.Git, "_run", git_run)
    monkeypatch.setattr(drv, "run_captured_gate", _captured_gate(gate))
    # Cleanup's own destructive/dirty-tree contract has separate real-worktree
    # tests above; this seam asserts the integration lock remains held at entry.
    monkeypatch.setattr(molt_dev_retire, "cleanup_worktree", cleanup)
    assert (
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), cleanup_worktree=True))
        == drv.EXIT_OK
    )
    assert observed == ["fetch", "gate", "push", "fetch", "cleanup"]
    handle = file_locks._try_acquire_file_lock(lock_path, create=False)
    assert handle is not None
    file_locks._release_file_lock(handle)


@pytest.mark.parametrize("explicit", [False, True])
def test_integrate_gates_build_into_the_checkouts_cargo_target(
    drv, origin_and_clone, monkeypatch, tmp_path, explicit
):
    from molt.dx import project_cargo_target_dir

    _, work = origin_and_clone
    _commit_file(work, "gate-source.txt", "source\n", "source for gate target")
    monkeypatch.setenv("MOLT_PREFER_EXTERNAL_ARTIFACTS", "1")
    if explicit:
        monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "operator-target"))
    else:
        monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    targets = []

    def gate(command, repo, env):
        targets.append((env.environment["CARGO_TARGET_DIR"], dict(env.environment)))
        return 0

    monkeypatch.setattr(drv, "run_captured_gate", _captured_gate(gate))
    assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work))) == drv.EXIT_OK
    assert targets
    for target, env in targets:
        if explicit:
            assert target == str(tmp_path / "operator-target")
            continue
        # The target Molt's own builds of this checkout select, which outlives
        # the worktree: never the worktree's ./target.
        without = {
            key: value for key, value in env.items() if key != "CARGO_TARGET_DIR"
        }
        assert target == str(project_cargo_target_dir(work.resolve(), without))
        assert not Path(target).is_relative_to(work.resolve())


@pytest.mark.parametrize("outcome", ["success", "nonzero", "interrupt", "exception"])
def test_integration_progress_preserves_gate_outcomes(
    drv, origin_and_clone, monkeypatch, capsys, outcome
):
    from datetime import datetime

    _, work = origin_and_clone
    tip = _commit_file(work, "change.txt", "changed\n", "candidate")
    upstream = _git(work, "rev-parse", "origin/main").stdout.strip()
    shell = "printf '%s\\n' 'literal shell text'"
    argv = ("python", "-c", "print('literal [argument]')", "line\nbreak")
    commands = [shell, argv, "echo final-gate"]
    selected = [SelectedGate(command, ()) for command in commands]
    monkeypatch.setattr(GateConfig, "select", lambda *a, **kw: (selected, []))
    seen = []
    selected_environments = []

    def run_gate(command, repo, environment):
        seen.append(command)
        selected_environments.append(environment.environment)
        if command == argv:
            if outcome == "interrupt":
                raise KeyboardInterrupt("independent interrupted child")
            if outcome == "exception":
                raise drv.DriverError("independent launch failure")
            if outcome == "nonzero":
                return 7
        return 0

    monkeypatch.setattr(drv, "run_captured_gate", _captured_gate(run_gate))
    monkeypatch.setenv("MOLT_EVENT_SECRET_CONTROL", "must-not-appear-in-events")
    from molt import dx

    published = []

    def collect(env, *, cwd):
        output = capsys.readouterr().out
        terminal = json.loads(output.splitlines()[-1])
        assert terminal["event"] == "integration_finished"
        assert env is selected_environments[-1]
        assert cwd == work
        published.append(output)

    monkeypatch.setattr(dx, "collect_checkout_state_after_result", collect)
    args = _integrate_ns(drv, repo=str(work), no_push=True)
    if outcome == "interrupt":
        with pytest.raises(KeyboardInterrupt, match="independent interrupted child"):
            drv.cmd_integrate(args)
    elif outcome in {"exception", "nonzero"}:
        with pytest.raises(drv.DriverError):
            drv.cmd_integrate(args)
    else:
        try:
            raise LookupError("unrelated caller exception")
        except LookupError:
            assert drv.cmd_integrate(args) == 0
    assert len(published) == 1
    output = "".join(published) + capsys.readouterr().out
    events = [json.loads(line) for line in output.splitlines()]
    expected_commands = commands if outcome == "success" else commands[:2]
    assert seen == expected_commands
    assert [event["event"] for event in events] == [
        "candidate_selected",
        *[
            name
            for _ in expected_commands
            for name in ("gate_started", "gate_finished")
        ],
        "integration_finished",
    ]
    assert "must-not-appear-in-events" not in output
    assert all(event["schema"] == "molt.integration-progress.v1" for event in events)
    assert all(event["advisory"] is True for event in events)
    assert all(event["source_commit"] == tip for event in events)
    assert all(event["upstream_commit"] == upstream for event in events)
    assert all(event["gate_total"] == 3 for event in events)
    assert all(
        datetime.fromisoformat(event["recorded_at"]).utcoffset().total_seconds() == 0
        for event in events
    )
    clocks = [event["monotonic_seconds"] for event in events]
    assert clocks == sorted(clocks)
    assert all(event["elapsed_seconds"] >= 0 for event in events)
    for ordinal, command in enumerate(expected_commands, 1):
        start, finish = events[ordinal * 2 - 1 : ordinal * 2 + 1]
        expected = (
            {"kind": "shell", "source": command}
            if isinstance(command, str)
            else {"kind": "argv", "argv": list(command)}
        )
        assert start["command"] == finish["command"] == expected
        assert start["gate_ordinal"] == finish["gate_ordinal"] == ordinal
        assert start["toolchains"] == finish["toolchains"] == []
        assert finish["duration_seconds"] >= 0
    final_gate = events[-2]
    terminal = events[-1]
    expected_gate_rc = {
        "success": 0,
        "nonzero": 7,
        "interrupt": None,
        "exception": None,
    }
    assert final_gate["returncode"] == expected_gate_rc[outcome]
    assert (
        terminal["returncode"]
        == {"success": 0, "nonzero": 1, "interrupt": 130, "exception": 1}[outcome]
    )
    assert (
        terminal["status"]
        == {
            "success": "completed",
            "nonzero": "failed",
            "interrupt": "interrupted",
            "exception": "failed",
        }[outcome]
    )
    assert terminal["dry_run"] is False
    # A start event or successful prefix never claims whole-integration success.
    assert not any(event.get("status") == "completed" for event in events[:-1])
    assert _git(work, "rev-parse", "origin/main").stdout.strip() == upstream


def test_integration_progress_flushes_and_does_not_mask_output_failure(
    drv, monkeypatch
):
    writes = []

    def capture(value, *, flush):
        writes.append((json.loads(value), flush))

    monkeypatch.setattr(drv, "print", capture, raising=False)
    progress = drv._IntegrationProgress()
    progress.emit("candidate_selected")
    assert len(writes) == 1 and writes[0][1] is True
    assert writes[0][0]["source_commit"] is None
    assert writes[0][0]["upstream_commit"] is None

    def unavailable(*args, **kwargs):
        raise BrokenPipeError("closed diagnostics consumer")

    monkeypatch.setattr(drv, "print", unavailable)
    progress.emit("integration_finished", returncode=7)


def test_gate_time_summary_names_the_total_and_the_slowest_gates(drv):
    summary = drv.gate_time_summary(
        [(1.0, "ruff"), (612.25, "pytest serial"), (3.5, "clippy"), (40.0, "cargo")],
        top=2,
    )
    assert summary.splitlines() == [
        "    gates: 4 in 656.8 s; slowest:",
        "        612.2 s  pytest serial",
        "         40.0 s  cargo",
    ]


def test_integrate_dry_run_never_resolves_or_acquires_integration_lock(
    drv, origin_and_clone, monkeypatch
):
    _, work = origin_and_clone
    _commit_file(work, "dry-source.txt", "source\n", "dry-run source")
    lock_path = work / ".git" / "molt-integrate.lock"
    assert not lock_path.exists()

    def forbidden(*args, **kwargs):
        pytest.fail("dry-run entered the live integration lock policy")

    monkeypatch.setattr(drv, "_integration_lock_path", forbidden)
    monkeypatch.setattr(drv, "_try_acquire_file_lock", forbidden)
    commands = _record_driver_git_commands(drv, monkeypatch)
    assert (
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), dry_run=True))
        == drv.EXIT_OK
    )
    assert not lock_path.exists()
    assert not any(args[0] in {"fetch", "push", "rebase"} for args in commands)


# --------------------------------------------------------------------------
# namespace builders (argparse.Namespace with the fields each cmd reads)
# --------------------------------------------------------------------------


def _ns(drv, **kwargs):
    import argparse

    return argparse.Namespace(**kwargs)


def _integrate_ns(drv, **overrides):
    base = dict(
        repo=None,
        remote="origin",
        branch="main",
        dry_run=False,
        no_push=False,
        expected_upstream=None,
        candidate_controller=False,
        pinned_controller=None,
        wait_for_lock=False,
        marker=None,
        extra_gate=None,
        cleanup_worktree=False,
        ignore=None,
        session_id="devtool-test",
    )
    base.update(overrides)
    import argparse

    return argparse.Namespace(**base)


# --------------------------------------------------------------------------
# Hazard 11: backgrounded long-runs die silently (detached-run / detached-verify)
# --------------------------------------------------------------------------


def _dr_ns(drv, name, state_root, command, *, cwd=None, env=None, replace=False):
    options = [
        "detached-run",
        "--name",
        name,
        "--state-dir",
        str(state_root),
    ]
    if cwd is not None:
        options.extend(("--cwd", str(cwd)))
    for value in env or ():
        options.extend(("--env", value))
    if replace:
        options.append("--replace")
    return drv.build_parser().parse_args([*options, "--", *command])


def _dv_ns(drv, name, state_root, *, min_age_s=0.0, as_json=True):
    import argparse

    return argparse.Namespace(
        name=name, state_dir=str(state_root), min_age_s=min_age_s, json=as_json
    )


def _wait_done(
    drv, capsys, name: str, state_root: Path, timeout_s: float = 90.0
) -> dict:
    import time

    deadline = time.monotonic() + timeout_s
    last: dict | None = None
    while time.monotonic() < deadline:
        _rc, verdict = _verify_json(
            drv, capsys, _dv_ns(drv, name, state_root, min_age_s=0.0)
        )
        last = verdict
        if verdict["status"] == "done":
            return verdict
        if verdict["status"] == "died-silent":
            raise AssertionError(f"detached {name!r} died silently: {verdict}")
        time.sleep(0.1)
    raise AssertionError(
        f"detached {name!r} never completed within {timeout_s}s; last={last}"
    )


def _verify_json(drv, capsys, ns) -> tuple[int, dict]:
    import json as _json

    rc = drv.cmd_detached_verify(ns)
    out = capsys.readouterr().out
    payload = next(line for line in out.splitlines() if line.strip().startswith("{"))
    return rc, _json.loads(payload)


def test_subcommand_identity_destinations_never_alias_payloads(drv):
    import argparse

    def check(parser, identities):
        for action in parser._actions:
            assert action.dest not in identities, (parser.prog, action.dest)
            if isinstance(action, argparse._SubParsersAction):
                for child in action.choices.values():
                    check(child, identities | {action.dest})

    check(drv.build_parser(), set())


@pytest.mark.parametrize(
    ("argv", "pinned"),
    [
        (["lane", "start", "feature"], True),
        (["lane", "retire", "feature"], True),
        (["lane", "start", "feature", "--candidate-controller"], False),
        (["lane", "retire", "feature", "--candidate-controller"], False),
        (["lane", "list"], False),
        # The handoff itself: main's controller receives these and runs it.
        (
            [
                "lane",
                "start",
                "feature",
                "--repo",
                "/repo",
                "--pinned-controller",
                "a" * 32,
                "--controller-cwd",
                "/repo",
            ],
            False,
        ),
        (["land"], True),
        (["queue", "status"], False),
    ],
)
def test_lane_start_and_retire_run_mains_driver(drv, argv, pinned) -> None:
    """HF-468: a lane's start provisions it at main and its retirement judges
    its trees against main, so both run main's pinned driver, as land does;
    a driver at another commit read their manifests with another schema."""
    parsed = drv.build_parser().parse_args(argv)
    assert molt_dev.molt_dev_controller.runs_from_pinned_controller(parsed) is pinned


def test_detached_run_completes_and_records_rc(drv, tmp_path, capsys):
    state_root = tmp_path / "detached"
    # Exercise the complete dispatcher, not only the detached handler: payload
    # argv must never replace the subcommand identity used by the controller.
    argv = [
        "detached-run",
        "--name",
        "ok-run",
        "--state-dir",
        str(state_root),
        "--",
        sys.executable,
        "-c",
        "print('MARKER-OUT', flush=True)",
    ]
    parsed = drv.build_parser().parse_args(argv)
    assert parsed.command == "detached-run"
    payload = parsed.argv[1:] if parsed.argv[:1] == ["--"] else parsed.argv
    assert payload == argv[6:]
    assert not molt_dev.molt_dev_controller.runs_from_pinned_controller(parsed)
    rc = drv.main(argv)
    assert rc == drv.EXIT_OK
    state = state_root / "ok-run"
    assert _wait_done(drv, capsys, "ok-run", state_root)["rc"] == 0
    # Unbuffered log captured the child's stdout despite daemonization.
    assert "MARKER-OUT" in (state / "run.log").read_text(encoding="utf-8")
    # cmd.json records the exact argv for postmortems.
    import json as _json

    recorded = _json.loads((state / "cmd.json").read_text(encoding="utf-8"))
    assert recorded["argv"][0] == sys.executable
    vrc, verdict = _verify_json(drv, capsys, _dv_ns(drv, "ok-run", state_root))
    assert vrc == drv.EXIT_OK
    assert verdict["status"] == "done" and verdict["rc"] == 0


@pytest.mark.parametrize(
    "explicit_root", [False, True], ids=["default-root", "explicit-root"]
)
def test_detached_run_prints_a_verify_command_that_finds_its_state(
    drv, tmp_path, capsys, monkeypatch, explicit_root
):
    import tempfile

    from tools import molt_dev_detached as detached

    launch_temp = tmp_path / "launch temporary é"
    verify_temp = tmp_path / "later temporary é"
    unrelated_cwd = tmp_path / "unrelated cwd"
    for directory in (launch_temp, verify_temp, unrelated_cwd):
        directory.mkdir()
    launch_temp = launch_temp.resolve()
    verify_temp = verify_temp.resolve()
    # Observe a different default only in this test's detached module. Do not
    # change process-wide tempfile state used by guard/sentinel threads.
    install_thread_module_view(
        monkeypatch,
        "tempfile",
        tempfile,
        detached,
        gettempdir=lambda: str(launch_temp),
    )
    state_root = launch_temp / (
        "explicit runners" if explicit_root else "molt_dev_detached"
    )
    marker = tmp_path / "payload ran once"
    command = [
        sys.executable,
        "-c",
        "from pathlib import Path\n"
        f"with Path({str(marker)!r}).open('x', encoding='utf-8') as stream:\n"
        "    stream.write('ran once\\n')\n",
    ]
    options = [
        "detached-run",
        "--name",
        "hinted",
        "--json",
        "--verify-min-age-hint",
        "0",
    ]
    if explicit_root:
        options.extend(("--state-dir", str(state_root)))
    args = drv.build_parser().parse_args([*options, "--", *command])
    assert drv.cmd_detached_run(args) == 0
    printed = capsys.readouterr()
    launched = json.loads(printed.out)
    state = state_root / "hinted"
    argv = launched["verify_argv"]
    expected_prefix = [
        sys.executable,
        *(["-I"] if sys.flags.isolated else []),
        str(SCRIPT_PATH),
    ]
    assert argv == [
        *expected_prefix,
        "detached-verify",
        "--name",
        "hinted",
        "--state-dir",
        str(state_root),
        "--min-age-s",
        "0",
    ]
    assert Path(argv[0]).is_absolute() and SCRIPT_PATH.is_absolute()
    assert launched["name"] == "hinted" and launched["state_dir"] == str(state)
    assert type(launched["pid"]) is int and launched["pid"] > 0
    # The human projection and machine handoff carry the same coordinates.
    if os.name == "nt":
        assert "JSON argument array; launch without a shell" in printed.err
        assert "      " + json.dumps(argv) in printed.err
    else:
        assert "POSIX shell command:" in printed.err
        assert "      " + shlex.join(argv) in printed.err
    assert _wait_done(drv, capsys, "hinted", state_root)["rc"] == 0
    pid_before = (state / "pid").read_bytes()
    command_before = (state / "cmd.json").read_bytes()
    assert int(pid_before) == launched["pid"]
    assert marker.read_text(encoding="utf-8") == "ran once\n"
    verify_env = dict(os.environ)
    verify_env.update(
        TMPDIR=str(verify_temp),
        TMP=str(verify_temp),
        TEMP=str(verify_temp),
        MOLT_PRESERVE_AMBIENT_TMPDIR="1",
    )
    # A fresh verifier with the default lookup cannot see the first process's
    # state. This independent negative proves the different environment is real.
    root_index = argv.index("--state-dir")
    missing_root = run_guarded_test_process(
        [*argv[:root_index], *argv[root_index + 2 :]],
        cwd=unrelated_cwd,
        env=verify_env,
        timeout=120,
    )
    assert missing_root.returncode != drv.EXIT_OK
    assert "no state" in missing_root.stderr
    assert str(verify_temp / "molt_dev_detached" / "hinted") in missing_root.stderr
    # Execute the actual returned argv unchanged, from the other environment
    # and cwd. Verification observes the original run; it must never relaunch it.
    completed = run_guarded_test_process(
        argv, cwd=unrelated_cwd, env=verify_env, timeout=120
    )
    assert completed.returncode == drv.EXIT_OK, completed.stdout + completed.stderr
    assert "DONE rc=0" in completed.stderr
    assert (state / "pid").read_bytes() == pid_before
    assert (state / "cmd.json").read_bytes() == command_before
    assert marker.read_text(encoding="utf-8") == "ran once\n"


@pytest.mark.parametrize(("isolated", "expected_flags"), [(0, []), (1, ["-I"])])
def test_detached_verify_argv_preserves_interpreter_isolation(
    tmp_path, monkeypatch, isolated, expected_flags
):
    from types import SimpleNamespace

    from tools import molt_dev_detached as detached

    install_thread_module_view(
        monkeypatch,
        "sys",
        sys,
        detached,
        flags=SimpleNamespace(isolated=isolated),
    )
    state_root = tmp_path / "state root"
    assert detached.verify_command("retained", state_root, 31) == [
        sys.executable,
        *expected_flags,
        str(SCRIPT_PATH),
        "detached-verify",
        "--name",
        "retained",
        "--state-dir",
        str(state_root),
        "--min-age-s",
        "31",
    ]


def test_detached_runner_never_forks_the_live_python_process(drv):
    """Detachment stays safe under memory-guard and free-threaded callers."""
    source = (
        Path(drv.__file__).with_name("molt_dev_detached.py").read_text(encoding="utf-8")
    )
    assert "os.fork(" not in source
    assert "start_new_session" in source


def test_detached_run_nonzero_rc_is_loud(drv, tmp_path, capsys):
    state_root = tmp_path / "detached"
    drv.cmd_detached_run(
        _dr_ns(
            drv,
            "fail-run",
            state_root,
            [sys.executable, "-c", "import sys; sys.exit(7)"],
        )
    )
    assert _wait_done(drv, capsys, "fail-run", state_root)["rc"] == 7
    vrc, verdict = _verify_json(drv, capsys, _dv_ns(drv, "fail-run", state_root))
    assert vrc == drv.EXIT_FAIL
    assert verdict["status"] == "done" and verdict["rc"] == 7


def test_detached_daemon_survives_spawner_and_runs_in_new_session(
    drv, tmp_path, capsys
):
    state_root = tmp_path / "detached"
    rc = drv.cmd_detached_run(
        _dr_ns(
            drv,
            "sleeper",
            state_root,
            [sys.executable, "-c", "import time; time.sleep(2); print('woke')"],
        )
    )
    assert rc == drv.EXIT_OK  # spawner returned while the daemon still runs
    state = state_root / "sleeper"
    assert not (state / "rc").exists()  # still running -> detachment is real
    sid_text = (state / "sid").read_text(encoding="utf-8").strip()
    if os.name == "nt":
        assert sid_text.startswith("windows-process-group:")
    else:
        # New session: daemon sid differs from the test process's sid.
        assert int(sid_text) != os.getsid(0)
    vrc, verdict = _verify_json(
        drv, capsys, _dv_ns(drv, "sleeper", state_root, min_age_s=0.0)
    )
    assert vrc == drv.EXIT_OK and verdict["status"] == "running"
    assert _wait_done(drv, capsys, "sleeper", state_root)["rc"] == 0
    vrc2, verdict2 = _verify_json(drv, capsys, _dv_ns(drv, "sleeper", state_root))
    assert vrc2 == drv.EXIT_OK and verdict2["status"] == "done"


def test_detached_run_refuses_live_duplicate_and_never_kills(drv, tmp_path, capsys):
    state_root = tmp_path / "detached"
    drv.cmd_detached_run(
        _dr_ns(
            drv,
            "dup",
            state_root,
            [sys.executable, "-c", "import time; time.sleep(3)"],
        )
    )
    state = state_root / "dup"
    live_pid = int((state / "pid").read_text(encoding="utf-8").strip())
    # A second spawn under the same name must REFUSE (never kill), even
    # with --replace (replace only clears DEAD state).
    for replace in (False, True):
        with pytest.raises(drv.DriverError, match="RUNNING"):
            drv.cmd_detached_run(
                _dr_ns(
                    drv,
                    "dup",
                    state_root,
                    [sys.executable, "-c", "print('imposter')"],
                    replace=replace,
                )
            )
    # The original daemon was not harmed by the refusals.
    assert drv.probe_pid(live_pid)["alive"]
    assert _wait_done(drv, capsys, "dup", state_root)["rc"] == 0
    # After completion: same name still refuses WITHOUT --replace...
    with pytest.raises(drv.DriverError, match="--replace"):
        drv.cmd_detached_run(
            _dr_ns(drv, "dup", state_root, [sys.executable, "-c", "print('x')"])
        )
    # ...and respawns cleanly WITH --replace.
    rc = drv.cmd_detached_run(
        _dr_ns(
            drv,
            "dup",
            state_root,
            [sys.executable, "-c", "print('second')"],
            replace=True,
        )
    )
    assert rc == drv.EXIT_OK
    assert _wait_done(drv, capsys, "dup", state_root)["rc"] == 0
    assert "second" in (state / "run.log").read_text(encoding="utf-8")


def test_detached_verify_detects_died_silent(drv, tmp_path, capsys):
    # Fabricate the hazard-11 signature: a pid that is GONE and no rc file.
    dead_pid = 999_999_999
    state = tmp_path / "detached" / "ghost"
    state.mkdir(parents=True)
    (state / "pid").write_text(str(dead_pid), encoding="utf-8")
    (state / "run.log").write_text("", encoding="utf-8")
    vrc, verdict = _verify_json(
        drv, capsys, _dv_ns(drv, "ghost", tmp_path / "detached")
    )
    assert vrc == drv.EXIT_FAIL
    assert verdict["status"] == "died-silent"


def test_detached_verify_too_young_is_not_trusted(drv, tmp_path, capsys):
    state_root = tmp_path / "detached"
    drv.cmd_detached_run(
        _dr_ns(
            drv,
            "young",
            state_root,
            [sys.executable, "-c", "import time; time.sleep(3)"],
        )
    )
    vrc, verdict = _verify_json(
        drv, capsys, _dv_ns(drv, "young", state_root, min_age_s=9999.0)
    )
    assert vrc == drv.EXIT_FAIL
    assert verdict["status"] == "too-young"
    assert _wait_done(drv, capsys, "young", state_root)["rc"] == 0


def test_detached_run_missing_command_is_usage_error(drv, tmp_path):
    with pytest.raises(drv.DriverError) as exc_info:
        drv.cmd_detached_run(_dr_ns(drv, "noop", tmp_path / "detached", []))
    assert exc_info.value.code == drv.EXIT_USAGE


def test_detached_run_exec_failure_records_sentinel_rc(drv, tmp_path, capsys):
    state_root = tmp_path / "detached"
    drv.cmd_detached_run(
        _dr_ns(
            drv,
            "noexec",
            state_root,
            ["/nonexistent/binary/definitely-not-here"],
        )
    )
    state = state_root / "noexec"
    assert _wait_done(drv, capsys, "noexec", state_root)["rc"] == 127
    assert "exec failed" in (state / "run.log").read_text(encoding="utf-8")
    vrc, verdict = _verify_json(drv, capsys, _dv_ns(drv, "noexec", state_root))
    assert vrc == drv.EXIT_FAIL
    assert verdict["status"] == "done" and verdict["rc"] == 127


# --------------------------------------------------------------------------
# Hazard 12: split-root toolchain (difftest)
# --------------------------------------------------------------------------


def test_difftest_toolchain_env_roots_frontend_and_backend_together(drv, tmp_path):
    """The hazard-12 core: ONE --root drives BOTH the frontend import path
    (PYTHONPATH) and the runtime/backend build root (MOLT_PROJECT_ROOT), so a
    worktree edit can never be silently compiled out of one but not the other.
    """
    root = tmp_path / "wt"
    env = drv._difftest_toolchain_env(root, "sess-x", None)
    assert env["MOLT_PROJECT_ROOT"] == str(root)
    assert env["PYTHONPATH"].split(os.pathsep)[0] == str(root / "src")
    assert env["MOLT_SESSION_ID"] == "sess-x"


def test_difftest_toolchain_env_preserves_existing_pythonpath(
    drv, tmp_path, monkeypatch
):
    monkeypatch.setenv("PYTHONPATH", "/pre/existing")
    env = drv._difftest_toolchain_env(tmp_path, "s", None)
    parts = env["PYTHONPATH"].split(os.pathsep)
    assert parts[0] == str(tmp_path / "src")
    assert "/pre/existing" in parts  # prepended, not clobbered


def test_difftest_toolchain_env_passthrough_and_bad_kv(drv, tmp_path):
    env = drv._difftest_toolchain_env(tmp_path, "s", ["MOLT_TRACE_X=1"])
    assert env["MOLT_TRACE_X"] == "1"
    with pytest.raises(drv.DriverError) as exc:
        drv._difftest_toolchain_env(tmp_path, "s", ["NOEQUALS"])
    assert exc.value.code == drv.EXIT_USAGE


def _difftest_ns(drv, program, root):
    return drv.build_parser().parse_args(
        ["difftest", str(program), "--root", str(root)]
    )


def test_difftest_refuses_non_molt_root(drv, tmp_path):
    """A --root without the Molt CLI package is a LOUD usage refusal, never a
    build that silently uses the canonical checkout's runtime."""
    prog = tmp_path / "p.py"
    prog.write_text("print('hi')\n", encoding="utf-8")
    not_a_checkout = tmp_path / "empty"
    not_a_checkout.mkdir()
    with pytest.raises(drv.DriverError) as exc:
        drv.cmd_difftest(_difftest_ns(drv, prog, not_a_checkout))
    assert exc.value.code == drv.EXIT_USAGE
    assert "not a molt checkout" in str(exc.value)


def test_difftest_refuses_missing_program(drv):
    """A missing program file refuses before any build (root is the real repo
    so the root check passes and the program check fires)."""
    with pytest.raises(drv.DriverError) as exc:
        drv.cmd_difftest(_difftest_ns(drv, REPO_ROOT / "does-not-exist.py", REPO_ROOT))
    assert exc.value.code == drv.EXIT_USAGE
    assert "not found" in str(exc.value)


def test_difftest_streams_match_normalizes_windows_pipe_crlf(drv, monkeypatch):
    install_module_os_view(monkeypatch, drv, name="nt")
    assert drv._difftest_streams_match(b"ok\r\n", b"ok\n")


def test_difftest_streams_match_stays_byte_strict_on_posix(drv, monkeypatch):
    install_module_os_view(monkeypatch, drv, name="posix")
    assert not drv._difftest_streams_match(b"ok\r\n", b"ok\n")


def test_difftest_roots_relative_output_dir_before_safe_run(drv, tmp_path, monkeypatch):
    """A relative --out-dir is part of the rooted toolchain, not a process-cwd
    accident.

    On Windows, handing safe_run a relative extensionless native binary path can
    fail at spawn time even though the build wrote the artifact successfully.
    The difftest runner must pass an absolute, --root-scoped artifact path to
    both build verification and safe_run.
    """
    root = tmp_path / "wt"
    (root / "src" / "molt" / "cli").mkdir(parents=True)
    (root / "tools").mkdir()
    (root / "src" / "molt" / "cli" / "__init__.py").write_text("", encoding="utf-8")
    (root / "tools" / "safe_run.py").write_text("", encoding="utf-8")
    program = root / "case.py"
    program.write_text("print('ok')\n", encoding="utf-8")

    monkeypatch.setattr(
        drv.python_interpreter,
        "probe_python_command",
        lambda command, **_kwargs: drv.python_interpreter.PythonInterpreter(
            tuple(command), command[0], "3.12.0", "CPython"
        ),
    )

    class Probe:
        returncode = 0
        stderr = ""

    monkeypatch.setattr(drv, "_run_driver_command", lambda *a, **kw: Probe())

    captured: list[list[str]] = []
    oracle_stdout = b"ok\r\n" if drv.os.name == "nt" else b"ok\n"

    def fake_capture(cmd, *, env, cwd, timeout):
        captured.append(list(cmd))
        if len(cmd) >= 2 and cmd[1] == str(program):
            return 0, oracle_stdout, b""
        if cmd[1:3] == ["-m", "molt"]:
            output = Path(cmd[cmd.index("--output") + 1])
            assert output.is_absolute()
            assert output.parent == root / "tmp" / "difftest-rel"
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(b"MZ")
            return 0, b"", b""
        if cmd[1:2] == [str(root / "tools" / "safe_run.py")]:
            artifact = Path(cmd[cmd.index("--") + 1])
            assert artifact.is_absolute()
            assert artifact.parent == root / "tmp" / "difftest-rel"
            assert artifact.exists()
            return 0, b"ok\n", b""
        return 0, b"", b""

    monkeypatch.setattr(drv, "_difftest_capture", fake_capture)

    ns = drv.build_parser().parse_args(
        [
            "difftest",
            str(program),
            "--root",
            str(root),
            "--target",
            "native",
            "--python-version",
            "3.12",
            "--out-dir",
            "tmp/difftest-rel",
        ]
    )
    assert drv.cmd_difftest(ns) == drv.EXIT_OK

    run_cmds = [cmd for cmd in captured if str(root / "tools" / "safe_run.py") in cmd]
    assert len(run_cmds) == 1


def _commit_integration_policy(work: Path, commands: list[str]) -> str:
    return _commit_file(
        work,
        "tools/proof_plan.toml",
        "always = " + json.dumps(commands) + "\n",
        "policy",
    )


def _integration_python(source: str) -> str:
    return shlex.join([sys.executable, "-c", source])


@pytest.mark.parametrize("remote", ["origin", "publication"])
@pytest.mark.parametrize("attributed", [False, True], ids=["clean", "rejected"])
def test_integrate_binds_the_fetched_upstream_for_every_change_gate(
    drv, origin_and_clone, remote, attributed, capfd, borrowed_gate_python
):
    """Real gates consume one full SHA despite unrelated and moving refs."""
    origin, work = origin_and_clone
    if remote != "origin":
        publication = origin.with_name("publication.git")
        shutil.copytree(origin, publication)
        _git(work, "remote", "add", remote, str(publication))
    expected = _git(work, "rev-parse", "origin/main").stdout.strip()
    message = "changed human Rust source"
    if attributed:
        message += "\n\nCo-Authored-By: Claude <noreply@anthropic.com>"
    feature = _commit_file(work, "feature.rs", "fn feature() {}\n", message)
    upstream = "{integration_upstream_commit}"
    # Always gates run concurrently, so each gate moves the selected tracking
    # ref itself after fetch. A ref spelling passed instead of the captured
    # commit would silently select no changes. The two gates update one ref at
    # once: core.filesRefLockTimeout makes Git wait for the other's ref lock
    # instead of failing with "cannot lock ref" (exit 128).
    imports = (
        "import sys, subprocess; from pathlib import Path; "
        f"sys.path[:0] = [{str(REPO_ROOT)!r}, {str(REPO_ROOT / 'src')!r}]; "
        "subprocess.run(['git', '-c', 'core.filesRefLockTimeout=60000', "
        f"'update-ref', 'refs/remotes/{remote}/main', 'HEAD'], check=True); "
    )
    selection_gate = (
        _integration_python(
            imports
            + "from tools import check_changed_clippy as clippy, check_rustfmt as rustfmt; "
            + "clippy.ROOT = rustfmt.ROOT = Path.cwd(); "
            + "assert clippy.changed_paths('origin/main') == (); "
            + "assert rustfmt._changed_rust_paths('origin/main') == (); "
            + "assert set(clippy.changed_paths(sys.argv[1])) == {'feature.rs', 'tools/proof_plan.toml'}; "
            + "assert rustfmt._changed_rust_paths(sys.argv[1]) == (Path('feature.rs'),); "
            + f"assert sys.argv[1] == {expected!r}; "
            + "print('selected-upstream-path' + '-gates-passed', flush=True)"
        )
        + f" {upstream}"
    )
    attribution_gate = (
        _integration_python(
            imports
            + "from tools import check_commit_attribution as attribution; "
            + "assert attribution.check_revisions(['origin/main..HEAD'], cwd=Path.cwd()) == []; "
            # This fixture proves both consumers. Wait for the scheduler's
            # closed, successful selection outcome before attribution may fail.
            + "import json, time\n"
            + "head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()\n"
            + "receipt = Path('.git/molt-integrate/receipts') / (head + '.gates.json')\n"
            + "while True:\n"
            + "    rows = json.loads(receipt.read_text(encoding='utf-8'))['commands']\n"
            + "    if any(r['id'] == 'local.always.1' and r['status'] == 'success' for r in rows): break\n"
            + "    time.sleep(0.01)\n"
            + "raise SystemExit(attribution.main(['--range', sys.argv[1]]))"
        )
        + f" {upstream}..HEAD"
    )
    selected = _commit_integration_policy(work, [selection_gate, attribution_gate])
    if remote != "origin":
        # The source-sync remote may already contain this exact checkout.
        # Only the explicitly selected publication remote defines new changes.
        _git(origin, "fetch", "-q", str(work), "HEAD:refs/heads/main")
        _git(work, "fetch", "-q", "origin", "main:refs/remotes/origin/main")
    ns = _integrate_ns(
        drv, repo=str(work), remote=remote, expected_upstream=expected, no_push=True
    )
    if attributed:
        with pytest.raises(drv.DriverError, match=r"gate failed \(exit 1\)"):
            drv.cmd_integrate(ns)
    else:
        assert drv.cmd_integrate(ns) == drv.EXIT_OK
    output = capfd.readouterr()
    assert kept_gate_stdout(output.out)[0] == "selected-upstream-path-gates-passed\n"
    if attributed:
        assert feature[:12] in output.err
    assert _git(work, "rev-parse", f"{remote}/main").stdout.strip() == selected
    assert _git(work, "rev-parse", "HEAD").stdout.strip() == selected


def test_integrate_rejects_unknown_gate_binding_before_admission_or_spawn(
    drv, origin_and_clone, monkeypatch
):
    _, work = origin_and_clone
    _commit_integration_policy(work, ["echo {integration_upstream_ref}"])
    monkeypatch.setattr(
        GateConfig,
        "prepare",
        lambda *args, **kwargs: pytest.fail("invalid policy admitted"),
    )
    monkeypatch.setattr(
        drv,
        "run_captured_gate",
        _captured_gate(lambda *args: pytest.fail("invalid gate ran")),
    )
    with pytest.raises(drv.DriverError, match="unknown local gate placeholder"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))


@pytest.mark.parametrize("kind", ["tracked", "staged", "untracked", "policy"])
def test_integrate_refuses_dirty_selected_source(drv, origin_and_clone, kind):
    _, work = origin_and_clone
    _commit_file(work, "feature.py", "value = 1\n", "feature")
    original = _git(work, "rev-parse", "origin/main").stdout.strip()
    path = (
        work
        / {
            "tracked": "feature.py",
            "staged": "feature.py",
            "untracked": "injected.py",
            "policy": "tools/proof_plan.toml",
        }[kind]
    )
    path.write_text(
        "always = []\n" if kind == "policy" else "uncommitted\n", encoding="utf-8"
    )
    if kind == "staged":
        _git(work, "add", path.name)
    _git(work, "config", "status.showUntrackedFiles", "no")
    with pytest.raises(drv.DriverError, match="clean selected source"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work)))
    assert path.read_text(encoding="utf-8").startswith(
        "always" if kind == "policy" else "uncommitted"
    )
    assert (
        _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
        == original
    )


@pytest.mark.skipif(os.name == "nt", reason="POSIX shell quoting for adversarial gate")
@pytest.mark.parametrize("mutation", ["source", "head", "policy"])
def test_integrate_refuses_gate_source_or_head_drift(
    drv, origin_and_clone, mutation, borrowed_gate_python
):
    _, work = origin_and_clone
    if mutation == "head":
        command = "git -c user.name=Test -c user.email=test@example.test commit --allow-empty -qm changed"
    else:
        path = "README.md" if mutation == "source" else "tools/proof_plan.toml"
        command = _integration_python(
            f"from pathlib import Path; Path({path!r}).write_text('changed')"
        )
    _commit_integration_policy(work, [command])
    original = _git(work, "rev-parse", "origin/main").stdout.strip()
    with pytest.raises(drv.DriverError, match="HEAD changed|clean selected source"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work)))
    assert (
        _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
        == original
    )


@pytest.mark.skipif(os.name == "nt", reason="POSIX shell quoting for adversarial gate")
def test_integrate_block_refuses_a_gate_that_dirties_the_source(
    drv, origin_and_clone, monkeypatch, borrowed_gate_python
):
    _, work = origin_and_clone
    dirty = _integration_python(
        "from pathlib import Path; Path('README.md').write_text('changed')"
    )
    _commit_integration_policy(work, [dirty, "echo clean"])
    monkeypatch.setattr(drv, "concurrent_gate_limit", lambda: 2)
    original = _git(work, "rev-parse", "origin/main").stdout.strip()
    with pytest.raises(drv.DriverError, match="clean selected source") as exc:
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work)))
    # A dirtied source has no single culprit; the queue must not bisect on it.
    assert not isinstance(exc.value, CandidateFailure)
    assert (
        _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
        == original
    )


def test_integrate_uses_original_policy_despite_replace_ref(drv, origin_and_clone):
    _, work = origin_and_clone
    failing = _commit_integration_policy(work, ["exit 19"])
    passing = _commit_integration_policy(work, ["echo replacement-must-not-run"])
    _git(work, "checkout", "--detach", failing)
    _git(work, "replace", failing, passing)
    with pytest.raises(drv.DriverError, match="gate failed.*19"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work)))
    # The replacement ref remains operator-owned; admission does not delete it.
    assert _git(work, "replace", "-l").stdout.strip() == failing


@pytest.mark.skipif(os.name == "nt", reason="executable POSIX Git hook")
def test_integrate_preserves_foreign_hook_and_both_rejection_streams(
    drv, origin_and_clone, capsys
):
    _, work = origin_and_clone
    _commit_file(work, "feature.py", "value = 1\n", "feature")
    hook = work / ".git/hooks/pre-push"
    content = b"#!/bin/sh\necho foreign-stdout\necho foreign-stderr >&2\nexit 1\n"
    hook.write_bytes(content)
    hook.chmod(0o755)
    original = _git(work, "rev-parse", "origin/main").stdout.strip()
    with pytest.raises(drv.DriverError, match="PUSH NOT CONFIRMED"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work)))
    captured = capsys.readouterr()
    output = captured.out + captured.err
    assert "foreign-stdout" in output and "foreign-stderr" in output
    assert hook.read_bytes() == content and hook.stat().st_mode & 0o111
    assert (
        _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
        == original
    )


def test_integrate_claim_refuses_advanced_upstream_without_rebase(
    drv, origin_and_clone
):
    origin, work = origin_and_clone
    checked = _git(work, "rev-parse", "origin/main").stdout.strip()
    local = _commit_file(work, "claim.txt", "mine\n", "my claim")
    other = work.parent / "competing"
    _git(work.parent, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "other@example.test")
    _git(other, "config", "user.name", "Other")
    winner = _commit_file(other, "claim.txt", "theirs\n", "competing claim")
    _git(other, "push", "origin", "main")
    with pytest.raises(drv.DriverError, match="expected upstream changed"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), expected_upstream=checked))
    assert _git(work, "rev-parse", "HEAD").stdout.strip() == local
    assert (work / "claim.txt").read_text(encoding="utf-8") == "mine\n"
    assert (
        _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0] == winner
    )


def test_integrate_has_no_gate_bypass_cli(drv):
    for flag in ("--no-gates", "--gates-config"):
        with pytest.raises(SystemExit) as exc:
            drv.build_parser().parse_args(["integrate", flag])
        assert exc.value.code == 2


@pytest.mark.skipif(os.name == "nt", reason="executable POSIX Git hook")
def test_integrate_pushes_captured_commit_when_foreign_hook_moves_head(
    drv, origin_and_clone
):
    _, work = origin_and_clone
    selected = _commit_file(work, "feature.py", "value = 1\n", "feature")
    hook = work / ".git/hooks/pre-push"
    hook.write_text(
        "#!/bin/sh\ngit -c user.name=Test -c user.email=test@example.test commit --allow-empty -qm hook-head\n",
        encoding="utf-8",
    )
    hook.chmod(0o755)
    assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work))) == drv.EXIT_OK
    assert _git(work, "rev-parse", "HEAD").stdout.strip() != selected
    assert (
        _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
        == selected
    )


@pytest.mark.skipif(os.name == "nt", reason="POSIX shell quoting for adversarial gate")
def test_integrate_remote_race_after_gates_is_not_rebased_or_force_pushed(
    drv, origin_and_clone
):
    origin, work = origin_and_clone
    other = work.parent / "racing"
    _git(work.parent, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "racer@example.test")
    _git(other, "config", "user.name", "Racer")
    winner = _commit_file(other, "winner.txt", "winner\n", "winner")
    policy = shlex.join(["git", "-C", str(other), "push", "origin", "main"])
    selected = _commit_integration_policy(work, [policy])
    with pytest.raises(drv.DriverError, match="PUSH NOT CONFIRMED"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work)))
    assert _git(work, "rev-parse", "HEAD").stdout.strip() == selected
    assert (
        _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0] == winner
    )


def test_integrate_clean_rebase_runs_committed_policy(
    drv, origin_and_clone, monkeypatch, capsys
):
    origin, work = origin_and_clone
    source = _commit_file(work, "feature.py", "value = 1\n", "local")
    _git(work, "commit", "--allow-empty", "-qm", "intentional empty replay")
    empty = _git(work, "rev-parse", "HEAD").stdout.strip()
    other = work.parent / "upstream"
    _git(work.parent, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "upstream@example.test")
    _git(other, "config", "user.name", "Upstream")
    upstream = _commit_file(other, "other.py", "other = 1\n", "upstream")
    _git(other, "push", "origin", "main")
    commands = _record_driver_git_commands(drv, monkeypatch)
    deadlines = _record_process_deadlines(monkeypatch)
    assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work))) == drv.EXIT_OK
    selected = _git(work, "rev-parse", "HEAD").stdout.strip()
    assert selected != source
    # A landing's Git plumbing waits out a host stall: a deadline on a 5 ms
    # read failed this test when the host stalled (2026-10-21 landing batch).
    git_deadlines = [deadline for argv, deadline in deadlines if argv[0] == "git"]
    assert len(git_deadlines) >= len(commands) > 0
    assert set(git_deadlines) == {None}
    replay = next(index for index, args in enumerate(commands) if args[0] == "rebase")
    assert ["diff-tree", "-p", "--no-color", source] in commands[:replay]
    assert ["diff-tree", "-p", "--no-color", selected] in commands[replay + 1 :]
    assert any(args[0] == "patch-id" for args in commands[:replay])
    assert any(args[0] == "patch-id" for args in commands[replay + 1 :])
    assert drv.Git(work).is_ancestor(upstream, selected)
    assert (
        _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
        == selected
    )
    assert (work / "feature.py").read_text(encoding="utf-8") == "value = 1\n"
    assert (
        _git(work, "log", "-1", "--format=%s").stdout.strip()
        == "intentional empty replay"
    )
    assert (
        _git(work, "rev-parse", "HEAD^{tree}").stdout
        == _git(work, "rev-parse", "HEAD~1^{tree}").stdout
    )
    output = capsys.readouterr()
    assert "empty diffs (no patch-id, not patch-tracked)" in output.out + output.err
    assert empty[:9] in output.out + output.err


def test_landing_local_git_reads_carry_no_wall_clock_deadline(
    drv, origin_and_clone, monkeypatch
):
    from tools import molt_dev_controller

    _, work = origin_and_clone
    deadlines = _record_process_deadlines(monkeypatch)
    head = drv.Git(work).rev_parse("HEAD")
    assert drv._git_common_directory(work) is not None
    assert molt_dev_controller._git_bytes(work, "rev-parse", "HEAD").strip() == (
        head.encode("ascii")
    )
    reads = [(argv, deadline) for argv, deadline in deadlines if argv[0] == "git"]
    assert {argv[-1] for argv, _ in reads} >= {"HEAD", "--git-common-dir"}
    assert len(reads) >= 3
    assert {deadline for _, deadline in reads} == {None}


class _SilentPeer:
    """A local TCP peer that accepts and never answers: a wedged remote the
    kernel keeps alive, which TCP keepalive never ends."""

    def __init__(self) -> None:
        import socket
        import threading

        self.server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.server.bind(("127.0.0.1", 0))
        self.server.listen(4)
        self.port = self.server.getsockname()[1]
        self.held: list[socket.socket] = []
        threading.Thread(target=self._serve, daemon=True).start()

    def _serve(self) -> None:
        while True:
            try:
                conn, _ = self.server.accept()
            except OSError:
                return
            self.held.append(conn)
            try:
                conn.recv(65536)
            except OSError:
                pass

    def close(self) -> None:
        self.server.close()
        for conn in self.held:
            conn.close()


class _Unauthorized:
    """A local HTTP remote that asks every request for credentials."""

    def __init__(self) -> None:
        import http.server
        import threading

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:  # noqa: N802 - the stdlib's name
                self.send_response(401)
                self.send_header("WWW-Authenticate", 'Basic realm="molt"')
                self.send_header("Content-Length", "0")
                self.end_headers()

            def log_message(self, *_args) -> None:
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_port}/private.git"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()


def _transfer_in_thread(drv, work, args):
    """Run a driver transfer on a thread; a hang guard only bounds the test."""
    import threading

    outcome: dict[str, object] = {}
    transfer = threading.Thread(
        target=lambda: outcome.update(proc=drv.Git(work)._run(args, check=False))
    )
    transfer.start()
    transfer.join(120)  # A hang guard only; the bounds end it within seconds.
    assert not transfer.is_alive(), "the transfer outlived its progress bound"
    return outcome["proc"]


@pytest.fixture
def isolated_git_identity(monkeypatch, tmp_path):
    """Keep the developer's own Git configuration out of transfer tests."""
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(tmp_path / "gitconfig"))
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
    (tmp_path / "gitconfig").write_text("", encoding="utf-8")
    return tmp_path / "gitconfig"


@pytest.mark.parametrize("scheme", ["http", "ssh"])
def test_git_transfer_ends_a_silent_peer_by_progress_not_wall_clock(
    drv, origin_and_clone, monkeypatch, isolated_git_identity, scheme
):
    """A peer that never answers ends by lack of progress, with no deadline.

    HTTP: curl's low-speed limit. ssh: ConnectTimeout, which covers the
    banner exchange; server-alive probes start only after it. The transfer
    is attended here, so only the transport's own bound can end it.
    """
    from tools import git_transfer

    if scheme == "ssh" and shutil.which("ssh") is None:
        pytest.skip("needs an OpenSSH client")
    _, work = origin_and_clone
    peer = _SilentPeer()
    monkeypatch.setattr(git_transfer, "TRANSFER_STALL_SECONDS", 2)
    monkeypatch.setattr(git_transfer, "prompt_reaches_a_person", lambda: True)
    deadlines = _record_process_deadlines(monkeypatch)
    url = (
        f"http://127.0.0.1:{peer.port}/silent.git"
        if scheme == "http"
        else f"ssh://git@127.0.0.1:{peer.port}/silent.git"
    )
    try:
        proc = _transfer_in_thread(drv, work, ["ls-remote", url])
    finally:
        peer.close()
    assert proc.returncode != 0
    if scheme == "http":
        assert "too slow" in proc.stderr.lower()
    else:
        assert "timed out during banner exchange" in proc.stderr.lower()
    transfer_deadlines = [deadline for argv, deadline in deadlines if url in argv]
    assert transfer_deadlines and set(transfer_deadlines) == {None}


def test_unattended_git_transfer_never_runs_an_inherited_askpass(
    drv, origin_and_clone, monkeypatch, isolated_git_identity, tmp_path
):
    from tools import git_transfer

    _, work = origin_and_clone
    asked = tmp_path / "asked"
    askpass = tmp_path / "askpass.sh"
    # Stands in for a desktop prompt nobody answers.
    askpass.write_text(
        f'#!/bin/sh\necho "$1" >> {shlex.quote(str(asked))}\nexec sleep 3600\n',
        encoding="utf-8",
    )
    askpass.chmod(0o755)
    monkeypatch.setenv("GIT_ASKPASS", str(askpass))
    monkeypatch.setenv("SSH_ASKPASS", str(askpass))
    monkeypatch.setattr(git_transfer, "prompt_reaches_a_person", lambda: False)
    remote = _Unauthorized()
    try:
        proc = _transfer_in_thread(drv, work, ["ls-remote", remote.url])
    finally:
        remote.close()
    assert proc.returncode not in (0, 124)
    assert not asked.exists(), asked.read_text(encoding="utf-8")


def test_unattended_git_transfer_ends_a_credential_helper_that_waits(
    drv, origin_and_clone, monkeypatch, isolated_git_identity, tmp_path
):
    """A helper that waits (a keychain prompt nobody sees) ends on silence."""
    from tools import git_transfer

    _, work = origin_and_clone
    marker = hung_child_marker()
    helper = tmp_path / "helper.py"
    helper.write_text(HUNG_CHILD_SOURCE, encoding="utf-8")
    isolated_git_identity.write_text(
        "[credential]\n\thelper = "
        f"!{shlex.quote(sys.executable)} {shlex.quote(str(helper))} {marker}\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(git_transfer, "TRANSFER_STALL_SECONDS", 2)
    monkeypatch.setattr(git_transfer, "prompt_reaches_a_person", lambda: False)
    remote = _Unauthorized()
    try:
        proc = _transfer_in_thread(drv, work, ["ls-remote", remote.url])
    finally:
        remote.close()
        stop_test_processes_carrying(marker)
    assert proc.returncode == 124
    assert "no output for 2.00s" in proc.stderr
    assert not processes_carrying(marker)


def test_integrate_classifies_raw_paths_without_git_quote_escaping(
    drv, origin_and_clone
):
    _, work = origin_and_clone
    _commit_file(
        work,
        "tools/proof_plan.toml",
        'always = ["echo required"]\n[[rule]]\nname = "source"\nglobs = ["src/**"]\ngates = ["exit 23"]\n',
        "path policy",
    )
    _commit_file(work, 'src/a"b.py', "pass\n", "quoted source path")
    with pytest.raises(drv.DriverError, match="gate failed.*23"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work)))


def test_integrate_rename_runs_both_source_and_destination_rules(drv, origin_and_clone):
    _, work = origin_and_clone
    # Seed the source path upstream so this is a genuine cross-rule rename,
    # not an added file whose destination alone is sufficient.
    _commit_file(work, "native/owner.rs", "same body\n", "upstream owner")
    _git(work, "push", "origin", "main")
    _commit_file(
        work,
        "tools/proof_plan.toml",
        'always = ["echo always"]\n'
        '[[rule]]\nname = "destination"\nglobs = ["docs/**"]\ngates = ["echo destination-proof"]\n'
        '[[rule]]\nname = "source"\nglobs = ["native/**"]\ngates = ["exit 29"]\n',
        "rename policy",
    )
    (work / "docs").mkdir()
    _git(work, "mv", "native/owner.rs", "docs/owner.rs")
    _git(work, "commit", "-qm", "move across proof classes")
    assert _git(work, "diff", "--name-status", "origin/main..HEAD").stdout.startswith(
        "R100"
    )
    with pytest.raises(drv.DriverError, match="gate failed.*29"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work)))
    assert _git(work, "show", "origin/main:native/owner.rs").stdout == "same body\n"


def test_integrate_cli_starts_without_ambient_pythonpath(tmp_path):
    env = os.environ.copy()
    env.pop("PYTHONPATH", None)
    # Ignore guard PYTHONPATH and site-startup hooks that can preload another
    # checkout. Retain third-party dependencies without executing .pth files.
    bootstrap = (
        "import runpy, sys; "
        f"sys.path[:0] = {[str(TOOLS_ROOT), sysconfig.get_path('purelib'), sysconfig.get_path('platlib')]!r}; "
        f"sys.argv = {[str(SCRIPT_PATH), 'integrate', '--help']!r}; "
        f"runpy.run_path({str(SCRIPT_PATH)!r}, run_name='__main__')"
    )
    result = _run_fast_captured_command(
        [sys.executable, "-E", "-S", "-c", bootstrap],
        cwd=tmp_path,
        env=ProcessEnvironment(env),
    )
    assert result.returncode == 0, result.stderr
    assert "--expected-upstream" in result.stdout


def test_integrate_admission_aggregates_failures_before_any_gate(
    drv, origin_and_clone, monkeypatch
):
    from tools import proof_plan

    _origin, work = origin_and_clone
    policy = """
always = ["echo must-not-run"]
always_toolchains = ["one", "two"]
[[toolchain_policy]]
name = "one"
executable = "one"
version_pattern = '^ok$'
[[toolchain_policy]]
name = "two"
executable = "two"
version_pattern = '^ok$'
"""
    _commit_file(work, "tools/proof_plan.toml", policy, "selected prerequisites")

    def probe(policy, *, environment, python_executable):
        if policy.name == "one":
            raise OSError("first probe raised")
        return None

    monkeypatch.setattr(proof_plan, "_version_fingerprint", probe)
    monkeypatch.setattr(
        drv,
        "run_captured_gate",
        _captured_gate(lambda *args: pytest.fail("gate ran before admission")),
    )
    with pytest.raises(drv.DriverError) as caught:
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))
    message = str(caught.value)
    for expected in ("one", "two", "OSError", "local.always.0", "executed=0"):
        assert expected in message


def _capacity_integration_policy(drv, work, monkeypatch):
    _commit_file(
        work,
        "tools/proof_plan.toml",
        """
always = ["echo non-cargo-prefix"]
[[rule]]
name = "cargo-capability"
globs = ["**"]
gates = ["echo opaque-cargo-wrapper"]
toolchains = ["cargo"]
[[toolchain_policy]]
name = "cargo"
executable = "cargo"
version_pattern = '.*'
""",
        "selected Cargo capability",
    )
    # Tool installation/version projection is outside the capacity contract.
    # Selection, explicit capability, output resolution, and admission stay real.
    monkeypatch.setattr(
        GateConfig,
        "prepare",
        lambda self, gates, *, baseline, prepare_python: PreparedGates(
            {
                gate.toolchains: GateEnvironment(
                    ProcessEnvironment(baseline), None, False
                )
                for gate in gates
            },
            {},
        ),
    )


def _capacity_measurement(monkeypatch, measure):
    from molt import disk_capacity

    # The measurement is the independent oracle on every host, including
    # Darwin's important-usage estimate when observed raw free is too small.
    monkeypatch.setattr(disk_capacity, "_default_measure_free_bytes", measure)
    monkeypatch.setattr(disk_capacity, "_darwin_important_usage_bytes", measure)


def test_integrate_selected_output_capacity_refuses_before_gate_prefix(
    drv, origin_and_clone, monkeypatch, tmp_path
):
    _origin, work = origin_and_clone
    _capacity_integration_policy(drv, work, monkeypatch)
    target = tmp_path / "selected-target"
    build = tmp_path / "selected-build"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target))
    monkeypatch.setenv("CARGO_BUILD_BUILD_DIR", str(build))
    monkeypatch.delenv("MOLT_DISK_GUARD_HIGH_WATER_GB", raising=False)
    _capacity_measurement(monkeypatch, lambda path: 25 * 1024**3 - 1)
    monkeypatch.setattr(
        drv,
        "run_captured_gate",
        _captured_gate(lambda *args: pytest.fail("expensive prefix started")),
    )
    with pytest.raises(drv.DriverError, match="capacity failed; executed=0") as caught:
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))
    assert str(target) in str(caught.value)
    assert str(build) in str(caught.value)
    assert "26843545600" in str(caught.value)
    assert not target.exists() and not build.exists()


@pytest.mark.parametrize("gate_rc", [0, 19])
def test_integrate_capacity_observes_interval_without_campaign_guarantee(
    drv, origin_and_clone, monkeypatch, tmp_path, capsys, gate_rc
):
    _origin, work = origin_and_clone
    _capacity_integration_policy(drv, work, monkeypatch)
    target = tmp_path / "selected-target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target))
    monkeypatch.delenv("CARGO_BUILD_BUILD_DIR", raising=False)
    monkeypatch.delenv("MOLT_DISK_GUARD_HIGH_WATER_GB", raising=False)
    available = 25 * 1024**3 + 426_721_280
    _capacity_measurement(monkeypatch, lambda path: available)
    ran = []

    def gate(command, repo, env):
        nonlocal available
        ran.append(command)
        if command == "echo opaque-cargo-wrapper":
            available -= 1_420_288_000
            return gate_rc
        return 0

    monkeypatch.setattr(drv, "run_captured_gate", _captured_gate(gate))
    if gate_rc:
        with pytest.raises(drv.DriverError, match="gate failed \\(exit 19\\)"):
            drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))
    else:
        assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True)) == 0
    assert ran == ["echo non-cargo-prefix", "echo opaque-cargo-wrapper"]
    output = capsys.readouterr().err
    readiness = json.loads(
        next(
            line.split("selected output capacity: ", 1)[1]
            for line in output.splitlines()
            if "selected output capacity: " in line
        )
    )
    assert readiness["remaining_campaign_growth_bytes"] is None
    assert readiness["reserved_bytes"] == 0
    assert "remaining campaign growth unknown; capacity is not reserved" in output
    observations = [
        json.loads(line.split("gate capacity: ", 1)[1])
        for line in output.splitlines()
        if "gate capacity: " in line
    ]
    assert [row["phase"] for row in observations] == ["before", "after"]
    assert observations[1]["free_bytes_delta"] == {str(target): -1_420_288_000}
    assert observations[0]["probes"][0]["free_bytes"] == 27_270_266_880
    assert observations[1]["probes"][0]["free_bytes"] == 25_849_978_880
    for row in observations:
        assert row["gate"] == "echo opaque-cargo-wrapper"
        assert "not exclusive allocation" in row["scope"]
        assert row["probes"][0]["required_bytes"] == 26_843_545_600
        assert "measurement" in row["probes"][0]
        assert "available_bytes" in row["probes"][0]


@pytest.mark.parametrize("dry", [False, True])
def test_integrate_no_cargo_or_dry_run_has_no_new_capacity_requirement(
    drv, origin_and_clone, monkeypatch, dry
):
    from molt import disk_capacity

    _origin, work = origin_and_clone
    if dry:
        _capacity_integration_policy(drv, work, monkeypatch)
    else:
        _commit_file(work, "source.txt", "source\n", "non-Cargo change")

    def forbidden(*args, **kwargs):
        pytest.fail("non-build path acquired a capacity requirement")

    monkeypatch.setattr(disk_capacity, "require_build_capacity", forbidden)
    monkeypatch.setattr(disk_capacity, "probe_capacity", forbidden)
    monkeypatch.setattr(
        drv, "run_captured_gate", _captured_gate(forbidden if dry else lambda *args: 0)
    )
    assert (
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True, dry_run=dry))
        == 0
    )


@pytest.mark.parametrize("gate_rc", [0, 19])
def test_integrate_capacity_observer_failure_preserves_gate_result(
    drv, origin_and_clone, monkeypatch, capsys, gate_rc
):
    from molt import disk_capacity

    _origin, work = origin_and_clone
    _capacity_integration_policy(drv, work, monkeypatch)
    _capacity_measurement(monkeypatch, lambda path: 30 * 1024**3)
    after = False
    probe = disk_capacity.probe_capacity

    def observe(*args, **kwargs):
        if after:
            raise RuntimeError("independent observer failure")
        return probe(*args, **kwargs)

    def gate(command, repo, env):
        nonlocal after
        if command == "echo opaque-cargo-wrapper":
            after = True
            return gate_rc
        return 0

    monkeypatch.setattr(disk_capacity, "probe_capacity", observe)
    monkeypatch.setattr(drv, "run_captured_gate", _captured_gate(gate))
    if gate_rc:
        with pytest.raises(drv.DriverError, match="gate failed \\(exit 19\\)"):
            drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))
    else:
        assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True)) == 0
    output = capsys.readouterr()
    assert "independent observer failure" in output.out + output.err


@pytest.mark.skipif(os.name == "nt", reason="POSIX executable fixture")
def test_integrate_admission_projects_the_selected_checkout_into_real_gate(
    drv, origin_and_clone, tmp_path, monkeypatch
):
    from types import SimpleNamespace
    from molt import tool_releases

    _origin, work = origin_and_clone
    provider = tmp_path / "selected checkout tools"
    provider.mkdir()
    executable = provider / "cat"
    # Preserve both the native image and argv[0] utility selection across
    # macOS system-image restrictions and multicall coreutils implementations.
    executable.symlink_to("/bin/cat")
    observed_roots = []
    # Only the gates' baseline carries this key (gate_environment sets it), so
    # admission under the process environment would be seen.
    monkeypatch.delenv("PYTHONPYCACHEPREFIX", raising=False)

    def discover(name, root, *, environ, cwd):
        assert cwd is None
        assert name == "cat"
        assert root == work
        assert "PYTHONPYCACHEPREFIX" in environ
        observed_roots.append(root)
        return SimpleNamespace(executable=executable)

    monkeypatch.setattr(tool_releases, "discover_pinned_tool", discover)
    # Admission must not update this process; the restore only bounds a
    # regression to this test.
    monkeypatch.setenv("PATH", os.environ["PATH"])
    path_before = os.environ["PATH"]
    _commit_file(
        work, "tool-version.txt", "selected-checkout-tool\n", "owned tool version input"
    )
    gate = (
        f'test "$(command -v cat)" = {shlex.quote(str(executable))} && '
        'test "$(cat tool-version.txt)" = selected-checkout-tool'
    )
    _commit_file(
        work,
        "tools/proof_plan.toml",
        f"""
always = [{json.dumps(gate)}]
always_toolchains = ["cat"]
[[toolchain_policy]]
name = "cat"
projection = "tool-release"
executable = "cat"
version_args = ["tool-version.txt"]
version_pattern = '^selected-checkout-tool$'
""",
        "selected checkout tool projection",
    )
    assert (
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))
        == drv.EXIT_OK
    )
    # The gate itself proved the projection: it ran the selected checkout's
    # `cat`. The integrating process keeps its own PATH.
    assert observed_roots == [work, work]
    assert os.environ["PATH"] == path_before


def test_selected_local_gate_does_not_materialize_unrelated_policy(
    drv, tmp_path, monkeypatch
):
    from tools import proof_plan

    cfg = GateConfig.parse(
        """
always = []
[[toolchain_policy]]
name = "unrelated"
node_package = "missing"
package_lock = "missing/lock"
[[rule]]
name = "simple"
globs = ["src/**"]
gates = ["echo selected"]
toolchains = []
[[rule]]
name = "browser"
globs = ["wasm/**"]
gates = ["echo unrelated"]
toolchains = ["unrelated"]
""",
        "selected fixture",
        root=tmp_path,
    )
    commands, matched = cfg.select(["src/one.py"])
    commands = [gate.command for gate in commands]
    assert commands == ["echo selected"]
    monkeypatch.setattr(
        proof_plan,
        "_version_fingerprint",
        lambda _, **_kw: pytest.fail("unrelated probe ran"),
    )
    # No selected gate declares a toolchain, so nothing is fingerprinted.
    selected, _ = cfg.select(["src/one.py"])
    prepared = cfg.prepare(
        selected,
        baseline=_gate_caller_env(tmp_path / "host"),
        prepare_python=lambda env, needed: drv.prepare_gate_environment(
            tmp_path, env, requires_python=needed
        ),
    )
    assert prepared.fingerprints == {}
    with pytest.raises(drv.DriverError, match="lack declared prerequisite custody"):
        cfg.select(["src/one.py"], extra=["unknown executable"])


def test_integrate_missing_committer_refuses_before_rebase(
    drv, origin_and_clone, monkeypatch
):
    origin, work = origin_and_clone
    local = _commit_file(work, "feature.py", "value = 1\n", "local")
    other = work.parent / "upstream"
    _git(work.parent, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "upstream@example.test")
    _git(other, "config", "user.name", "Upstream")
    _commit_file(other, "other.py", "other = 1\n", "upstream")
    _git(other, "push", "origin", "main")
    _git(work, "config", "--unset", "user.name")
    _git(work, "config", "--unset", "user.email")
    _git(work, "config", "user.useConfigOnly", "true")
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(work.parent / "absent-global-config"))
    monkeypatch.delenv("GIT_COMMITTER_NAME", raising=False)
    monkeypatch.delenv("GIT_COMMITTER_EMAIL", raising=False)
    calls = []
    real_run = drv.Git._run

    def run(self, args, **kwargs):
        calls.append(args)
        return real_run(self, args, **kwargs)

    monkeypatch.setattr(drv.Git, "_run", run)
    with pytest.raises(drv.DriverError, match="configured Git committer"):
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))
    assert ["var", "GIT_COMMITTER_IDENT"] in calls
    assert not any(args and args[0] == "rebase" for args in calls)
    assert drv.Git(work).head_sha() == local


def test_selected_catalog_admission_preserves_imports_after_proof_plan_collection():
    # A sibling file may collect proof_plan before pytest puts the repository
    # root first. Command identity is still lazy at integration time, exactly
    # the lifecycle the original path/catalog isolation oracle must preserve.
    target = (
        "tests/test_molt_dev.py::"
        "test_integrate_uses_selected_committed_target_catalog_without_import_rebinding"
    )
    code = (
        "import sys\n"
        "from tools import proof_plan\n"
        "assert 'tools.proof_queue_pkg.command_identity' not in sys.modules\n"
        "import pytest\n"
        f"raise SystemExit(pytest.main(['-q', '-n', '0', {target!r}]))\n"
    )
    result = run_guarded_test_process(
        [sys.executable, "-c", code],
        cwd=REPO_ROOT,
        env=ProcessEnvironment(os.environ).updated(
            {
                "PYTHONPATH": os.pathsep.join((str(REPO_ROOT), str(REPO_ROOT / "src"))),
                "PYTEST_ADDOPTS": "",
            }
        ),
        timeout=60,
    )
    assert result.returncode == 0, result.stdout + result.stderr


def test_integrate_uses_selected_committed_target_catalog_without_import_rebinding(
    drv, origin_and_clone, monkeypatch
):
    from molt import target_python_policy
    from tools import proof_plan

    _, work = origin_and_clone
    _commit_file(work, "src/molt/__init__.py", "", "selected package")
    _commit_file(
        work,
        "src/molt/_selected_test_catalog.py",
        "VERSIONS = ('3.12', '3.15')\n",
        "selected catalog dependency",
    )
    _commit_file(
        work,
        "src/molt/target_python_policy.py",
        "from molt._selected_test_catalog import VERSIONS\nSUPPORTED_TARGET_PYTHON_SHORT_VERSIONS = VERSIONS\n",
        "selected target catalog",
    )
    _commit_file(
        work,
        "tools/proof_plan.toml",
        """
always = ["echo must-not-run"]
always_toolchains = ["target-pythons"]
[[toolchain_policy]]
name = "uv"
executable = "uv"
version_pattern = '^ok$'
[[toolchain_policy]]
name = "target-pythons"
python_versions = "supported-targets"
""",
        "selected target prerequisites",
    )
    before_path = list(sys.path)
    before_catalog = target_python_policy.SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
    observed = []

    def probe(policy, *, environment, python_executable):
        observed.append(policy.name)
        if policy.name == "uv":
            return {
                "version": "ok",
                "launcher_sha256": "a" * 64,
                "executable_sha256": "b" * 64,
            }
        return None

    monkeypatch.setattr(proof_plan, "_version_fingerprint", probe)
    monkeypatch.setattr(
        drv,
        "run_captured_gate",
        _captured_gate(
            lambda *args: pytest.fail("gate began before selected catalog admission")
        ),
    )
    with pytest.raises(drv.DriverError) as caught:
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))
    assert set(observed) == {"uv", "target-pythons.3.12", "target-pythons.3.15"}
    assert "local.always.0" in str(caught.value)
    assert "3.15" in str(caught.value) and "executed=0" in str(caught.value)
    assert sys.path == before_path
    assert sys.modules["molt.target_python_policy"] is target_python_policy
    assert target_python_policy.SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS is before_catalog


def test_gate_selection_preserves_tools_across_batching_dedup_and_extras(drv, tmp_path):
    (tmp_path / "tests").mkdir()
    for name in ("a", "b"):
        (tmp_path / "tests" / f"test_{name}.py").write_text("", encoding="utf-8")
    cfg = GateConfig.parse(
        """
always = ["shared"]
always_toolchains = ["base"]
[[rule]]
name = "first"
globs = ["src/**"]
gates = ["shared"]
pytest = ["tests/test_a.py"]
pytest_select = [{ k = "chosen", targets = ["tests/test_b.py"] }]
toolchains = ["first"]
[[rule]]
name = "second"
globs = ["src/**"]
gates = ["shared"]
pytest = ["tests/test_b.py"]
toolchains = ["second"]
[[rule]]
name = "sdk"
globs = ["src/**"]
gates = ["sdk-command"]
toolchains = ["sdk"]
[[rule]]
name = "compile-controls"
globs = ["src/**"]
gates = ["cargo test -p tiny", "python tools/check_changed_clippy.py"]
toolchains = ["rust"]
[[rule]]
name = "late-metadata"
globs = ["src/**"]
gates = ["python tools/perf_methodology.py --check", "shared"]
toolchains = ["python"]
[[rule]]
name = "unmatched"
globs = ["other/**"]
gates = ["explicit-extra"]
pytest = ["tests/test_b.py"]
toolchains = ["extra"]
""",
        "tool custody fixture",
        root=tmp_path,
    )
    gates, matched = cfg.select(["src/a.py"], extra=["explicit-extra", "shared"])
    prefix = (
        "python",
        "-m",
        "pytest",
        "-q",
        "-n",
        "auto",
        "--molt-worker-memory-gb=2.5",
    )
    assert [gate.command for gate in gates] == [
        "shared",
        "sdk-command",
        "cargo test -p tiny",
        "python tools/check_changed_clippy.py",
        "python tools/perf_methodology.py --check",
        "explicit-extra",
        (*prefix, "tests/test_a.py"),
        (*prefix, "tests/test_b.py"),
        (*prefix, "-k", "chosen", "tests/test_b.py"),
    ]
    assert len(gates) == len({gate.command for gate in gates}) == 9
    assert [rule.name for rule in matched] == [
        "first",
        "second",
        "sdk",
        "compile-controls",
        "late-metadata",
    ]
    assert next(gate.toolchains for gate in gates if gate.command == "shared") == (
        "base",
        "first",
        "second",
        "python",
    )
    assert next(
        gate.toolchains for gate in gates if gate.command == "explicit-extra"
    ) == ("extra",)
    cohorts = [
        gate
        for gate in gates
        if isinstance(gate.command, tuple) and "-k" not in gate.command
    ]
    assert [(gate.command[-1], gate.toolchains) for gate in cohorts] == [
        ("tests/test_a.py", ("first",)),
        ("tests/test_b.py", ("second",)),
    ]
    selected = next(
        gate
        for gate in gates
        if isinstance(gate.command, tuple) and "-k" in gate.command
    )
    assert selected.toolchains == ("first",)
    with pytest.raises(drv.DriverError, match="lack declared prerequisite custody"):
        cfg.select(["src/a.py"], extra=["undeclared"])


@pytest.mark.parametrize("explicit_sdk_flags", [False, True])
def test_integrate_isolates_selected_toolchains_in_real_children(
    drv,
    origin_and_clone,
    tmp_path,
    monkeypatch,
    explicit_sdk_flags,
    borrowed_gate_python,
):
    from tools import proof_plan

    _, work = origin_and_clone
    flag_key = "CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS"
    user_flags = "-C target-feature=-reference-types"
    if explicit_sdk_flags:
        monkeypatch.setenv(flag_key, user_flags)
    else:
        monkeypatch.delenv(flag_key, raising=False)
    monkeypatch.setenv("RUSTFLAGS", "-C debuginfo=1")
    outputs = [tmp_path / f"child-{index}.json" for index in range(3)]
    commands = []
    for index, output in enumerate(outputs):
        source = (
            "import json,os,sys; from pathlib import Path; "
            f"Path({str(output)!r}).write_text(json.dumps("
            f'[os.environ.get({flag_key!r}), os.environ["RUSTFLAGS"], sys.argv[1:]]), encoding="utf-8")'
        )
        commands.append(_integration_python(source) + " {integration_upstream_commit}")
    policy = f"""
always = [{json.dumps(commands[0])}]
always_toolchains = ["rust"]
[[toolchain_policy]]
name = "rust"
executable = "rust"
version_pattern = '^ok$'
[[toolchain_policy]]
name = "sdk"
executable = "sdk"
version_pattern = '^ok$'
[[rule]]
name = "sdk"
globs = ["tools/proof_plan.toml"]
gates = {json.dumps(commands[1:])}
toolchains = ["sdk"]
"""
    upstream = _git(work, "rev-parse", "origin/main").stdout.strip()
    _commit_file(work, "tools/proof_plan.toml", policy, "command scoped tools")
    calls = []

    def project(policies, names, *, environ, root, cwd=None):
        assert root == work
        assert environ.get(flag_key) == (user_flags if explicit_sdk_flags else None)
        calls.append(names)
        return proof_plan.ToolchainProjection(
            delta={flag_key: "-C link-self-contained=no"} if "sdk" in names else {},
            missing=(),
        )

    monkeypatch.setattr(proof_plan, "project_toolchains", project)
    monkeypatch.setattr(
        proof_plan,
        "_version_fingerprint",
        lambda policy, **_kw: {
            "version": "ok",
            "launcher_sha256": "a" * 64,
            "executable_sha256": "b" * 64,
        },
    )
    assert (
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))
        == drv.EXIT_OK
    )
    assert json.loads(outputs[0].read_text(encoding="utf-8")) == [
        user_flags if explicit_sdk_flags else None,
        "-C debuginfo=1",
        [upstream],
    ]
    for output in outputs[1:]:
        assert json.loads(output.read_text(encoding="utf-8")) == [
            "-C link-self-contained=no",
            "-C debuginfo=1",
            [upstream],
        ]
    # One union admission and one projection per distinct child selection.
    assert calls == [("rust", "sdk"), ("rust",), ("sdk",)]


def _detached_managed_environment(tmp_path):
    from molt.dx import checkout_component
    from molt.build_state_layout import build_state_root

    checkout = tmp_path / "project"
    checkout.mkdir()
    artifact = tmp_path / "artifacts"
    parent = artifact / "target" / "checkouts" / checkout_component(checkout)
    session = parent / "sessions" / "reader"
    environment = {
        **os.environ,
        "CARGO_TARGET_DIR": str(session),
        "MOLT_EXT_ROOT": str(artifact),
    }
    control = build_state_root(
        project_root=checkout, cargo_target=session, environment=environment
    )
    state = tmp_path / "detached-state"
    state.mkdir()
    return environment, (parent, session, control), state


def _assert_detached_retirement_binding(roots, available):
    from molt.checkout_state import checkout_state_retirement

    for root in roots:
        with checkout_state_retirement(root) as admitted:
            assert admitted is available, root


def test_detached_ack_holds_idle_managed_roots_through_actual_child(tmp_path):
    from tools import molt_dev_detached as detached

    environment, roots, state = _detached_managed_environment(tmp_path)
    child_ready, release = tmp_path / "child-ready", tmp_path / "release"
    script = tmp_path / "child.py"
    script.write_text(
        "import pathlib,time\n"
        f"pathlib.Path({str(child_ready)!r}).touch()\n"
        f"release=pathlib.Path({str(release)!r})\n"
        "deadline=time.monotonic()+15\n"
        "while not release.exists() and time.monotonic()<deadline: time.sleep(.02)\n"
        "assert release.exists(), 'test release deadline'\n",
        encoding="utf-8",
    )
    pid = detached._detached_daemonize(
        state, [sys.executable, str(script)], tmp_path, environment
    )
    try:
        ready = json.loads((state / "state-use-ready.json").read_text(encoding="utf-8"))
        assert ready["pid"] == pid
        assert int((state / "pid").read_text(encoding="utf-8")) == ready["pid"]
        assert (state / "sid").read_text(encoding="utf-8") == (
            f"windows-process-group:{pid}" if os.name == "nt" else str(pid)
        )
        deadline = time.monotonic() + 10
        while not child_ready.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        assert child_ready.exists(), (state / "run.log").read_text(encoding="utf-8")
        # A stale activity stamp cannot stand in for the actual lifetime hold.
        old = time.time() - 40 * 86400
        for root in roots:
            for path in (
                root,
                root / ".molt-last-use",
                root / ".molt-state-owner.json",
            ):
                if path.exists():
                    os.utime(path, (old, old))
        _assert_detached_retirement_binding(roots, False)
    finally:
        release.touch()
        deadline = time.monotonic() + 10
        while not (state / "rc").exists() and time.monotonic() < deadline:
            time.sleep(0.02)
    assert (state / "rc").read_text(encoding="utf-8") == "0"
    # rc is written before the worker's outer use scope closes.
    deadline = time.monotonic() + 5
    from molt.checkout_state import checkout_state_retirement

    while time.monotonic() < deadline:
        with checkout_state_retirement(roots[0]) as admitted:
            if admitted:
                break
        time.sleep(0.02)
    _assert_detached_retirement_binding(roots, True)


def test_detached_invalid_ack_retains_parent_use_after_exception_gc(
    tmp_path, monkeypatch
):
    import atexit
    import gc
    import weakref
    from tools import molt_dev_detached as detached

    environment, roots, state = _detached_managed_environment(tmp_path)
    (state / "state-use-ready.json").write_text('{"stale": true}', encoding="utf-8")
    start = detached._COMMANDS.start_owned
    process = None

    def child(self, command, **kwargs):
        nonlocal process
        process = start([sys.executable, "-c", HUNG_CHILD_SOURCE], env=environment)
        return process

    monkeypatch.setattr(type(detached._COMMANDS), "start_owned", child)
    try:
        with pytest.raises(detached.DriverError, match="invalid use handoff") as caught:
            detached._detached_daemonize(state, ["unused"], tmp_path, environment)
        stack_ref = weakref.ref(caught.value.state_use)
        handoff = caught.value.state_owner
        assert caught.value.owned_process is process
        del caught
        gc.collect()
        assert stack_ref() is not None
        assert process is not None and process.poll() is None
        _assert_detached_retirement_binding(roots, False)
    finally:
        assert process is not None
        process.terminate()
        process.wait()
        atexit.unregister(handoff._close_at_exit)
        handoff._close_at_exit()
    _assert_detached_retirement_binding(roots, True)


def test_detached_spawn_failure_releases_prelaunch_use(tmp_path, monkeypatch):
    from tools import molt_dev_detached as detached

    environment, roots, state = _detached_managed_environment(tmp_path)

    def refuse(self, command, **kwargs):
        _assert_detached_retirement_binding(roots, False)
        raise FileNotFoundError("independent detached launch refusal")

    monkeypatch.setattr(type(detached._COMMANDS), "start_owned", refuse)
    with pytest.raises(FileNotFoundError, match="independent detached launch refusal"):
        detached._detached_daemonize(state, ["unused"], tmp_path, environment)
    _assert_detached_retirement_binding(roots, True)


@pytest.mark.parametrize("completion", ["terminal", "acknowledged", "unknown"])
def test_detached_retry_preserves_unresolved_birth_and_requires_worker_disposition(
    drv, tmp_path, capsys, monkeypatch, completion
):
    from types import SimpleNamespace
    from tools import molt_dev_detached as detached

    state_root = tmp_path / "detached"
    state = state_root / "retry"
    release = tmp_path / "release-worker"
    children = []
    owner = None
    start = detached._COMMANDS.start_owned
    command = [sys.executable, "-c", "print('replacement completed')"]
    args = _dr_ns(drv, "retry", state_root, command)

    def launch(argv, **kwargs):
        if children:
            process = start(argv, **kwargs)
        else:
            script = (
                "import json,os,pathlib,sys,time\n"
                "payload=pathlib.Path(sys.argv[1]); state=payload.parent\n"
                "token=json.loads(payload.read_text())['use_token']\n"
                "ready=state/'state-use-ready.json'\n"
                "pending=state/'ready.tmp'; pending.write_text('null'); os.replace(pending,ready)\n"
                "release=pathlib.Path(sys.argv[2]); deadline=time.monotonic()+20\n"
                "while not release.exists() and time.monotonic()<deadline: time.sleep(.01)\n"
                "assert release.exists(), 'test release deadline'\n"
                "mode=sys.argv[3]\n"
                "if mode=='terminal': (state/'rc').write_text('0')\n"
                "if mode=='acknowledged': ready.write_text(json.dumps({'schema_version':1,'pid':os.getpid(),'use_token':token}))\n"
            )
            process = start(
                [
                    sys.executable,
                    "-I",
                    "-S",
                    "-c",
                    script,
                    str(state / "worker.json"),
                    str(release),
                    completion,
                ],
                **kwargs,
            )
        children.append(process)
        return process

    monkeypatch.setattr(detached, "_COMMANDS", SimpleNamespace(start_owned=launch))
    try:
        with pytest.raises(detached.DriverError, match="invalid use handoff") as caught:
            detached.cmd_detached_run(args)
        owner = caught.value.state_owner
        assert caught.value.owned_process is children[0]
        assert children[0].poll() is None
        del caught
        assert not (state / "pid").exists()
        preserved = {
            path.name: path.read_bytes() for path in state.iterdir() if path.is_file()
        }
        for replace in (False, True):
            with pytest.raises(detached.DriverError, match="INCOMPLETE"):
                detached.cmd_detached_run(
                    _dr_ns(drv, "retry", state_root, command, replace=replace)
                )
            assert len(children) == 1 and children[0].poll() is None
            assert {
                path.name: path.read_bytes()
                for path in state.iterdir()
                if path.is_file()
            } == preserved
        release.touch()
        assert children[0].wait(timeout=5) == 0
        owner._close_at_exit()
        if completion == "unknown":
            # A dead launcher record does not establish the actual worker's
            # identity or death (Windows may delegate to a different PID).
            (state / "pid").write_text(str(children[0].pid), encoding="utf-8")
            for replace in (False, True):
                with pytest.raises(detached.DriverError, match="INCOMPLETE"):
                    detached.cmd_detached_run(
                        _dr_ns(drv, "retry", state_root, command, replace=replace)
                    )
            assert len(children) == 1
        else:
            with pytest.raises(detached.DriverError, match="--replace"):
                detached.cmd_detached_run(args)
            assert (
                detached.cmd_detached_run(
                    _dr_ns(drv, "retry", state_root, command, replace=True)
                )
                == detached.EXIT_OK
            )
            ready = json.loads(
                (state / "state-use-ready.json").read_text(encoding="utf-8")
            )
            assert int((state / "pid").read_text(encoding="utf-8")) == ready["pid"]
            assert _wait_done(drv, capsys, "retry", state_root)["rc"] == 0
            assert "replacement completed" in (state / "run.log").read_text(
                encoding="utf-8"
            )
    finally:
        release.touch()
        for process in children:
            try:
                process.wait(timeout=5)
            except BaseException:
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=5)
                raise
        if owner is not None:
            owner._close_at_exit()


def test_probe_unknown_never_admits_replacement_or_reports_death(
    drv, tmp_path, capsys, monkeypatch
):
    from molt import process_model
    from tools import molt_dev_detached as detached, molt_dev_probe as probe

    def unknown(*args):
        raise OSError("independent process query unavailable")

    # Only this thread's queries fail; the session sentinel keeps real ones.
    install_thread_module_view(
        monkeypatch, "process_model", process_model, probe, pid_alive=unknown
    )
    pid = os.getpid()
    result = probe.probe_pid(pid)
    assert result["alive"] is None and "unavailable" in result["detail"]
    ns = _ns(drv, file=None, pid=[pid], json=True)
    assert drv.cmd_probe(ns) == drv.EXIT_FAIL
    assert json.loads(capsys.readouterr().out)[0]["alive"] is None
    ns.json = False
    assert drv.cmd_probe(ns) == drv.EXIT_FAIL
    assert "alive=unknown" in capsys.readouterr().err

    state_root = tmp_path / "detached"
    state = state_root / "unknown"
    state.mkdir(parents=True)
    token = "b" * 32
    (state / "pid").write_text(str(pid), encoding="utf-8")
    (state / "worker.json").write_text(
        json.dumps({"use_token": token}), encoding="utf-8"
    )
    (state / "state-use-ready.json").write_text(
        json.dumps({"schema_version": 1, "pid": pid, "use_token": token}),
        encoding="utf-8",
    )
    before = {p.name: p.read_bytes() for p in state.iterdir()}
    rc, verdict = _verify_json(drv, capsys, _dv_ns(drv, "unknown", state_root))
    assert rc == drv.EXIT_FAIL and verdict["status"] == "unknown"
    assert "unavailable" in verdict["detail"]
    with pytest.raises(detached.DriverError, match="INCOMPLETE"):
        detached.cmd_detached_run(
            _dr_ns(drv, "unknown", state_root, ["must-not-launch"], replace=True)
        )
    assert {p.name: p.read_bytes() for p in state.iterdir()} == before


@pytest.mark.parametrize("cleanup_fails", [False, True])
def test_detached_terminal_publication_follows_actual_child_and_owner_cleanup(
    tmp_path, monkeypatch, cleanup_fails
):
    from contextlib import contextmanager
    from tools import molt_dev_detached as detached

    state = tmp_path / "worker"
    state.mkdir()
    marker = tmp_path / "command-finished"
    payload = state / "worker.json"
    payload.write_text(
        json.dumps(
            {
                "argv": [
                    sys.executable,
                    "-B",
                    "-c",
                    "from pathlib import Path; import sys; Path(sys.argv[1]).write_text('finished')",
                    str(marker),
                ],
                "cwd": str(Path.cwd()),
                "use_token": "c" * 32,
            }
        ),
        encoding="utf-8",
    )
    real_use = detached.detached_command_use
    observations = []

    @contextmanager
    def closing_use(*args, **kwargs):
        with real_use(*args, **kwargs):
            yield
        # A parseable terminal record authorizes another caller to replace the
        # directory. Observe its absence at the actual owner's final cleanup.
        observations.append(
            (marker.read_text(encoding="utf-8"), (state / "rc").exists())
        )
        if cleanup_fails:
            raise OSError("independent owner cleanup failure")

    monkeypatch.setattr(detached, "detached_command_use", closing_use)
    assert detached._detached_worker_main(payload) == 0
    assert observations == [("finished", False)]
    assert (state / "rc").read_text(encoding="utf-8") == (
        "126" if cleanup_fails else "0"
    )
    log = (state / "run.log").read_text(encoding="utf-8")
    assert ("independent owner cleanup failure" in log) is cleanup_fails


@pytest.mark.parametrize("outcome", [0, 1])
def test_windows_pinned_controller_preserves_logical_context_and_releases_owned_cwd(
    origin_and_clone, tmp_path, driver_seed, outcome
):
    """The actual new pinned consumer resumes context; supervisors stay outside."""
    from tools import molt_dev_controller
    from tools.venv_exec import venv_python

    _origin, main = origin_and_clone
    wt = _worktree_off(main, "wt_controller_cwd")
    (wt / "relative-tmp").mkdir()
    if outcome:
        (wt / "README.md").write_text("independent tracked edit\n", encoding="utf-8")
    key = "c" * 32
    cache = main / ".git" / "molt" / "controllers" / key
    copy_driver_trees(cache, seed=driver_seed)
    # Use the controller's interpreter, without the proof environment's
    # editable-install startup hooks binding Molt to a different source tree.
    molt_dev_controller._sync_environment(cache)
    (cache / ".molt-controller.json").write_text("{}\n", encoding="utf-8")
    report = tmp_path / "controller-context.json"
    state = tmp_path / "preserved-state"
    state.write_bytes(b"independent state")
    driver = cache / "tools" / "molt_dev.py"
    probe = f"""
import json, os, sys, tempfile
from pathlib import Path
sys.path[:0] = [{str(cache / "src")!r}, {str(cache)!r}]
from tools import molt_dev as actual
from tools import molt_dev_controller as controller
from tools import molt_dev_retire

assert Path(actual.__file__).resolve() == Path(__file__).resolve()
args = actual.build_parser().parse_args(sys.argv[1:])
physical_cwd = Path.cwd()
controller.resume_controller_cwd(args, driver_file=__file__)
repo = Path(args.repo)
assert Path.cwd() == repo
assert args.controller_cwd is None
assert args.marker == ["exists:unchanged-relative"]
assert os.environ["CARGO_TARGET_DIR"] == "relative-target"
assert os.environ["CARGO_HOME"] == "relative-cargo-home"
assert os.environ["TMPDIR"] == "relative-tmp"
assert os.environ["TMP"] == "relative-tmp"
assert os.environ["TEMP"] == "relative-tmp"
selected_temp = Path(tempfile.gettempdir())
assert selected_temp == repo / "relative-tmp"
assert not (Path(__file__).resolve().parents[1] / "relative-tmp").exists()
temporary_fd = os.open(Path(os.environ["TMPDIR"]) / "selected-child", os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
try:
    os.write(temporary_fd, b"logical temporary output")
finally:
    os.close(temporary_fd)
temporary_bytes = (selected_temp / "selected-child").read_bytes()
# Check repository association only after the original environment context is
# restored; this is the same existing identity owner used by command dispatch.
common = actual._git_common_directory(repo)
assert common is not None
controller.verify_pinned_controller(args.pinned_controller, driver_file=__file__, common_dir=common)
result = molt_dev_retire.cleanup_worktree(actual.Git(repo), repo, "origin/main", actual.DEFAULT_IGNORE_GLOBS, force_sha=None)
Path({str(report)!r}).write_text(json.dumps({{
    "physical_cwd": str(physical_cwd), "logical_cwd": str(repo),
    "argv": sys.argv[1:], "remaining_context": args.controller_cwd,
    "selected_temp": str(selected_temp), "temporary_bytes": temporary_bytes.decode("ascii"),
    "result": result, "repo_exists": repo.exists(),
}}), encoding="utf-8")
raise SystemExit(result)
"""
    # Give every imported consumer the same fixture-owned pinned source tree.
    # Replace only the script entrypoint; tools.molt_dev still imports its real
    # implementation, and all source-derived launch directories remain local.
    original_driver = driver.read_text(encoding="utf-8")
    entrypoint = "    raise SystemExit(main(sys.argv[1:]))"
    assert original_driver.count(entrypoint) == 1
    driver.write_text(
        original_driver.replace(entrypoint, textwrap.indent(probe, "    ")),
        encoding="utf-8",
    )
    # Isolate the process-wide cwd transition from pytest's guard threads.
    code = f"""
import os, sys
from pathlib import Path
from types import SimpleNamespace
sys.path[:0] = [{str(cache / "src")!r}, {str(cache)!r}]
from tools import molt_dev_controller as controller
os.chdir({str(wt)!r})
assert Path.cwd() == Path({str(wt)!r})
os.environ.update(CARGO_TARGET_DIR="relative-target", CARGO_HOME="relative-cargo-home", TMPDIR="relative-tmp", TMP="relative-tmp", TEMP="relative-tmp")
os.environ["MOLT_SOURCE_ROOT"] = os.path.relpath({str(cache)!r}, Path.cwd())
real_os = controller.os
controller.os = SimpleNamespace(name="nt", chdir=real_os.chdir, environ=real_os.environ)
rc = controller.run_controller(controller.Controller({key!r}, Path({str(cache)!r})),
    ["integrate", "--repo", ".", "--marker", "exists:unchanged-relative", "--no-push"], repo=Path("."))
assert Path.cwd() == Path({str(cache)!r})
assert rc == {outcome}
raise SystemExit(0)
"""
    completed = run_guarded_test_process(
        [str(venv_python(cache / ".venv")), "-I", "-c", code],
        cwd=tmp_path,
        env=ProcessEnvironment(os.environ),
        timeout=120,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    observed = json.loads(report.read_text(encoding="utf-8"))
    assert Path(observed["physical_cwd"]) == cache
    assert Path(observed["logical_cwd"]) == wt
    assert observed["remaining_context"] is None
    assert Path(observed["selected_temp"]) == wt / "relative-tmp"
    assert observed["temporary_bytes"] == "logical temporary output"
    assert observed["result"] == outcome
    assert observed["repo_exists"] is (outcome != 0)
    assert wt.exists() is (outcome != 0)
    assert state.read_bytes() == b"independent state"
    # Imported implementation sources supply code, not this fixture's output
    # cwd. Relative selections must never materialize in the proof checkout.
    assert not (REPO_ROOT / "relative-tmp").exists()


@pytest.mark.parametrize("command", [["integrate"], ["queue", "run"]])
@pytest.mark.parametrize(
    "bad",
    [
        "unpinned",
        "relative",
        "missing",
        "missing-field",
        "nonexistent",
        "wrong-driver",
        "relative-repo",
    ],
)
def test_pinned_controller_context_rejects_before_command_selection(
    tmp_path, monkeypatch, command, bad
):
    from tools import molt_dev_controller as controller

    def forbidden(*_args, **_kwargs):
        raise AssertionError("selected command or Git before admitting context")

    monkeypatch.setattr(controller, "runs_from_pinned_controller", forbidden)
    monkeypatch.setattr(molt_dev, "_git_common_directory", forbidden)
    install_module_os_view(monkeypatch, controller, name="nt")
    before = Path.cwd()
    args = [*command, "--repo", "." if bad == "relative-repo" else str(tmp_path)]
    if bad != "unpinned":
        args.extend(["--pinned-controller", "c" * 32])
    if bad != "missing-field":
        args.append("--controller-cwd")
    if bad not in {"missing", "missing-field"}:
        args.append(
            "relative"
            if bad == "relative"
            else str(tmp_path / "absent")
            if bad == "nonexistent"
            else str(tmp_path)
        )
    if bad == "missing":
        with pytest.raises(SystemExit) as caught:
            molt_dev.main(args)
        assert caught.value.code == 2
    else:
        assert molt_dev.main(args) != 0
    assert Path.cwd() == before


def test_controller_parser_defers_temporary_state_selection_until_command_use(tmp_path):
    code = f"""
import sys, tempfile
from pathlib import Path
sys.path[:0] = [{str(REPO_ROOT / "src")!r}, {str(REPO_ROOT)!r}]
def forbidden():
    raise AssertionError("temporary selection before controller context")
tempfile.gettempdir = forbidden
from tools import molt_dev
from tools import molt_dev_detached
parser = molt_dev.build_parser()
assert parser.parse_args(["difftest", "sample.py"]).out_dir is None
assert parser.parse_args(["difftest", "sample.py", "--out-dir", ""]).out_dir == ""
parser.parse_args(["integrate", "--repo", {str(tmp_path)!r}, "--dry-run"])
parser.parse_args(["queue", "run", "--repo", {str(tmp_path)!r}])
tempfile.gettempdir = lambda: {str(tmp_path)!r}
assert molt_dev_detached._detached_state_dir("lazy", None) == Path({str(tmp_path)!r}) / "molt_dev_detached" / "lazy"
print("lazy-temporary-state")
"""
    completed = run_guarded_test_process(
        [sys.executable, "-I", "-c", code], cwd=tmp_path, timeout=60
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    assert completed.stdout.strip() == "lazy-temporary-state"


@pytest.mark.skipif(os.name != "posix", reason="direct POSIX interpreter startup")
def test_detached_observed_pre_ack_death_publishes_failure_before_replacement(
    tmp_path, monkeypatch
):
    from tools import molt_dev_detached as detached
    from tools.command_execution import CommandExecutor

    original = CommandExecutor.start_owned
    children = []

    def exit_before_ack(self, command, **kwargs):
        child = original(
            self, [sys.executable, "-I", "-S", "-c", "raise SystemExit(17)"], **kwargs
        )
        children.append(child)
        return child

    monkeypatch.setattr(CommandExecutor, "start_owned", exit_before_ack)
    state = tmp_path / "startup-state"
    state.mkdir()
    with pytest.raises(detached.DriverError, match="worker exited 17"):
        detached._detached_daemonize(
            state, [sys.executable, "-c", "pass"], tmp_path, dict(os.environ)
        )
    assert len(children) == 1 and children[0].poll() == 17
    assert (state / "pid").read_text(encoding="utf-8") == str(children[0].pid)
    assert (state / "rc").read_text(encoding="utf-8") == "17"
    with detached._detached_launch_admission(state, replace=True):
        assert list(state.iterdir()) == []


def test_queue_withdraw_closes_pending_requests_only_between_batches(
    drv, origin_and_clone, monkeypatch
):
    from tools import molt_dev_queue as queue

    _origin, work = origin_and_clone
    # A `land` that probed the lock while withdraw held it started no runner,
    # so withdraw starts one while requests remain. Record it; never spawn.
    started = []
    monkeypatch.setattr(
        queue,
        "_start_queue_runner",
        lambda paths, repo, args: started.append(
            (paths, repo, args.remote, args.branch)
        ),
    )
    paths = queue.queue_paths(work)
    first, second, third = (
        queue.enqueue(
            paths, head=f"{index:040x}", worktree=work, branch=f"lane/{index}"
        )
        for index in range(1, 4)
    )
    # A bisection plan from an earlier runner still names the second request.
    queue._write_plan(paths, [[first.id, second.id], [third.id]])

    def withdraw(*request_ids: str) -> int:
        args = drv.build_parser().parse_args(
            ["queue", "withdraw", "--repo", str(work), *request_ids,
             "--reason", "review found defects"]
        )  # fmt: skip
        return args.func(args)

    assert withdraw(second.id, second.id) == drv.EXIT_OK
    assert started == [(paths, work.resolve(), "origin", "main")]
    result = queue.read_result(paths, second.id)
    assert result["status"] == queue.LandingStatus.WITHDRAWN
    assert result["detail"] == "review found defects"
    assert queue.pending(paths) == [first, third]
    assert queue.next_batch(paths, queue.pending(paths)) == [first]
    assert [row["request"]["id"] for row in queue.recorded_results(paths)] == [
        second.id
    ]
    # All or nothing: one unknown id withdraws nothing.
    with pytest.raises(drv.DriverError, match="no-such-request"):
        withdraw(third.id, "no-such-request")
    assert queue.read_result(paths, third.id) is None
    # A runner holds the lock for each batch; nothing is withdrawn under it.
    monkeypatch.setattr(queue, "RUNNER_LOCK_WAIT_S", 0.1)
    handle = drv._try_acquire_file_lock(paths.runner_lock)
    assert handle is not None
    try:
        assert withdraw(third.id) == drv.EXIT_FAIL
    finally:
        drv._release_file_lock(handle)
    assert queue.pending(paths) == [first, third]
    # The last pending requests close; no runner starts for an empty queue.
    started.clear()
    assert withdraw(first.id, third.id) == drv.EXIT_OK
    assert queue.pending(paths) == [] and started == []


def test_runner_liveness_probes_never_read_each_other_as_a_runner(
    drv, tmp_path, monkeypatch
):
    from tools import molt_dev_queue as queue

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    paths.root.mkdir(parents=True)
    spawned = []
    monkeypatch.setattr(drv, "cmd_detached_run", lambda args: spawned.append(args))
    args = drv.build_parser().parse_args(["queue", "run", "--repo", str(tmp_path)])
    # A `land --wait` poller holds its probe: a runner still starts.
    poller = drv._try_acquire_file_lock(paths.runner_lock, shared=True)
    assert poller is not None
    try:
        queue._start_queue_runner(paths, tmp_path, args)
    finally:
        drv._release_file_lock(poller)
    assert len(spawned) == 1
    # A runner (or a withdrawal) holds it exclusively: none starts.
    runner = drv._try_acquire_file_lock(paths.runner_lock)
    assert runner is not None
    try:
        queue._start_queue_runner(paths, tmp_path, args)
    finally:
        drv._release_file_lock(runner)
    assert len(spawned) == 1


def test_a_probe_never_sends_a_runner_away(drv, tmp_path, monkeypatch):
    import molt.file_locks as file_locks
    from tools import molt_dev_queue as queue

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    paths.root.mkdir(parents=True)
    # A `land --wait` poller holds its shared probe as a runner starts, and
    # lets go between the runner's polls.
    probe = drv._try_acquire_file_lock(paths.runner_lock, shared=True)
    assert probe is not None
    real_try = file_locks._try_acquire_file_lock
    attempts = []

    def probe_ends_after_the_first_try(path, **kwargs):
        handle = real_try(path, **kwargs)
        attempts.append(handle is not None)
        if len(attempts) == 1:
            drv._release_file_lock(probe)
        return handle

    monkeypatch.setattr(
        file_locks, "_try_acquire_file_lock", probe_ends_after_the_first_try
    )
    handle = queue._take_runner_lock(paths)
    assert handle is not None and attempts == [False, True]
    drv._release_file_lock(handle)
    monkeypatch.setattr(file_locks, "_try_acquire_file_lock", real_try)
    # A runner holds it for the whole wait: the next one stands aside.
    monkeypatch.setattr(queue, "RUNNER_LOCK_WAIT_S", 0.1)
    runner = drv._try_acquire_file_lock(paths.runner_lock)
    assert runner is not None
    try:
        assert queue._take_runner_lock(paths) is None
    finally:
        drv._release_file_lock(runner)


def test_queue_results_read_in_the_order_they_closed(tmp_path, monkeypatch):
    from tools import molt_dev_queue as queue

    paths = queue.QueuePaths(tmp_path / "landing-queue")
    stamps = iter(f"2026-10-21T12:00:0{second}+00:00" for second in (0, 1, 2, 3, 4, 5))
    monkeypatch.setattr(queue, "_now", lambda: next(stamps))
    early, late = (
        queue.enqueue(paths, head=f"{index:040x}", worktree=tmp_path, branch=None)
        for index in (1, 2)
    )
    # A bisected batch closes the later request first.
    queue.record_result(paths, late, status=queue.LandingStatus.LANDED, detail="")
    queue.record_result(paths, early, status=queue.LandingStatus.FAILED, detail="")
    closed = [row["request"]["id"] for row in queue.recorded_results(paths)]
    assert closed == [late.id, early.id]
    assert [row["request"]["id"] for row in queue.recorded_results(paths, limit=1)] == [
        early.id
    ]


def test_queue_refuses_old_requests_without_removing_them(tmp_path):
    from tools import molt_dev_queue as queue

    paths = queue.QueuePaths(tmp_path)
    request = queue.enqueue(paths, head="1" * 40, worktree=tmp_path, branch=None)
    path = paths.requests / f"{request.id}.json"
    record = json.loads(path.read_text(encoding="utf-8"))
    assert record["schema"] == "molt.landing-request.v2"
    assert "retire" not in record
    record.update(schema="molt.landing-request.v1", retire=False)
    path.write_text(json.dumps(record), encoding="utf-8")
    before = path.read_bytes()
    with pytest.raises(ValueError, match="preserve it.*runner lock"):
        queue.pending(paths)
    assert path.read_bytes() == before
    assert not paths.results.exists()


def test_land_has_no_optional_retirement_flag(drv):
    with pytest.raises(SystemExit) as error:
        drv.build_parser().parse_args(["land", "--retire"])
    assert error.value.code == 2


@pytest.mark.parametrize(
    "case",
    [
        "missing",
        "missing_parameter",
        "import_error",
        "named",
        "parameter",
        "body_failure",
    ],
)
def test_selected_pytest_nodes_collect_before_build_and_still_execute(
    drv,
    tmp_path,
    case,
):
    from dataclasses import replace

    (tmp_path / "tests").mkdir()
    # This is a real isolated checkout: admit the same repository custody and
    # memory-capacity plugins, rather than faking the canonical pytest option.
    (tmp_path / "pytest.ini").write_text(
        "[pytest]\naddopts = -p molt.pytest_memory_guard_config_plugin "
        "-p molt.pytest_capacity_plugin\nmarkers = excluded: selector control\n",
        encoding="utf-8",
    )
    test_file = tmp_path / "tests/test_selected.py"
    executed = tmp_path / "executed.txt"
    if case == "import_error":
        source = "raise RuntimeError('independent collection import failure')\n"
    else:
        source = (
            "import pytest\nfrom pathlib import Path\n"
            "@pytest.mark.parametrize('value', [1], ids=['space & quote'])\n"
            "def test_one(value):\n"
            f"    Path({str(executed)!r}).write_text('executed', encoding='utf-8')\n"
            f"    assert {case != 'body_failure'!r}\n"
        )
    test_file.write_text(source, encoding="utf-8")
    suffix = {
        "missing": "test_missing",
        "missing_parameter": "test_one[missing]",
        "parameter": "test_one[space & quote]",
    }.get(case, "test_one")
    target = "tests/test_selected.py::" + suffix
    selection = 'm = "not excluded"' if case == "named" else 'k = "one or missing"'
    policy = (
        'always = []\n[[rule]]\nname = "selected"\nglobs = ["source.py"]\n'
        'gates = ["mock-rust-build"]\ntoolchains = ["python"]\n'
        + "pytest_select = [{targets = ["
        + json.dumps(target)
        + "], "
        + selection
        + "}]\n"
    )
    config = GateConfig.parse(policy, "real collection boundary", root=tmp_path)
    gates, _ = config.select(["source.py"])
    assert gates[0].command == "mock-rust-build"
    cohort = next(gate for gate in gates if gate.pytest)
    assert cohort.collection_command is not None
    expected = list(cohort.command)
    expected[expected.index("-n") + 1] = "0"
    expected.append("--collect-only")
    assert list(cohort.collection_command) == expected

    observed = []
    progress = []

    class Progress:
        def emit(self, event, **details):
            progress.append((event, details))

    def execute(command, repo, env, cancelled, streams=None):
        observed.append(command)
        if command == "mock-rust-build":
            assert not executed.exists(), "collection executed test bodies"
            return _gate_outcome(0, "mock Rust completed", "", None)
        # Use real selected Python, pytest and canonical guard. No fake collector
        # verdict, shell parsing, host env mutation, or actual Rust compilation.
        return drv.run_captured_gate(command, repo, env, cancelled, streams)

    runtime = replace(
        _gate_runtime_fixture(
            molt_dev_gates, tmp_path, {}, output_dir=tmp_path / "logs"
        ),
        environments={
            ("python",): GateEnvironment(
                ProcessEnvironment(os.environ).updated(
                    {
                        "MOLT_VENV": sys.prefix,
                        "PYTEST_XDIST_AUTO_NUM_WORKERS": "1",
                    }
                ),
                Path(sys.prefix),
                False,
            )
        },
        run_captured_gate=execute,
        progress=Progress(),
    )
    timings = []
    succeeds = case in {"named", "parameter"}
    if succeeds:
        molt_dev_gates.run_selected_gates(gates, runtime, timings, host_limit=lambda: 1)
    else:
        with pytest.raises(GateFailure):
            molt_dev_gates.run_selected_gates(
                gates, runtime, timings, host_limit=lambda: 1
            )
    assert observed[0] == cohort.collection_command
    collection_failure = case in {"missing", "missing_parameter", "import_error"}
    if collection_failure:
        assert observed == [cohort.collection_command]
        assert not executed.exists() and not timings
        output = "\n".join(
            path.read_text(encoding="utf-8")
            for path in (tmp_path / "logs/collection").glob("*.log")
        )
        assert (
            "independent collection import failure"
            if case == "import_error"
            else "not found"
        ) in output
    else:
        assert observed == [
            cohort.collection_command,
            "mock-rust-build",
            cohort.command,
        ]
        assert executed.read_text(encoding="utf-8") == "executed"
        assert len(timings) == len(gates)
        assert any(event == "collection_finished" for event, _ in progress)
        assert any(event == "gate_finished" for event, _ in progress)
    # A passing collection cannot conceal the later real assertion failure.
    if case == "body_failure":
        assert progress[-1][1]["returncode"] != 0


def test_file_only_pytest_cohort_keeps_single_execution_without_collection(
    drv, tmp_path
):
    (tmp_path / "tests").mkdir()
    (tmp_path / "tests/test_file.py").write_text(
        "def test_one(): pass\n", encoding="utf-8"
    )
    config = GateConfig.parse(
        'always=[]\n[[rule]]\nname="file"\nglobs=["source.py"]\ngates=[]\n'
        'pytest=["tests/test_file.py"]\ntoolchains=["python"]\n',
        "file-only",
        root=tmp_path,
    )
    gates, _ = config.select(["source.py"])
    assert len(gates) == 1 and gates[0].pytest
    assert gates[0].collection_command is None


def test_merged_pytest_collection_admits_only_explicit_nodes(drv, tmp_path):
    # Synthetic 52-target cohort: 47 whole files and five nodes across two files.
    # Whole-file imports can be expensive or have side effects; they belong only
    # to the later mandatory run, not to named-node admission.
    (tmp_path / "tests").mkdir()
    files = [f"tests/test_file_{index}.py" for index in range(47)]
    nodes = [
        f"tests/test_nodes_{index % 2}.py::test_case_{index}" for index in range(5)
    ]
    for target in [*files, *nodes]:
        (tmp_path / target.split("::", 1)[0]).touch()
    policy = (
        'always=[]\n[[rule]]\nname="merged"\nglobs=["source.py"]\ngates=[]\n'
        + "pytest="
        + json.dumps([*files, *nodes])
        + '\ntoolchains=["python"]\n'
    )
    config = GateConfig.parse(policy, "synthetic mixed cohort", root=tmp_path)
    gates, _ = config.select(["source.py"])
    assert len(gates) == 1
    gate = gates[0]
    assert gate.pytest and gate.toolchains == ("python",)
    assert isinstance(gate.command, tuple)
    assert gate.command[-52:] == tuple([*files, *nodes])
    assert gate.command[:6] == ("python", "-m", "pytest", "-q", "-n", "auto")
    admission = gate.collection_command
    assert admission is not None
    assert admission[:6] == ("python", "-m", "pytest", "-q", "-n", "0")
    assert admission[-6:] == (*nodes, "--collect-only")
    assert not set(files).intersection(admission)
    assert len(admission) == len(gate.command) - len(files) + 1


def test_filtered_pytest_collection_retains_whole_selected_envelope(drv, tmp_path):
    # A selector applies to its whole cohort. Keep this declared envelope intact
    # rather than silently narrowing -k/-m to just its explicit-node members.
    (tmp_path / "tests").mkdir()
    for name in ("tests/test_file.py", "tests/test_nodes.py"):
        (tmp_path / name).touch()
    config = GateConfig.parse(
        'always=[]\n[[rule]]\nname="filtered"\nglobs=["source.py"]\ngates=[]\n'
        'pytest_select=[{targets=["tests/test_file.py", "tests/test_nodes.py::test_one"], '
        'k="one or another"}]\ntoolchains=["python"]\n',
        "filtered mixed cohort",
        root=tmp_path,
    )
    gates, _ = config.select(["source.py"])
    assert len(gates) == 1
    gate = gates[0]
    expected = list(gate.command)
    expected[expected.index("-n") + 1] = "0"
    expected.append("--collect-only")
    assert gate.collection_command == tuple(expected)
    assert tuple(expected[-5:]) == (
        "-k",
        "one or another",
        "tests/test_file.py",
        "tests/test_nodes.py::test_one",
        "--collect-only",
    )


@pytest.mark.parametrize("selector", ["MOLT_VENV", "UV_PROJECT_ENVIRONMENT"])
def test_empty_gate_demand_preserves_explicit_selection_refusal(
    drv, tmp_path, selector
):
    repo = tmp_path / "project"
    repo.mkdir()
    marker = repo / "launched.txt"
    config = GateConfig.parse(
        'always = ["echo forbidden > launched.txt"]\n', "empty demand", root=repo
    )
    gates, _ = config.select(["source.py"])
    environment = _gate_caller_env(tmp_path / "host")
    environment[selector] = str(tmp_path / "missing explicit image")
    with pytest.raises(drv.DriverError):
        config.prepare(
            gates,
            baseline=environment,
            prepare_python=lambda env, needed: drv.prepare_gate_environment(
                repo, env, requires_python=needed
            ),
        )
    assert not marker.exists()
    assert not (repo / ".venv").exists()


def test_transitive_gate_environment_binds_fingerprint_and_rechecks_image(
    drv, tmp_path
):
    from tests.python_environment_test_support import (
        materialize_development_environment,
    )

    repo = tmp_path / "source"
    repo.mkdir()
    artifacts = tmp_path / "artifacts"
    image = materialize_development_environment(
        repo, artifacts, runnable=True, select=True
    )
    environment = _gate_caller_env(tmp_path / "host")
    environment["MOLT_EXT_ROOT"] = str(artifacts)
    policy = """
always = ["python -c pass"]
always_toolchains = ["transitive"]
[[toolchain_policy]]
name = "transitive"
members = ["source-interpreter"]
[[toolchain_policy]]
name = "source-interpreter"
executable = "{python}"
version_args = ["--version"]
version_pattern = '^Python 3\\.'
"""
    config = GateConfig.parse(policy, "transitive environment", root=repo)
    gates, _ = config.select(["source.py"])
    prepared = config.prepare(
        gates,
        baseline=environment,
        prepare_python=lambda env, needed: drv.prepare_gate_environment(
            repo, env, requires_python=needed
        ),
    )
    context = prepared.environments[("transitive",)]
    assert context.python_root == image.root and context.managed is True
    expected = image.root / ("Scripts/python.exe" if os.name == "nt" else "bin/python3")
    assert prepared.fingerprints["source-interpreter"]["path"] == str(expected)
    assert prepared.fingerprints["source-interpreter"]["path"] != str(
        Path(sys.executable)
    )
    output = tmp_path / "observed.json"
    command = (
        "python",
        "-c",
        "import json,sys;from pathlib import Path;Path(sys.argv[1]).write_text(json.dumps([sys.prefix,sys.pycache_prefix]),encoding='utf-8')",
        str(output),
    )
    assert drv.run_gate(command, repo, context) == 0
    assert json.loads(output.read_text(encoding="utf-8")) == [
        str(image.root),
        str(image.root / "bytecode"),
    ]
    # The context is a plan, not reusable physical admission. A changed recipe
    # after preparation must refuse before the next child can overwrite output.
    image.manifest_path.write_text("{}\n", encoding="utf-8")
    output.write_text("must survive", encoding="utf-8")
    with pytest.raises(drv.DriverError, match="managed development image refused"):
        drv.run_gate(command, repo, context)
    assert output.read_text(encoding="utf-8") == "must survive"


def test_runtime_only_gate_demand_keeps_target_failure_without_source_image(
    drv, tmp_path, monkeypatch
):
    from tools import proof_plan

    # The target catalog/capture itself has separate real runtime controls.
    # Keep its role literal here so this oracle challenges source-image demand,
    # including a runtime whose name looks like the default Python tool.
    runtime = proof_plan.ToolchainPolicy(
        "python",
        {
            "name": "python",
            "executable": "target-python",
            "python_identity_role": "runtime",
            "version_args": ["--version"],
            "version_pattern": "^ok$",
        },
        tmp_path,
    )
    config = GateConfig.parse(
        'always = ["echo runtime-only"]\nalways_toolchains = ["python"]\n',
        "runtime demand",
        root=tmp_path,
    )
    gates, _ = config.select(["source.py"])
    monkeypatch.setattr(
        proof_plan, "load_toolchain_policies", lambda *args, **kwargs: (runtime,)
    )

    def probe(policy, *, environment, python_executable):
        assert policy.python_identity_role == "runtime"
        assert python_executable is None
        raise OSError("independent target runtime probe failure")

    monkeypatch.setattr(proof_plan, "_version_fingerprint", probe)
    monkeypatch.setattr(
        drv,
        "_resolve_repo_venv",
        lambda *args: pytest.fail("source image selected for target runtime"),
    )
    with pytest.raises(
        drv.DriverError, match="independent target runtime probe failure"
    ):
        config.prepare(
            gates,
            baseline=_gate_caller_env(tmp_path / "host"),
            prepare_python=lambda env, needed: drv.prepare_gate_environment(
                tmp_path, env, requires_python=needed
            ),
        )
    assert not (tmp_path / ".venv").exists()
