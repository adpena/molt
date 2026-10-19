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
import venv
from pathlib import Path
from tests.process_guard_common import (
    install_module_os_view,
    run_guarded_test_process,
)

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = REPO_ROOT / "tools"


from tools import dirty_tree_policy, molt_dev  # noqa: E402
from tools.molt_dev_common import _run_fast_captured_command  # noqa: E402

SCRIPT_PATH = REPO_ROOT / "tools" / "molt_dev.py"
COMMITTED_GATES = REPO_ROOT / "tools" / "proof_plan.toml"
RUNTIME_GENERATION_MANIFEST = "wasm/molt_runtime.generation.json"
CUSTOM_IGNORED_PATH = "tmp/operator-scratch.txt"


@pytest.fixture
def drv():
    return molt_dev


# --------------------------------------------------------------------------
# git repo scaffolding (real repos, bare origin) — exercises the real flows
# --------------------------------------------------------------------------


def _git(repo: Path, *args: str, check: bool = True, input_text: str | None = None):
    env = os.environ.copy()
    env["GIT_TERMINAL_PROMPT"] = "0"
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


@pytest.fixture
def origin_and_clone(tmp_path, _origin_seed, monkeypatch):
    """Independent real bare origin and working clone for each integration.

    Copy files, never hardlink them: tests mutate refs, hooks, config and objects.
    Subject fetch/rebase/push operations and clone creation still use real Git.
    """
    # These repositories exercise generic shell gates, not the caller's venv.
    # Interpreter-selection tests construct and select their own environments.
    for key in ("MOLT_VENV", "UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV"):
        monkeypatch.delenv(key, raising=False)
    origin = tmp_path / "origin.git"
    shutil.copytree(_origin_seed, origin)
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


def test_origin_fixture_copies_history_without_mutable_aliases(
    drv, tmp_path, _origin_seed, monkeypatch
):
    for key in ("MOLT_VENV", "UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV"):
        monkeypatch.setenv(key, str(tmp_path / "missing-caller-environment"))
    origin, work = origin_and_clone.__wrapped__(tmp_path, _origin_seed, monkeypatch)
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
    relative = Path("objects") / initial[:2] / initial[2:]
    original_object = (_origin_seed / relative).read_bytes()
    (origin / relative).chmod(0o600)
    (origin / relative).write_bytes(b"deliberately corrupt private copy")
    assert (_origin_seed / relative).read_bytes() == original_object
    assert _git(_origin_seed, "show", "HEAD:README.md").stdout == "seed\n"


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


def _gate_caller_env(hostile: Path) -> dict[str, str]:
    env = dict(os.environ)
    for key in ("MOLT_VENV", "UV_PROJECT_ENVIRONMENT", "VIRTUAL_ENV"):
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
    repo = tmp_path / "main"
    repo.mkdir()
    main_repo = repo
    if selection.startswith("worktree-"):
        _init_repo(repo)
        _commit_file(repo, "README.md", "fixture\n", "fixture")
        main_repo = repo
        repo = tmp_path / "worktree"
        _git(main_repo, "worktree", "add", "-q", "-b", "gate-test", str(repo))
    selected = (
        main_repo / ".venv"
        if selection == "worktree-main"
        else tmp_path / "explicit environment"
        if selection in {"MOLT_VENV", "UV_PROJECT_ENVIRONMENT"}
        else repo / ".venv"
    )
    hostile = tmp_path / "hostile"
    _make_gate_venv(selected, "selected λ")
    _make_gate_venv(hostile, None)
    if selection == "worktree-own":
        _make_gate_venv(main_repo / ".venv", None)
    env = _gate_caller_env(hostile)
    if selection in {"MOLT_VENV", "UV_PROJECT_ENVIRONMENT"}:
        env[selection] = os.path.relpath(selected, repo)
    if selection == "MOLT_VENV":
        # The canonical explicit selector outranks both uv and checkout defaults.
        env["UV_PROJECT_ENVIRONMENT"] = str(hostile)
        _make_gate_venv(repo / ".venv", None)
    script = tmp_path / "observe.py"
    script.write_text(
        "import json, os, sys\n"
        "from pathlib import Path\n"
        "from molt_gate_selection_marker import TOKEN\n"
        "Path(sys.argv[1]).write_text(json.dumps({\n"
        " 'token': TOKEN, 'prefix': sys.prefix,\n"
        " 'virtual_env': os.environ['VIRTUAL_ENV'],\n"
        " 'molt_venv': os.environ['MOLT_VENV'],\n"
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
    assert drv.run_gate(command, repo, env) == 0
    assert env == before
    expected = {
        "token": "selected λ",
        "prefix": str(selected.resolve()),
        "virtual_env": str(selected.resolve()),
        "molt_venv": str(selected.resolve()),
        "caller": "retained λ",
        "utf8": 1,
        "encoding": "utf-8",
    }
    assert [json.loads(path.read_text(encoding="utf-8")) for path in outputs] == [
        expected,
        expected,
    ]
    assert drv.run_gate('python -c "raise SystemExit(23)"', repo, env) == 23


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
    repo = tmp_path / "source"
    repo.mkdir()
    _make_gate_venv(repo / ".venv", None)
    env = _gate_caller_env(tmp_path / "hostile")
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
        "Path(sys.argv[1]).write_text(json.dumps({key: os.environ[key] "
        "for key in keys}), encoding='utf-8')\n",
        encoding="utf-8",
    )
    outputs = [tmp_path / "first.json", tmp_path / "second.json"]
    command = " && ".join(
        _gate_shell_command("python", str(script), str(output), wrapper)
        for output in outputs
    )
    before = dict(env)
    assert drv.run_gate(command, repo, env) == 0
    assert env == before
    first, second = [json.loads(path.read_text(encoding="utf-8")) for path in outputs]
    assert first == second
    assert first[wrapper] == env[wrapper]
    assert first["SCCACHE_SERVER_PORT"] == "49971"
    assert first["CARGO_INCREMENTAL"] == "0"
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
    with pytest.raises(drv.DriverError, match="virtualenv Python not found"):
        drv.run_gate("echo should-not-run > launched.txt", repo, env)
    assert not (repo / "launched.txt").exists()
    assert env == before


