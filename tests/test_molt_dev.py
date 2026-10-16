"""Failure-mode tests for the canonical integration driver (tools/molt_dev.py).

EVERY hazard countermeasure is proven by a test that FIRES the countermeasure
on a synthetic git repo built in tmp (the tests/test_check_suite_honesty.py
style: green-path test + a failure-mode test per guard direction). The repos
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
  5 diff/ls/stat lie            (all verdicts use plumbing/python: implicit in
                                every test; binaries-identical proven directly)
                                test_binaries_identical_helper
  6 stale-binary misattribution test_verify_toolchain_flags_stale_binary
                                test_verify_toolchain_fresh_binary_with_marker
                                test_verify_toolchain_missing_marker_fails
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
import shlex
import os
import sys
import sysconfig
import subprocess
import venv
from pathlib import Path
from tests.process_guard_common import install_module_os_view

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


@pytest.fixture
def origin_and_clone(tmp_path):
    """A bare `origin` plus a working clone tracking origin/main with one commit.

    Returns (origin_bare, work). The work clone is the "agent worktree" the
    driver acts on; origin is the upstream the push lands in.
    """
    origin = tmp_path / "origin.git"

    seed = tmp_path / "seed"
    _init_repo(seed)
    (seed / "tools").mkdir()
    (seed / "tools/proof_plan.toml").write_text(
        'always = ["echo integration-policy"]\n', encoding="utf-8"
    )
    _git(seed, "add", "tools/proof_plan.toml")
    _commit_file(seed, "README.md", "seed\n", "seed: initial commit")
    _git(tmp_path, "clone", "--bare", "-q", str(seed), str(origin))

    work = tmp_path / "work"
    _git(tmp_path, "clone", "-q", str(origin), str(work))
    _git(work, "config", "user.email", "dev@molt.test")
    _git(work, "config", "user.name", "Molt Dev")
    return origin, work


def _make_gate_venv(path: Path, token: str | None) -> Path:
    venv.EnvBuilder(with_pip=False).create(path)
    python = path / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    result = _run_fast_captured_command(
        [str(python), "-c", "import sysconfig; print(sysconfig.get_path('purelib'))"],
        cwd=path.parent,
    )
    assert result.returncode == 0, result.stderr
    site = Path(result.stdout.strip())
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


@pytest.mark.parametrize(
    "path",
    [
        "runtime/molt-ir/src/tir/ops.rs",
        "runtime/molt-passes/src/tir/passes/effects.rs",
        "runtime/molt-passes/src/tir/lower_to_simple.rs",
        "runtime/molt-tir/src/representation_plan.rs",
    ],
)
def test_committed_gate_manifest_selects_tir_midend_ratchet(drv, path):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select([path])
    assert [r.name for r in matched] == ["rust-ffi-blocks", "tir-midend"]
    cargo = [shlex.split(gate) for gate in gates if gate.startswith("cargo ")]
    assert len(cargo) == 2
    assert [command[1] for command in cargo] == ["clippy", "test"]
    for command in cargo:
        packages = {command[i + 1] for i, token in enumerate(command) if token == "-p"}
        assert packages == {"molt-ir", "molt-passes", "molt-tir"}
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
    _, matched = cfg.select([path])
    rule = next(rule for rule in matched if rule.name == "backend-native")
    clippy, tests = [shlex.split(gate) for gate in rule.gates]
    for command in (clippy, tests):
        packages = {command[i + 1] for i, token in enumerate(command) if token == "-p"}
        assert packages == {
            "molt-backend",
            "molt-backend-native",
            "molt-artifact-publish",
        }
        assert "--locked" in command
        assert command[command.index("--features") + 1] == "molt-backend/native-backend"
    assert clippy[1] == "clippy" and clippy[-3:] == ["--", "-D", "warnings"]
    assert tests[1] == "test" and "--lib" in tests and "--no-fail-fast" in tests
    assert tests[tests.index("--profile") + 1] == "release-fast"


def test_committed_source_checks_precede_complete_native_build_selection(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(
        [
            "runtime/molt-backend-native/src/native_backend/simple_backend/config.rs",
            "runtime/molt-ir/src/tir/op_kinds.toml",
            "tools/proof_plan.toml",
        ]
    )
    # These independent families must survive reordering, including every
    # native build owner and its requirement for a fresh toolchain.
    source_names = {
        "op-kind-registry",
        "source-extension-package-sets",
        "dev-tooling",
        "rust-ffi-blocks",
        "ci-wiring",
    }
    build_names = ["backend-native", "native-object-emission", "tir-midend"]
    assert {rule.name for rule in matched} == source_names | set(build_names)
    source_rules = [rule for rule in matched if rule.name in source_names]
    build_rules = [rule for rule in matched if rule.name in build_names]
    assert [rule.name for rule in build_rules] == build_names
    assert {rule.name for rule in matched if rule.require_fresh_toolchain} == {
        "backend-native",
        "native-object-emission",
        "tir-midend",
    }
    source_gates = {gate for rule in source_rules for gate in rule.gates}
    build_gates = [gate for rule in build_rules for gate in rule.gates]
    assert source_gates <= set(gates)
    assert set(build_gates) <= set(gates)
    assert gates[: len(cfg.always)] == cfg.always
    assert max(gates.index(gate) for gate in source_gates) < min(
        gates.index(gate) for gate in build_gates
    )
    assert gates[-len(build_gates) :] == build_gates
    assert len(gates) == len(set(gates))


def test_committed_gate_manifest_selects_molt_ir_op_kind_registry(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(["tools/op_kinds/render_rust.py"])

    assert [r.name for r in matched] == ["op-kind-registry"]
    assert "python tools/gen_op_kinds.py --check" in gates
    assert "python tools/audit_op_kinds.py --check" in gates

    gates, matched = cfg.select(["runtime/molt-ir/src/tir/op_kinds.toml"])
    assert [r.name for r in matched] == ["op-kind-registry", "tir-midend"]
    assert "python tools/gen_op_kinds.py --check" in gates
    assert "python tools/audit_op_kinds.py --check" in gates


def test_committed_gate_manifest_selects_backend_native_llvm(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(
        [
            "runtime/molt-backend-native/src/llvm_backend/lowering/preserved_ops/direct_ops.rs"
        ]
    )

    assert [r.name for r in matched] == [
        "op-kind-registry",
        "rust-ffi-blocks",
        "backend-llvm",
    ]
    assert (
        "cargo test --locked --profile release-fast -p molt-backend-native --features llvm --lib"
        in gates
    )
    assert "python tools/audit_op_kinds.py --check" in gates


def test_committed_gate_manifest_selects_codec_table_registry(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(["runtime/molt-stdlib-text/src/codec_registry.rs"])

    assert [r.name for r in matched] == ["codec-tables", "rust-ffi-blocks"]
    assert "python tools/gen_codecs.py --check" in gates
    assert "python -m pytest -q tests/test_gen_codecs.py" in gates

    gates, matched = cfg.select(["src/molt/stdlib/encodings/cp1252.py"])
    assert "codec-tables" in {r.name for r in matched}
    assert "python tools/gen_codecs.py --check" in gates

    gates, matched = cfg.select(
        ["runtime/molt-stdlib-text/src/codec_aliases_generated.rs"]
    )
    assert [r.name for r in matched] == ["codec-tables", "rust-ffi-blocks"]
    assert "python tools/gen_codecs.py --check" in gates

    gates, matched = cfg.select(
        ["runtime/molt-stdlib-text/src/charmap_codecs_generated.rs"]
    )
    assert [r.name for r in matched] == ["codec-tables", "rust-ffi-blocks"]
    assert "python tools/gen_codecs.py --check" in gates

    gates, matched = cfg.select(["runtime/molt-runtime/src/object/ops_encoding.rs"])
    assert "codec-tables" in {r.name for r in matched}
    assert "python tools/gen_codecs.py --check" in gates
    assert "python -m pytest -q tests/test_gen_codecs.py" in gates


def test_committed_gate_manifest_selects_wasm_host_ratchet(drv):
    cfg = drv.GateConfig.load(COMMITTED_GATES)
    gates, matched = cfg.select(["runtime/molt-wasm-host/src/main.rs"])

    assert [r.name for r in matched] == ["rust-ffi-blocks", "wasm-host"]
    assert "cargo clippy --locked -p molt-wasm-host -- -D warnings" in gates
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
    assert "echo always-ran" in gates
    assert "echo backend-gate" in gates
    assert "echo python-gate" not in gates
    assert [r.name for r in matched] == ["backend-native"]


def test_gate_manifest_selection_python(drv, tmp_path):
    cfg = drv.GateConfig.load(_write_gates(tmp_path))
    gates, matched = cfg.select(["src/molt/frontend.py"])
    assert "echo python-gate" in gates
    assert "echo backend-gate" not in gates
    assert [r.name for r in matched] == ["frontend-python"]


def test_gate_manifest_always_runs(drv, tmp_path):
    # An unmatched change-class still runs `always` gates, nothing else.
    cfg = drv.GateConfig.load(_write_gates(tmp_path))
    gates, matched = cfg.select(["docs/notes.txt"])
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


def test_integrate_detects_dropped_commit(drv, origin_and_clone, monkeypatch):
    """Simulate a rebase that DROPS a source commit and prove integrate fails.

    We monkeypatch Git.commits_in_range so that, for the post-rebase branch
    range, the source commit's patch-id is absent (as if a conflict resolution
    discarded it), while the source range still lists it. The patch-id survival
    check must then raise a DriverError naming the dangling sha.
    """
    origin, work = origin_and_clone
    git = drv.Git(work)
    dropped = _commit_file(work, "feature.py", "x = 1\n", "feat: important change")

    source_shas = [dropped]
    pre = {dropped: git.patch_id(dropped)}
    assert pre[dropped] is not None

    # New range contains NOTHING (the commit was 'dropped'); upstream too.
    monkeypatch.setattr(drv.Git, "commits_in_range", lambda self, rng: [], raising=True)
    with pytest.raises(drv.DriverError) as exc:
        drv._verify_no_dropped_commits(
            git, source_shas, pre, "origin/main..HEAD", git.rev_parse("origin/main")
        )
    assert "DROPPED COMMITS" in str(exc.value)
    assert git.short(dropped) in str(exc.value)


def test_integrate_accepts_already_upstream_by_patch_id(drv, origin_and_clone):
    """A source change that is ALREADY upstream (same patch-id) is NOT dropped.

    Build a commit, push an equivalent change to origin under a different sha,
    then verify the survival check accounts for the source by upstream patch-id
    match (the legitimate "moved upstream" dedup) rather than flagging it.
    """
    origin, work = origin_and_clone
    git = drv.Git(work)

    # Local source commit.
    src = _commit_file(work, "shared.py", "value = 42\n", "feat: shared value")
    pre = {src: git.patch_id(src)}

    # Push an EQUIVALENT change to origin from a separate clone (different sha,
    # same diff -> same patch-id).
    other = work.parent / "other"
    _git(work.parent, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "dev2@molt.test")
    _git(other, "config", "user.name", "Other Dev")
    _commit_file(other, "shared.py", "value = 42\n", "feat: same shared value")
    _git(other, "push", "-q", "origin", "main")

    git.fetch("origin", "main")
    upstream = git.rev_parse("origin/main")

    # The branch range (origin/main..HEAD) no longer contains the source change
    # by patch-id once origin has it; but the upstream span does. The check must
    # pass (no DriverError) because the patch-id is found upstream.
    # We pass a new_range that excludes the source to force the upstream branch.
    drv._verify_no_dropped_commits(
        git, [src], pre, f"{upstream}..{src}", upstream
    )  # must NOT raise


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


def test_cleanup_allows_clean_pushed(drv, origin_and_clone, tmp_path):
    origin, _work = origin_and_clone
    wt = _worktree_off(origin, tmp_path, "wt_clean")
    # No commits beyond origin/main, no dirty tree -> cleanup proceeds.
    rc = drv._cleanup_worktree(
        drv.Git(wt), wt, "origin/main", drv.DEFAULT_IGNORE_GLOBS, force_sha=None
    )
    assert rc == drv.EXIT_OK
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
# Hazard 5: comparison verdicts use python/plumbing (binaries-identical)
# --------------------------------------------------------------------------


def test_binaries_identical_helper(drv, tmp_path):
    a = tmp_path / "a.bin"
    b = tmp_path / "b.bin"
    c = tmp_path / "c.bin"
    a.write_bytes(b"\x00\x01\x02molt")
    b.write_bytes(b"\x00\x01\x02molt")
    c.write_bytes(b"\x00\x01\x02MOLT")
    assert drv._binaries_identical(a, b) is True
    assert drv._binaries_identical(a, c) is False
    assert drv._binaries_identical(a, tmp_path / "missing.bin") is False


# --------------------------------------------------------------------------
# Hazard 6: stale-binary misattribution (freshness + behavior marker)
# --------------------------------------------------------------------------


def _fake_binary(path: Path, prints: str) -> None:
    """A tiny executable python 'binary' that prints a marker, runnable under
    safe_run.py (which the driver always uses to run a binary)."""
    if os.name == "nt":
        path.write_text(f"@echo off\r\necho {prints}\r\n", encoding="utf-8")
        return
    path.write_text("#!/usr/bin/env python3\nprint(%r)\n" % prints, encoding="utf-8")
    path.chmod(0o755)


@pytest.mark.parametrize(
    "source_path",
    [
        "runtime/molt-backend/src/x.rs",
        "vendor/rust/object/src/write/coff/object.rs",
        "vendor/rust/cranelift-object/Cargo.toml",
    ],
)
def test_verify_toolchain_flags_stale_binary(
    drv, origin_and_clone, tmp_path, source_path
):
    origin, work = origin_and_clone
    git = drv.Git(work)
    # Make a binary, then make a NEWER Rust-source commit so the binary is stale.
    binary = tmp_path / "stale_bin"
    _fake_binary(binary, "SETATTR marker present")
    old = binary.stat().st_mtime
    os.utime(binary, (old - 10_000, old - 10_000))  # backdate the binary
    _commit_file(work, source_path, "// change\n", "rt: change")

    report = drv.verify_toolchain(
        git, binary, marker=None, probe_args=[], rss_mb=256, timeout=10
    )
    assert report.fresh is False  # mtime < newest rust commit


def test_verify_toolchain_fresh_binary_with_marker(drv, origin_and_clone, tmp_path):
    origin, work = origin_and_clone
    git = drv.Git(work)
    _commit_file(work, "runtime/molt-backend/src/y.rs", "// y\n", "rt: y")
    binary = tmp_path / ("fresh_bin.cmd" if os.name == "nt" else "fresh_bin")
    _fake_binary(binary, "native=148 ready")  # marker in output
    # Binary is created AFTER the commit -> fresh.
    report = drv.verify_toolchain(
        git, binary, marker="native=148", probe_args=[], rss_mb=256, timeout=10
    )
    assert report.fresh is True
    assert report.marker_found is True
    assert report.probe_exit == 0


def test_verify_toolchain_missing_marker_fails(drv, origin_and_clone, tmp_path):
    origin, work = origin_and_clone
    git = drv.Git(work)
    _commit_file(work, "runtime/molt-backend/src/z.rs", "// z\n", "rt: z")
    binary = tmp_path / ("nomarker_bin.cmd" if os.name == "nt" else "nomarker_bin")
    _fake_binary(binary, "some other output")
    report = drv.verify_toolchain(
        git, binary, marker="native=148", probe_args=[], rss_mb=256, timeout=10
    )
    assert report.marker_found is False


def test_verify_toolchain_reference_detects_noop_rebuild(
    drv, origin_and_clone, tmp_path
):
    origin, work = origin_and_clone
    git = drv.Git(work)
    a = tmp_path / "bin_a"
    b = tmp_path / "bin_b"
    _fake_binary(a, "v1")
    _fake_binary(b, "v1")  # byte-identical to a (a no-op rebuild)
    report = drv.verify_toolchain(
        git, a, marker=None, probe_args=[], rss_mb=256, timeout=10, reference=b
    )
    assert report.differs_from_reference is False  # identical -> suspicious


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
    assert "gate failed (exit 3): exit 3" in str(exc.value)
    assert not (work / "build-started.txt").exists()
    # Nothing was pushed (origin unchanged).
    origin_main = _git(work, "rev-parse", "origin/main").stdout.strip()
    seed = _git(work, "rev-list", "--max-parents=0", "origin/main").stdout.strip()
    assert origin_main == seed  # still just the seed commit


def test_integrate_full_push_and_confirm(drv, origin_and_clone, tmp_path):
    # The happy path end-to-end: a passing gate, real push, fetch+ancestor
    # confirm. Proves the push-confirm gate accepts a genuinely-landed tip.
    origin, work = origin_and_clone
    _commit_file(work, "src/molt/ok.py", "ok = True\n", "feat: ok")
    git = drv.Git(work)
    tip = git.head_sha()
    ns = _integrate_ns(
        drv,
        repo=str(work),
        no_push=False,
    )
    assert drv.cmd_integrate(ns) == drv.EXIT_OK
    # origin/main now contains the tip (confirmed by the same ancestor check).
    git.fetch("origin", "main")
    assert git.is_ancestor(tip, "origin/main")


def test_integrate_dry_run_mutates_nothing(drv, origin_and_clone, tmp_path):
    origin, work = origin_and_clone
    _commit_file(work, "src/molt/d.py", "d = 1\n", "feat: d")
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


def test_integrate_nothing_to_do_is_green(drv, origin_and_clone):
    # HEAD already on origin/main -> integrate is a green no-op.
    origin, work = origin_and_clone
    ns = _integrate_ns(drv, repo=str(work), no_push=True)
    assert drv.cmd_integrate(ns) == drv.EXIT_OK


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
        toolchain_binary=None,
        toolchain_marker=None,
        toolchain_probe_arg=None,
        rss_mb=512,
        timeout=10,
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


def test_integrate_clean_rebase_runs_committed_policy(drv, origin_and_clone):
    origin, work = origin_and_clone
    _commit_file(work, "feature.py", "value = 1\n", "local")
    other = work.parent / "upstream"
    _git(work.parent, "clone", "-q", str(origin), str(other))
    _git(other, "config", "user.email", "upstream@example.test")
    _git(other, "config", "user.name", "Upstream")
    upstream = _commit_file(other, "other.py", "other = 1\n", "upstream")
    _git(other, "push", "origin", "main")
    assert drv.cmd_integrate(_integrate_ns(drv, repo=str(work))) == drv.EXIT_OK
    selected = _git(work, "rev-parse", "HEAD").stdout.strip()
    assert drv.Git(work).is_ancestor(upstream, selected)
    assert (
        _git(work, "ls-remote", "origin", "refs/heads/main").stdout.split()[0]
        == selected
    )
    assert (work / "feature.py").read_text(encoding="utf-8") == "value = 1\n"


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
    assert commands == ["echo selected"]
    monkeypatch.setattr(
        proof_plan, "_version_fingerprint", lambda _: pytest.fail("unrelated probe ran")
    )
    cfg.admit(["src/one.py"], matched, [])
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