def test_run_gate_invalid_worktree_venv_does_not_fall_back_to_main(drv, tmp_path):
    main = tmp_path / "main"
    _init_repo(main)
    _commit_file(main, "README.md", "fixture\n", "fixture")
    _make_gate_venv(main / ".venv", "main must not run")
    work = tmp_path / "work"
    _git(main, "worktree", "add", "-q", "-b", "gate-test", str(work))
    (work / ".venv").mkdir()
    with pytest.raises(drv.DriverError, match="virtualenv Python not found"):
        drv.run_gate(
            "echo should-not-run > launched.txt", work, _gate_caller_env(main / ".venv")
        )
    assert not (work / "launched.txt").exists()


def test_run_gate_missing_project_venv_refuses_python_but_allows_generic_shell(
    drv, tmp_path
):
    env = _gate_caller_env(tmp_path / "absent")
    with pytest.raises(drv.DriverError, match="virtualenv Python not found"):
        drv.run_gate('python -c "raise SystemExit(0)"', tmp_path, env)
    assert drv.run_gate("echo generic > observed.txt", tmp_path, env) == 0
    assert (tmp_path / "observed.txt").read_text(encoding="utf-8").strip() == "generic"
    if os.name != "nt":
        # POSIX sh reports unmatched quotes; cmd.exe accepts that spelling.
        assert drv.run_gate('echo "unterminated', tmp_path, env) != 0


def test_run_gate_missing_selected_python_alias_never_uses_host_path(drv, tmp_path):
    selected = tmp_path / ".venv"
    _make_gate_venv(selected, "selected")
    alias = selected / ("Scripts/python3.exe" if os.name == "nt" else "bin/python3")
    alias.unlink(missing_ok=True)
    env = _gate_caller_env(tmp_path / "unused")
    with pytest.raises(
        drv.DriverError, match="virtualenv command 'python3' not executable"
    ):
        drv.run_gate('python3 -c "raise SystemExit(0)"', tmp_path, env)


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
    with pytest.raises(drv.DriverError, match="virtualenv Python not found"):
        drv.run_gate(
            f'{head} -c "raise SystemExit(0)"',
            tmp_path,
            _gate_caller_env(tmp_path / "absent"),
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


def test_committed_gate_manifest_is_valid(drv):
    # The shipped manifest must parse and carry rules + always-gates.
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    assert cfg.always, "the committed manifest must define `always` gates"
    assert cfg.rules, "the committed manifest must define rules"
    names = {r.name for r in cfg.rules}
    # The dev-tooling self-gate must exist (the tool gates its own changes).
    assert "dev-tooling" in names
    assert "backend-native" in names
    assert "backend-llvm" in names
    assert "tir-midend" in names
    assert "wasm-host" in names


def test_selected_rules_run_their_pytest_targets_in_one_invocation(drv, tmp_path):
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
    cfg = drv.GateConfig.parse(source, "merged pytest fixture", root=tmp_path)
    gates, matched = cfg.select(["src/a/x.py", "src/b/y.py"])
    gates = [gate.command for gate in gates]
    assert [rule.name for rule in matched] == ["first", "second"]
    # One pytest run at the first contributing rule's place, each target once.
    assert gates == [
        "python tools/check_a.py",
        "python tools/check_first.py",
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
        "cargo test -p b",
    ]


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
    cfg = drv.GateConfig.parse(source, "pytest selection fixture", root=tmp_path)
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
        drv.GateConfig.parse(source, "invalid batch", root=tmp_path)


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

    command = local_pytest_argv(targets, memory_units=PytestMemoryUnits(()))
    observed = []

    def guarded(argv, **kwargs):
        observed.append(argv)
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(
        molt_dev_common.harness_memory_guard, "guarded_completed_process", guarded
    )
    assert molt_dev_common._run_live_gate(command, repo=tmp_path, env={}) == 0
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


def test_landing_pytest_inherits_actual_larger_owning_units(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, _rules = cfg.select(
        [
            "src/molt/cli/backend_compile.py",
            "runtime/molt-runtime/src/builtins/exceptions.rs",
        ]
    )
    gates = [gate.command for gate in gates]
    import_run = next(
        gate
        for gate in gates
        if isinstance(gate, tuple) and "tests/cli/test_cli_import_collection.py" in gate
    )
    assert "--molt-worker-memory-gb=6.25" in import_run
    assert import_run[import_run.index("-k") + 1].startswith("backend_codegen or ")
    merged = next(
        gate
        for gate in gates
        if isinstance(gate, tuple) and "tests/test_table_drift_gate.py" in gate
    )
    # Backend preparation consumes admitted frontend digest seeds, so its joined
    # assembly proof includes CLI controls and inherits that cell's declared unit.
    # Compiled heavy-regression cells remain separate below.
    assert "--molt-worker-memory-gb=6.25" in merged
    assert "tests/test_resource_enforcement_e2e.py" not in merged
    assert "tests/test_wasm_runtime_heavy_regressions.py" not in merged
    assert "-k" not in merged and "-m" not in merged[3:]


@pytest.mark.parametrize(
    "unit", ["0", "-1", "nan", "inf", "1e-100", "1e308", "invalid", None]
)
def test_gate_driver_rejects_malformed_declared_pytest_unit_before_admission(drv, unit):
    argv = ["python", "-m", "pytest", "tests/test_x.py", "--molt-worker-memory-gb"]
    if unit is not None:
        argv.append(unit)
    source = "[[command]]\nargv = " + json.dumps(argv)
    with pytest.raises(drv.DriverError, match="invalid local gate policy"):
        drv.GateConfig.parse(source, "invalid worker unit")


def _gate_args(gate):
    return shlex.split(gate) if isinstance(gate, str) else gate


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
    assert drv.run_gate(command, repo, _gate_caller_env(tmp_path / "absent")) == 0
    assert json.loads(observed.read_text(encoding="utf-8")) == [
        "selected",
        str(selected),
        literal,
    ]
    with pytest.raises(drv.DriverError, match="virtualenv Python not found"):
        drv.run_gate(command, tmp_path, _gate_caller_env(tmp_path / "absent"))


@pytest.mark.parametrize(
    "path",
    [
        "tools/proof_plan.toml",
        "tools/proof_plan.py",
        "tools/proof_receipts.py",
        "tests/test_proof_plan.py",
        "tools/proof_queue_pkg/command_admission.py",
        "tools/proof_queue_pkg/command_identity.py",
        "tools/proof_queue_pkg/execution_environment.py",
        "tools/proof_queue_pkg/target_derived_toolchains.py",
        "tools/proof_queue_pkg/toolchain_capture.py",
        "tools/proof_queue_pkg/process_image_capture.py",
        "tools/proof_queue_pkg/python_custody_bootstrap.py",
        "tools/proof_queue_pkg/cli.py",
        "tools/proof_queue_pkg/pact.py",
        "tests/tools/test_source_extension_proof_family.py",
        "tests/tools/test_target_derived_toolchains.py",
        "tests/tools/test_python_custody_bootstrap.py",
    ],
)
def test_proof_policy_edits_select_receivers_without_package_production(drv, path):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([path])
    gates = [gate.command for gate in gates]
    names = {rule.name for rule in matched}
    assert "source-extension-proof-policy" in names
    assert "source-extension-package-sets" not in names
    arguments = [_gate_args(gate) for gate in gates]
    selected = {argument for argv in arguments for argument in argv}
    # These mixed consumers include real proof-environment and Pact-oracle
    # reads; dropping all source-extension tests would miss those boundaries.
    assert {
        "tests/cli/test_source_extension_link_inputs.py",
        "tests/tools/test_source_extension_proof_family.py",
        "tests/tools/test_target_derived_toolchains.py",
        "tests/tools/test_python_custody_bootstrap.py",
        "tests/tools/test_scientific_stack_versions.py",
    } <= selected
    assert "tests/cli/test_source_extension_producer.py" not in selected
    assert "tests/cli/test_source_extension_compiler_inputs.py" not in selected
    queue = next(
        argv for argv in arguments if "tests/tools/test_proof_queue.py" in argv
    )
    assert set(queue[queue.index("-k") + 1].split(" or ")) == {
        "proof_queue_pact_witness",
        "proof_queue_rejects_scipy",
        "proof_queue_requires_explicit_scipy",
        "rejects_expected_extension_identity_drift",
    }


@pytest.mark.parametrize(
    "path",
    [
        "config/source_extension_package_sets.toml",
        "config/scientific_stack_versions.toml",
        "src/molt/cli/source_extension_set_registry.py",
        "src/molt/cli/source_extension_invocation.py",
        "src/molt/cli/source_extension_producer.py",
        "src/molt/cli/source_build_environment.py",
        "src/molt/cli/source_build_environment_schema.py",
        "src/molt/source_extension_link_inputs.py",
        "tests/cli/test_source_build_environment_probe.py",
        "tests/tools/test_scientific_stack_versions.py",
    ],
)
def test_producer_edits_retain_package_and_proof_boundary_checks(drv, path):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([path])
    gates = [gate.command for gate in gates]
    names = {rule.name for rule in matched}
    assert {"source-extension-package-sets", "source-extension-proof-policy"} <= names
    selected = {argument for gate in gates for argument in _gate_args(gate)}
    assert {
        "tests/cli/test_source_extension_producer.py",
        "tests/cli/test_source_extension_invocation.py",
        "tests/cli/test_source_extension_compiler_inputs.py",
        "tests/cli/test_source_extension_link_inputs.py",
        "tests/tools/test_source_extension_proof_family.py",
        "tests/tools/test_proof_queue.py",
    } <= selected
    assert len(gates) == len(set(gates))


def test_mixed_rust_python_change_checks_rust_before_consumer_cohorts(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    selected, _ = cfg.select(
        [
            "runtime/molt-backend/src/backend_process/input/mod.rs",
            "src/molt/cli/backend_execution.py",
        ]
    )
    commands = [gate.command for gate in selected]
    rust = next(
        i
        for i, command in enumerate(commands)
        if "tools/check_changed_clippy.py" in command
    )
    consumers = [
        i
        for i, command in enumerate(commands)
        if isinstance(command, tuple) and "pytest" in _gate_args(command)
    ]
    assert consumers and all(rust < index for index in consumers)


def test_every_producer_trigger_also_selects_its_proof_boundary(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    rules = {rule.name: rule for rule in cfg.rules}
    # Both actions depend on these inputs. A new producer trigger must not
    # silently leave its proof-admission consumer behind.
    assert set(rules["source-extension-package-sets"].globs) <= set(
        rules["source-extension-proof-policy"].globs
    )


@pytest.mark.parametrize(
    "path",
    [
        "runtime/molt-ir/src/tir/ops.rs",
        "runtime/molt-passes/src/tir/passes/effects.rs",
        "runtime/molt-passes/src/tir/lower_to_simple.rs",
        "runtime/molt-tir/src/representation_plan.rs",
        "runtime/molt-backend-wasm/src/wasm/final_functions.rs",
        "runtime/molt-backend-wasm/Cargo.toml",
    ],
)
def test_committed_gate_manifest_selects_tir_midend_ratchet(drv, path):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([path])
    gates = [gate.command for gate in gates]
    if not path.startswith("runtime/molt-backend-wasm/"):
        assert [r.name for r in matched] == [
            "rust-clippy-changed",
            "tir-midend",
        ]
    rule = next(rule for rule in matched if rule.name == "tir-midend")
    assert set(rule.gates) <= set(gates)
    cargo = [shlex.split(gate) for gate in rule.gates]
    assert len(cargo) == 2
    assert [command[1] for command in cargo] == ["clippy", "test"]
    for command in cargo:
        packages = {command[i + 1] for i, token in enumerate(command) if token == "-p"}
        assert packages == {"molt-ir", "molt-passes", "molt-tir", "molt-backend-wasm"}
        assert "--locked" in command and "--all-features" in command
        assert command[command.index("--profile") + 1] == "dev-fast"
    assert "--all-targets" in cargo[0] and cargo[0][-3:] == ["--", "-D", "warnings"]
    assert "--no-fail-fast" in cargo[1]


@pytest.mark.parametrize(
    "path",
    [
        "runtime/molt-backend-native/src/lib.rs",
        "runtime/molt-backend-native/src/native_backend/function_compiler.rs",
        "runtime/molt-backend-native/src/simple_backend/module_metadata.rs",
        "runtime/molt-backend-native/Cargo.toml",
        "runtime/molt-backend/src/lib.rs",
    ],
)
def test_committed_native_policy_includes_owning_crate(drv, path):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([path])
    gates = [gate.command for gate in gates]
    rule = next(rule for rule in matched if rule.name == "backend-native")
    (tests,) = [shlex.split(gate) for gate in rule.gates]
    packages = {tests[i + 1] for i, token in enumerate(tests) if token == "-p"}
    assert packages == {
        "molt-backend",
        "molt-backend-native",
        "molt-artifact-publish",
    }
    assert "--locked" in tests
    assert set(tests[tests.index("--features") + 1].split(",")) == {
        "molt-backend/native-backend",
        "molt-backend/wasm-backend",
    }
    assert "--test" not in tests
    assert (
        "python tools/check_changed_clippy.py --base {integration_upstream_commit}"
        in gates
    )
    assert tests[1] == "test" and "--lib" in tests and "--no-fail-fast" in tests
    assert tests[tests.index("--profile") + 1] == "release-fast"


@pytest.mark.parametrize(
    "path",
    [
        "runtime/molt-wasm-facts/src/publication.rs",
        "runtime/molt-wasm-facts/Cargo.toml",
        "tools/wasm_link_fact_provider.py",
    ],
)
def test_committed_facts_policy_excludes_compiler_codegen(drv, path):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([path])
    gates = [gate.command for gate in gates]
    assert "backend-native" not in {rule.name for rule in matched}
    rule = next(rule for rule in matched if rule.name == "wasm-facts-host-tool")
    tests, provider = [shlex.split(gate) for gate in rule.gates]
    assert {tests[i + 1] for i, token in enumerate(tests) if token == "-p"} == {
        "molt-wasm-facts",
        "molt-artifact-publish",
        "molt-host-process",
    }
    assert "--locked" in tests
    assert tests[tests.index("--features") + 1] == "molt-wasm-facts/host-tool"
    assert tests[tests.index("--test") + 1] == "wasm_link_facts_cli"
    assert provider == [
        "python",
        "-m",
        "pytest",
        "-q",
        "tests/test_wasm_link_validation.py",
        "-k",
        "rust_facts or wasm_artifact_state",
    ]
    # Rust edits select changed-crate lint cells; the Python bridge reuses the
    # Rust image and exercises its consumers without another lint campaign.
    assert (
        "python tools/check_changed_clippy.py --base {integration_upstream_commit}"
        in gates
    ) == path.startswith("runtime/")
    assert tests[1] == "test" and "--lib" in tests and "--no-fail-fast" in tests
    assert tests[tests.index("--profile") + 1] == "release-fast"


def test_committed_source_checks_precede_complete_native_build_selection(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(
        [
            "runtime/molt-backend-native/src/native_backend/simple_backend/config.rs",
            "runtime/molt-ir/src/tir/op_kinds.toml",
            "tools/proof_plan.toml",
            "config/source_extension_package_sets.toml",
        ]
    )
    gates = [gate.command for gate in gates]
    # These independent families must survive reordering, including every
    # native build owner and its complete Cargo test/build requirements.
    source_names = {
        "op-kind-registry",
        "source-extension-package-sets",
        "source-extension-proof-policy",
        "dev-tooling",
        "ci-wiring",
    }
    build_names = ["backend-native", "native-object-emission", "tir-midend"]
    lint_gate = (
        "python tools/check_changed_clippy.py --base {integration_upstream_commit}"
    )
    assert {rule.name for rule in matched} == source_names | set(build_names) | {
        "rust-clippy-changed"
    }
    source_rules = [rule for rule in matched if rule.name in source_names]
    build_rules = [rule for rule in matched if rule.name in build_names]
    assert [rule.name for rule in build_rules] == build_names
    source_gates = {gate for rule in source_rules for gate in rule.gates}
    build_gates = [gate for rule in build_rules for gate in rule.gates]
    assert source_gates <= set(gates)
    assert set(build_gates) <= set(gates)
    assert gates[: len(cfg.always)] == cfg.always
    # Clippy fails a lint after the source checks and before any build.
    assert max(gates.index(gate) for gate in source_gates) < gates.index(lint_gate)
    assert gates.index(lint_gate) < min(gates.index(gate) for gate in build_gates)
    assert gates[-len(build_gates) :] == build_gates
    assert len(gates) == len(set(gates))


def test_committed_gate_manifest_selects_molt_ir_op_kind_registry(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(["tools/op_kinds/render_rust.py"])
    gates = [gate.command for gate in gates]

    # Every host script change also proves its import identity.
    assert [r.name for r in matched] == ["op-kind-registry", "host-entrypoint-imports"]
    assert gates.count("python tools/generators.py check") == 1
    assert "python tools/gen_op_kinds.py --check" not in gates
    assert "python tools/audit_op_kinds.py --check" in gates

    gates, matched = cfg.select(["runtime/molt-ir/src/tir/op_kinds.toml"])
    gates = [gate.command for gate in gates]
    assert [r.name for r in matched] == [
        "op-kind-registry",
        "rust-clippy-changed",
        "tir-midend",
    ]
    assert gates.count("python tools/generators.py check") == 1
    assert "python tools/gen_op_kinds.py --check" not in gates
    assert "python tools/audit_op_kinds.py --check" in gates


def test_committed_gate_manifest_selects_backend_native_llvm(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(
        [
            "runtime/molt-backend-native/src/llvm_backend/lowering/preserved_ops/direct_ops.rs"
        ]
    )
    gates = [gate.command for gate in gates]

    assert [r.name for r in matched] == [
        "op-kind-registry",
        "rust-clippy-changed",
        "backend-llvm",
    ]
    assert (
        "cargo test --locked --profile release-fast -p molt-backend-native"
        " -p molt-backend --features molt-backend/native-backend,molt-backend/llvm"
        " --lib --test ir_contract_validation --no-fail-fast" in gates
    )
    assert "python tools/audit_op_kinds.py --check" in gates


def test_committed_gate_manifest_selects_codec_table_registry(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(["runtime/molt-stdlib-text/src/codec_registry.rs"])
    gates = [gate.command for gate in gates]

    assert [r.name for r in matched] == [
        "codec-tables",
        "rust-clippy-changed",
    ]
    assert gates.count("python tools/generators.py check") == 1
    assert "python tools/gen_codecs.py --check" not in gates
    merged = next(gate for gate in gates if isinstance(gate, tuple))
    assert "tests/test_gen_codecs.py" in merged

    gates, matched = cfg.select(["src/molt/stdlib/encodings/cp1252.py"])
    gates = [gate.command for gate in gates]
    assert "codec-tables" in {r.name for r in matched}
    assert gates.count("python tools/generators.py check") == 1
    assert "python tools/gen_codecs.py --check" not in gates

    gates, matched = cfg.select(
        ["runtime/molt-stdlib-text/src/codec_aliases_generated.rs"]
    )
    gates = [gate.command for gate in gates]
    assert [r.name for r in matched] == [
        "codec-tables",
        "rust-clippy-changed",
    ]
    assert gates.count("python tools/generators.py check") == 1
    assert "python tools/gen_codecs.py --check" not in gates

    gates, matched = cfg.select(
        ["runtime/molt-stdlib-text/src/charmap_codecs_generated.rs"]
    )
    gates = [gate.command for gate in gates]
    assert [r.name for r in matched] == [
        "codec-tables",
        "rust-clippy-changed",
    ]
    assert gates.count("python tools/generators.py check") == 1
    assert "python tools/gen_codecs.py --check" not in gates

    gates, matched = cfg.select(["runtime/molt-runtime/src/object/ops_encoding.rs"])
    gates = [gate.command for gate in gates]
    assert "codec-tables" in {r.name for r in matched}
    assert gates.count("python tools/generators.py check") == 1
    assert "python tools/gen_codecs.py --check" not in gates
    merged = next(gate for gate in gates if isinstance(gate, tuple))
    assert "tests/test_gen_codecs.py" in merged


def test_committed_gate_manifest_selects_wasm_host_ratchet(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(["runtime/molt-wasm-host/src/main.rs"])
    gates = [gate.command for gate in gates]

    assert [r.name for r in matched] == [
        "rust-clippy-changed",
        "wasm-host",
    ]
    # Clippy for every reached crate belongs to the changed-crate gate.
    assert (
        "python tools/check_changed_clippy.py --base {integration_upstream_commit}"
        in gates
    )
    assert "cargo build --locked --profile release-fast -p molt-wasm-host" in gates


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
    cfg = drv.GateConfig.load(_write_gates(tmp_path))
    gates, matched = cfg.select(["runtime/molt-backend/src/lib.rs"])
    gates = [gate.command for gate in gates]
    assert "echo always-ran" in gates
    assert "echo backend-gate" in gates
    assert "echo python-gate" not in gates
    assert [r.name for r in matched] == ["backend-native"]


def test_gate_manifest_selection_python(drv, tmp_path):
    cfg = drv.GateConfig.load(_write_gates(tmp_path))
    gates, matched = cfg.select(["src/molt/frontend.py"])
    gates = [gate.command for gate in gates]
    assert "echo python-gate" in gates
    assert "echo backend-gate" not in gates
    assert [r.name for r in matched] == ["frontend-python"]


def test_gate_manifest_always_runs(drv, tmp_path):
    # An unmatched change-class still runs `always` gates, nothing else.
    cfg = drv.GateConfig.load(_write_gates(tmp_path))
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
    cfg = drv.GateConfig.load(cfg_path)
    gates, _ = cfg.select(["x/1", "y/2"])
    gates = [gate.command for gate in gates]
    # 'echo shared' appears once, preserving first-seen order.
    assert gates == ["echo shared", "echo a-only", "echo b-only"]


def test_gate_manifest_rejects_bad_toml(drv, tmp_path):
    bad = tmp_path / "bad.toml"
    bad.write_text("[[rule]]\nname = 5\nglobs = []\ngates = []\n", encoding="utf-8")
    with pytest.raises(drv.DriverError):
        drv.GateConfig.load(bad)


def test_gate_manifest_missing_is_usage_error(drv, tmp_path):
    with pytest.raises(drv.DriverError) as exc:
        drv.GateConfig.load(tmp_path / "does_not_exist.toml")
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


def _worktree_off(origin: Path, base: Path, name: str) -> Path:
    """Add a linked worktree off origin/main so cleanup can `worktree remove` it."""
    main_clone = base / "mainclone"
    if not main_clone.exists():
        _git(base, "clone", "-q", str(origin), str(main_clone))
        _git(main_clone, "config", "user.email", "d@molt.test")
        _git(main_clone, "config", "user.name", "D")
    wt = base / name
    _git(main_clone, "worktree", "add", "-q", "--detach", str(wt), "origin/main")
    _git(wt, "config", "user.email", "d@molt.test")
    _git(wt, "config", "user.name", "D")
    return wt


def test_cleanup_allows_clean_pushed(drv, origin_and_clone, tmp_path, monkeypatch):
    from tools import molt_dev_common

    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_clean")
    main = tmp_path / "mainclone"
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
    rc = drv._cleanup_worktree(
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

    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_daemons")
    retired: list[tuple[Path, bool]] = []

    def retire(project_root, env, **kwargs):
        retired.append((project_root, project_root.exists()))
        return (4321,)

    monkeypatch.setattr(backend_execution, "retire_project_backend_daemons", retire)
    rc = drv._cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_OK
    assert retired == [(wt, True)], "daemons retire while their checkout still exists"
    assert not wt.exists()


def test_cleanup_refuses_unpushed(drv, origin_and_clone, tmp_path):
    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_unpushed")
    _commit_file(wt, "local.py", "n = 1\n", "feat: unpushed local")
    git = drv.Git(wt)
    rc = drv._cleanup_worktree(
        git, wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_FAIL
    assert wt.exists()  # NOT removed


def test_cleanup_refuses_dirty_tracked(drv, origin_and_clone, tmp_path):
    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_dirty")
    # Modify a TRACKED file without committing -> dirty -> refuse.
    (wt / "README.md").write_text("seed\nlocal edit\n", encoding="utf-8")
    rc = drv._cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_FAIL
    assert wt.exists()


def test_molt_dev_default_ignore_globs_use_dirty_tree_policy_authority(drv):
    assert drv.DEFAULT_IGNORE_GLOBS is dirty_tree_policy.DEFAULT_DIRTY_TREE_IGNORE_GLOBS


def test_cleanup_refuses_runtime_generation_churn(drv, origin_and_clone, tmp_path):
    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_wasm")
    generation = wt / RUNTIME_GENERATION_MANIFEST
    generation.parent.mkdir(parents=True, exist_ok=True)
    generation.write_text("{}\n", encoding="utf-8")
    _git(wt, "add", "--", RUNTIME_GENERATION_MANIFEST)
    rc = drv._cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_FAIL
    assert wt.exists()


def test_cleanup_force_requires_matching_sha(drv, origin_and_clone, tmp_path):
    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_force")
    _commit_file(wt, "local.py", "n = 1\n", "feat: unpushed")
    git = drv.Git(wt)
    head = git.head_sha()
    # Wrong force sha -> refused.
    rc_wrong = drv._cleanup_worktree(
        git, wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha="0" * 40
    )
    assert rc_wrong == drv.EXIT_FAIL
    assert wt.exists()
    # Correct force sha (the real HEAD) -> abandon proceeds.
    rc_right = drv._cleanup_worktree(
        git, wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=head
    )
    assert rc_right == drv.EXIT_OK
    assert not wt.exists()


def test_cleanup_refuses_main_worktree(drv, origin_and_clone, tmp_path):
    # Targeting the MAIN worktree must be refused (this is for disposable ones).
    origin, _work = origin_and_clone
    main_clone = tmp_path / "main_only"
    _git(tmp_path, "clone", "-q", str(origin), str(main_clone))
    _git(main_clone, "config", "user.email", "d@molt.test")
    _git(main_clone, "config", "user.name", "D")
    rc = drv._cleanup_worktree(
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
    target = drv.python_interpreter.parse_target_python_version(mm)
    resolved = drv.python_interpreter.resolve_target_python(
        target, prefer_uv=False, prefer_current=True
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
    resolved = drv.python_interpreter.probe_python_command([sys.executable])
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
            "--no-push",
            "--marker",
            "contains:impl.py::NONEXISTENT_TOKEN",
        ]
    )
    assert rc == drv.EXIT_FAIL


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
    drv, origin_and_clone, tmp_path
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
    ns = _integrate_ns(
        drv,
        repo=str(work),
        no_push=True,
    )
    # A failing gate halts integrate LOUDLY (DriverError) BEFORE push.
    with pytest.raises(drv.DriverError) as exc:
        drv.cmd_integrate(ns)
    assert re.search(r"gate failed \(exit 3\) after \d+\.\d s: exit 3", str(exc.value))
    assert re.search(r"gates: 1 in \d+\.\d s; slowest:", str(exc.value))
    assert str(exc.value).count("exit 3") == 3  # status, failed command, timing row
    assert not (work / "build-started.txt").exists()
    # Nothing was pushed (origin unchanged).
    origin_main = _git(work, "rev-parse", "origin/main").stdout.strip()
    seed = _git(work, "rev-list", "--max-parents=0", "origin/main").stdout.strip()
    assert origin_main == seed  # still just the seed commit


def test_integrate_gates_never_write_bytecode_into_the_checkout(
    drv, origin_and_clone, monkeypatch
):
    # Bytecode a gate writes under src/ changes the checkout's import topology
    # while later gates read it: a fresh worktree's first landing failed with
    # "Python import topology changed during operation: <worktree>/src/molt".
    # The caller's shell may already set the policy; integrate must not rely on it.
    monkeypatch.delenv("PYTHONDONTWRITEBYTECODE", raising=False)
    origin, work = origin_and_clone
    _commit_file(work, "src/probe_import.py", "VALUE = 1\n", "feat: probe")
    _commit_file(
        work,
        "tools/proof_plan.toml",
        """
always = []

[[rule]]
name = "imports-source"
globs = ["src/*.py"]
gates = ["env python3 -c \\"import sys; sys.path.insert(0, 'src'); import probe_import\\""]
""",
        "a gate that imports checkout source",
    )
    ns = _integrate_ns(drv, repo=str(work), no_push=True)
    drv.cmd_integrate(ns)
    assert not list((work / "src").rglob("__pycache__"))


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
def exclusive(args):
    Path(sys.argv[2]).write_text("entered", encoding="utf-8")
    return 73
molt_dev._integrate_exclusive = exclusive
try:
    code = molt_dev.cmd_integrate(argparse.Namespace(repo=sys.argv[1], dry_run=False))
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
            env=env,
            timeout=30,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        return json.loads(result.stdout)

    def exclusive(args):
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


@pytest.mark.parametrize("outcome", ["success", "driver-error", "interrupt"])
def test_integrate_lock_released_on_every_exclusive_exit(
    drv, origin_and_clone, monkeypatch, outcome
):
    from molt import file_locks

    _, work = origin_and_clone
    lock_path = work / ".git" / "molt-integrate.lock"
    held_identity = None

    def exclusive(args):
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
    monkeypatch.setattr(drv, "run_gate", gate)
    # Cleanup's own destructive/dirty-tree contract has separate real-worktree
    # tests above; this seam asserts the integration lock remains held at entry.
    monkeypatch.setattr(drv, "_cleanup_worktree", cleanup)
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
        targets.append((env["CARGO_TARGET_DIR"], dict(env)))
        return 0

    monkeypatch.setattr(drv, "run_gate", gate)
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
    import argparse

    return argparse.Namespace(
        name=name,
        state_dir=str(state_root),
        cwd=cwd,
        env=env,
        replace=replace,
        verify_min_age_hint=30,
        json=False,
        command=["--", *command],
    )


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


def test_detached_run_completes_and_records_rc(drv, tmp_path, capsys):
    state_root = tmp_path / "detached"
    rc = drv.cmd_detached_run(
        _dr_ns(
            drv,
            "ok-run",
            state_root,
            [sys.executable, "-c", "print('MARKER-OUT', flush=True)"],
        )
    )
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
    drv, origin_and_clone, remote, attributed, capfd
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
    imports = (
        "import sys, subprocess; from pathlib import Path; "
        f"sys.path[:0] = [{str(REPO_ROOT)!r}, {str(REPO_ROOT / 'src')!r}]; "
    )
    selection_gate = (
        _integration_python(
            imports
            + "from tools import check_changed_clippy as clippy, check_rustfmt as rustfmt; "
            + "clippy.ROOT = rustfmt.ROOT = Path.cwd(); "
            # Mutate the selected tracking ref after fetch. A ref spelling passed
            # instead of the captured commit would silently select no changes.
            + f"subprocess.run(['git', 'update-ref', 'refs/remotes/{remote}/main', 'HEAD'], check=True); "
            + "assert clippy.changed_paths('origin/main') == (); "
            + "assert rustfmt._changed_rust_paths('origin/main') == (); "
            + "assert set(clippy.changed_paths(sys.argv[1])) == {'feature.rs', 'tools/proof_plan.toml'}; "
            + "assert rustfmt._changed_rust_paths(sys.argv[1]) == (Path('feature.rs'),); "
            + f"assert sys.argv[1] == {expected!r}; "
            + "print('selected-upstream-path-gates-passed')"
        )
        + f" {upstream}"
    )
    attribution_gate = (
        _integration_python(
            imports
            + "from tools import check_commit_attribution as attribution; "
            + "assert attribution.check_revisions(['origin/main..HEAD'], cwd=Path.cwd()) == []; "
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
    assert "selected-upstream-path-gates-passed" in output.out
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
        drv.GateConfig, "admit", lambda *args: pytest.fail("invalid policy admitted")
    )
    monkeypatch.setattr(drv, "run_gate", lambda *args: pytest.fail("invalid gate ran"))
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
def test_integrate_refuses_gate_source_or_head_drift(drv, origin_and_clone, mutation):
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
    assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work))) == drv.EXIT_OK
    selected = _git(work, "rev-parse", "HEAD").stdout.strip()
    assert selected != source
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
        env=env,
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

    def probe(policy):
        if policy.name == "one":
            raise OSError("first probe raised")
        return None

    monkeypatch.setattr(proof_plan, "_version_fingerprint", probe)
    monkeypatch.setattr(
        drv, "run_gate", lambda *args: pytest.fail("gate ran before admission")
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
    monkeypatch.setattr(drv.GateConfig, "admit", lambda *args: None)
    monkeypatch.setattr(
        drv.GateConfig,
        "environments",
        lambda self, gates, *, baseline: {
            gate.toolchains: dict(baseline) for gate in gates
        },
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
        drv, "run_gate", lambda *args: pytest.fail("expensive prefix started")
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

    monkeypatch.setattr(drv, "run_gate", gate)
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
    monkeypatch.setattr(drv, "run_gate", forbidden if dry else lambda *args: 0)
    assert (
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True, dry_run=dry))
        == 0
    )


def test_transitive_cargo_metadata_availability_adds_no_build_requirement(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, _ = cfg.select(["docs/agent/V1_HANDOFF_FINDINGS.md"])
    # This is the real ledger-only policy, including its generator dependency.
    assert ("python", "git", "generators") in [gate.toolchains for gate in gates]
    environments = {gate.toolchains: {} for gate in gates}
    assert cfg.cargo_outputs(environments) == {}


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
    monkeypatch.setattr(drv, "run_gate", gate)
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

    def discover(name, root, *, environ, cwd):
        assert cwd is None
        assert name == "cat"
        assert root == work
        observed_roots.append(root)
        return SimpleNamespace(executable=executable)

    monkeypatch.setattr(tool_releases, "discover_pinned_tool", discover)
    # Admission updates this process; retain fixture cleanup for that real write.
    monkeypatch.setenv("PATH", os.environ["PATH"])
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
    assert observed_roots == [work, work]
    assert os.environ["PATH"].split(os.pathsep)[0] == str(provider)


def test_selected_local_gate_does_not_materialize_unrelated_policy(
    drv, tmp_path, monkeypatch
):
    from tools import proof_plan

    cfg = drv.GateConfig.parse(
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
        proof_plan, "_version_fingerprint", lambda _: pytest.fail("unrelated probe ran")
    )
    assert cfg.admit(["src/one.py"], matched, []) is None
    with pytest.raises(drv.DriverError, match="lack declared prerequisite custody"):
        cfg.admit(["src/one.py"], matched, ["unknown executable"])


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
        env={
            **os.environ,
            "PYTHONPATH": os.pathsep.join((str(REPO_ROOT), str(REPO_ROOT / "src"))),
            "PYTEST_ADDOPTS": "",
        },
        timeout=60,
    )
    assert result.returncode == 0, result.stdout + result.stderr


def test_integrate_uses_selected_committed_target_catalog_without_import_rebinding(
    drv, origin_and_clone, monkeypatch
):
    from molt import target_python
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
        "src/molt/target_python.py",
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
    before_catalog = target_python.SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
    observed = []

    def probe(policy):
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
        "run_gate",
        lambda *args: pytest.fail("gate began before selected catalog admission"),
    )
    with pytest.raises(drv.DriverError) as caught:
        drv.cmd_integrate(_integrate_ns(drv, repo=str(work), no_push=True))
    assert set(observed) == {"uv", "target-pythons.3.12", "target-pythons.3.15"}
    assert "local.always.0" in str(caught.value)
    assert "3.15" in str(caught.value) and "executed=0" in str(caught.value)
    assert sys.path == before_path
    assert sys.modules["molt.target_python"] is target_python
    assert target_python.SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS is before_catalog


def test_gate_selection_preserves_tools_across_batching_dedup_and_extras(drv, tmp_path):
    (tmp_path / "tests").mkdir()
    for name in ("a", "b"):
        (tmp_path / "tests" / f"test_{name}.py").write_text("", encoding="utf-8")
    cfg = drv.GateConfig.parse(
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
name = "unmatched"
globs = ["other/**"]
gates = ["explicit-extra"]
pytest = ["tests/test_b.py"]
toolchains = ["extra"]
""",
        "tool custody fixture",
        root=tmp_path,
    )
    gates, _ = cfg.select(["src/a.py"], extra=["explicit-extra", "shared"])
    assert [gate.command for gate in gates if isinstance(gate.command, str)] == [
        "shared",
        "sdk-command",
        "explicit-extra",
    ]
    assert next(gate.toolchains for gate in gates if gate.command == "shared") == (
        "base",
        "first",
        "second",
    )
    assert next(
        gate.toolchains for gate in gates if gate.command == "explicit-extra"
    ) == ("extra",)
    batched = next(
        gate
        for gate in gates
        if isinstance(gate.command, tuple) and "-k" not in gate.command
    )
    assert batched.toolchains == ("first", "second")
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
    drv, origin_and_clone, tmp_path, monkeypatch, explicit_sdk_flags
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
        lambda policy: {
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
