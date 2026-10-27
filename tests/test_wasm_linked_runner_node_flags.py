from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any, cast

import pytest

import tests.wasm_linked_runner as wasm_runner
from molt.cargo_execution_policy import PROOF_COMMAND_TIMEOUT_ENV
from molt.cli.wasm_host import (
    molt_wasm_host_exe_name,
    resolve_molt_wasm_host_binary,
)
from molt.node_runtime import NodeRuntime, NodeRuntimeError
from molt.wasm_artifact import wasm_runtime_manifest_path


def test_require_node_binary_uses_shared_selector_with_test_guard(
    monkeypatch, tmp_path: Path
) -> None:
    node = tmp_path / "node"
    calls: list[dict[str, Any]] = []

    def fake_resolve(**kwargs):  # type: ignore[no-untyped-def]
        calls.append(kwargs)
        return NodeRuntime(path=node, version="22.1.0", major=22)

    monkeypatch.setattr(wasm_runner, "resolve_node_runtime", fake_resolve)

    assert wasm_runner.require_node_binary() == str(node)
    assert calls == [
        {
            "source_root": Path(wasm_runner.__file__).resolve().parents[1],
            "guard_prefix": "MOLT_WASM_TEST",
        }
    ]


def test_require_node_binary_skips_with_diagnostic_when_node_unavailable(
    monkeypatch,
) -> None:
    def fake_resolve(**kwargs):  # type: ignore[no-untyped-def]
        raise NodeRuntimeError("Node >= 18 is unavailable on PATH")

    monkeypatch.delenv("MOLT_NODE_BIN", raising=False)
    monkeypatch.setattr(wasm_runner, "resolve_node_runtime", fake_resolve)

    with pytest.raises(pytest.skip.Exception, match="unavailable on PATH"):
        wasm_runner.require_node_binary()


def test_require_node_binary_fails_on_unusable_explicit_selection(
    monkeypatch, tmp_path: Path
) -> None:
    # The real selector rejects a missing explicit path before any probe runs.
    monkeypatch.setenv("MOLT_NODE_BIN", str(tmp_path / "missing-node"))

    with pytest.raises(pytest.fail.Exception, match="MOLT_NODE_BIN"):
        wasm_runner.require_node_binary()


def test_wasm_test_process_uses_memory_guard(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout="ok\n", stderr="")

    monkeypatch.setattr(
        wasm_runner.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = wasm_runner._run_wasm_test_process(
        ["node", "-e", "console.log('ok')"],
        cwd=tmp_path,
        env={"NODE_NO_WARNINGS": "1"},
        timeout=5,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert captured["cmd"] == ["node", "-e", "console.log('ok')"]
    assert captured["kwargs"]["prefix"] == "MOLT_WASM_TEST"
    assert captured["kwargs"]["cwd"] == tmp_path
    assert captured["kwargs"]["timeout"] == 5


def test_wasm_test_process_preserves_timeout_semantics(
    monkeypatch, tmp_path: Path
) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        return wasm_runner.process_guard_common.harness_memory_guard.GuardedCompletedProcess(
            cmd,
            wasm_runner.process_guard_common.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
            stdout="partial",
            stderr="memory_guard: timeout after 2.00s\n",
            elapsed_s=2.0,
            timed_out=True,
            child_stderr="",
        )

    monkeypatch.setattr(
        wasm_runner.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    try:
        wasm_runner._run_wasm_test_process(
            ["node", "runner.js"],
            cwd=tmp_path,
            env={},
            timeout=2,
        )
    except subprocess.TimeoutExpired as exc:
        assert exc.cmd == ["node", "runner.js"]
        assert exc.output == "partial"
        assert exc.stderr == "memory_guard: timeout after 2.00s\n"
    else:  # pragma: no cover - assertion clarity
        raise AssertionError("expected TimeoutExpired")


def test_wasm_nested_default_inherits_owning_proof_budget(
    monkeypatch, tmp_path: Path
) -> None:
    captured: dict[str, Any] = {}

    def fake_guarded_completed_process(cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout="ok\n", stderr="")

    monkeypatch.setattr(
        wasm_runner.process_guard_common.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    wasm_runner._run_wasm_test_process(
        ["node", "runner.js"],
        cwd=tmp_path,
        env={PROOF_COMMAND_TIMEOUT_ENV: "1200"},
    )

    assert captured["kwargs"]["timeout"] == 1200


def test_resolve_molt_wasm_host_binary_prefers_explicit_env(
    monkeypatch,
    tmp_path: Path,
) -> None:
    host_bin = tmp_path / molt_wasm_host_exe_name()
    host_bin.write_bytes(b"host")
    target_host = tmp_path / "target" / "dev-fast" / molt_wasm_host_exe_name()
    target_host.parent.mkdir(parents=True)
    target_host.write_bytes(b"target")

    monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host_bin))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_host.parent.parent))

    assert resolve_molt_wasm_host_binary(tmp_path, cargo_profile="dev-fast") == str(
        host_bin
    )


def test_resolve_molt_wasm_host_binary_uses_dev_fast_target_dir(
    monkeypatch,
    tmp_path: Path,
) -> None:
    host_bin = tmp_path / "target" / "dev-fast" / molt_wasm_host_exe_name()
    host_bin.parent.mkdir(parents=True)
    host_bin.write_bytes(b"host")

    monkeypatch.delenv("MOLT_WASM_HOST_BIN", raising=False)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(host_bin.parent.parent))

    assert resolve_molt_wasm_host_binary(tmp_path, cargo_profile="dev-fast") == str(
        host_bin
    )


@pytest.mark.usefixtures("developer_host_context")
def test_resolve_molt_wasm_host_binary_uses_requested_profile(
    monkeypatch, tmp_path: Path
) -> None:
    # Without a run context the project's target is Cargo's <project>/target.
    monkeypatch.delenv("MOLT_WASM_HOST_BIN", raising=False)
    host_bin = tmp_path / "target" / "release-output" / molt_wasm_host_exe_name()
    host_bin.parent.mkdir(parents=True)
    host_bin.write_bytes(b"host")

    assert resolve_molt_wasm_host_binary(
        tmp_path, cargo_profile="release-output"
    ) == str(host_bin)
    assert resolve_molt_wasm_host_binary(tmp_path, cargo_profile="dev-fast") is None


def test_resolve_molt_wasm_host_binary_rejects_missing_explicit_binary(
    monkeypatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(tmp_path / "missing-host"))
    assert resolve_molt_wasm_host_binary(tmp_path, cargo_profile="dev-fast") is None


@pytest.mark.parametrize("selection", ["environment", "caller", "explicit"])
def test_missing_selected_wasm_host_does_not_use_other_builds(
    monkeypatch, tmp_path: Path, selection: str
) -> None:
    monkeypatch.delenv("MOLT_WASM_HOST_BIN", raising=False)
    for directory in (tmp_path / "target", tmp_path / "environment"):
        host = directory / "dev-fast" / molt_wasm_host_exe_name()
        host.parent.mkdir(parents=True)
        host.write_bytes(b"unselected host")
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "environment"))
    target = None
    if selection == "environment":
        monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "missing"))
    elif selection == "caller":
        target = tmp_path / "missing"
    else:
        monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(tmp_path / "missing"))
    assert (
        resolve_molt_wasm_host_binary(
            tmp_path, cargo_profile="dev-fast", target_dir=target
        )
        is None
    )


def test_relative_cargo_host_directory_follows_the_cli_target_rule(
    monkeypatch, tmp_path: Path
) -> None:
    from molt.cli.runtime_paths import _cargo_target_root

    root = tmp_path / "root"
    (tmp_path / "cwd").mkdir()
    monkeypatch.chdir(tmp_path / "cwd")
    cwd = Path.cwd()
    monkeypatch.delenv("MOLT_WASM_HOST_BIN", raising=False)
    monkeypatch.setenv("CARGO_TARGET_DIR", "selected target")
    # Cargo reads a relative CARGO_TARGET_DIR from the working directory, and
    # so does the CLI that builds the runtime there.
    host = cwd / "selected target" / "dev-fast" / molt_wasm_host_exe_name()
    host.parent.mkdir(parents=True)
    host.write_bytes(b"selected host")
    assert resolve_molt_wasm_host_binary(root, cargo_profile="dev-fast") == str(host)
    assert host.parent.parent == _cargo_target_root(root)


def test_proof_wasm_host_runner_preserves_actual_child_arguments_and_exit(
    monkeypatch, tmp_path: Path
) -> None:
    from tools import run_wasm_host

    output = tmp_path / "actual child arguments.json"
    monkeypatch.setenv("MOLT_WASM_HOST_BIN", sys.executable)
    result = run_wasm_host.main(
        [
            "--cargo-profile",
            "dev-fast",
            "--",
            "-c",
            "import json, pathlib, sys; "
            "pathlib.Path(sys.argv[1]).write_text(json.dumps(sys.argv[2:]), encoding='utf-8'); "
            "sys.exit(7)",
            str(output),
            "manifest with spaces.json",
            "--host-option",
        ]
    )
    assert result == 7
    assert json.loads(output.read_text(encoding="utf-8")) == [
        "manifest with spaces.json",
        "--host-option",
    ]


def test_wasm_matrix_uses_selected_dev_fast_host(monkeypatch, tmp_path: Path) -> None:
    from types import SimpleNamespace
    from tools import wasm_run_matrix as matrix

    host = tmp_path / "dev-fast" / molt_wasm_host_exe_name()
    host.parent.mkdir(parents=True)
    host.write_bytes(b"host")
    monkeypatch.delenv("MOLT_WASM_HOST_BIN", raising=False)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path))
    case = matrix.SMOKE_CORPUS[0]
    wasm = tmp_path / "app.wasm"
    (tmp_path / "manifest.json").write_text("{}", encoding="utf-8")
    calls = []

    def run(command, **_kwargs):
        calls.append(command)
        return SimpleNamespace(
            returncode=0, stdout=case.expected, stderr="", elapsed_s=0.1
        )

    monkeypatch.setattr(matrix.harness_memory_guard, "guarded_completed_process", run)
    assert matrix._runtime_present("molt-wasm-host") == (True, "")
    result = matrix._run_molt_wasm_host(case, wasm, limits=None)
    assert result.status == "pass"
    assert calls == [[str(host), str(wasm_runtime_manifest_path(wasm))]]


def test_wasm_matrix_build_only_does_not_probe_runtimes(
    monkeypatch, tmp_path: Path
) -> None:
    from contextlib import nullcontext
    from tools import wasm_run_matrix as matrix

    def unexpected_probe(*_args, **_kwargs):
        pytest.fail("build-only operation probed execution runtimes")

    monkeypatch.setattr(matrix, "_runtime_present", unexpected_probe)
    monkeypatch.setattr(matrix, "build_corpus", lambda **_kwargs: {})
    monkeypatch.setattr(
        matrix.harness_memory_guard,
        "repo_process_sentinel",
        lambda **_kwargs: nullcontext(),
    )
    assert matrix.main(["--build-only", "--out-dir", str(tmp_path)]) == 0


def test_run_wasm_linked_uses_molt_wasm_host(
    monkeypatch,
    tmp_path: Path,
) -> None:
    wasm_path = tmp_path / "output_linked.wasm"
    wasm_path.write_bytes(b"\x00asm")
    manifest = tmp_path / "manifest.json"
    manifest.write_text("{}", encoding="utf-8")
    host_bin = tmp_path / molt_wasm_host_exe_name()
    host_bin.write_bytes(b"host")
    monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host_bin))
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["args"] = list(args[0])
        recorded["env"] = dict(kwargs["env"])
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    result = wasm_runner.run_wasm_linked(tmp_path, wasm_path)
    assert result.returncode == 0
    cmd = cast(list[str], recorded["args"])
    assert cmd == [str(host_bin), str(manifest)]
    env = cast(dict[str, str], recorded["env"])
    assert "NODE_NO_WARNINGS" not in env
    assert "MOLT_WASM_TEST_CHILD_RLIMIT_GB" not in env


def test_run_wasm_linked_preserves_inherited_child_rlimit_by_default(
    monkeypatch,
    tmp_path: Path,
) -> None:
    wasm_path = tmp_path / "output_linked.wasm"
    wasm_path.write_bytes(b"\x00asm")
    (tmp_path / "manifest.json").write_text("{}", encoding="utf-8")
    host_bin = tmp_path / molt_wasm_host_exe_name()
    host_bin.write_bytes(b"host")
    monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host_bin))
    monkeypatch.setenv("MOLT_WASM_TEST_CHILD_RLIMIT_GB", "16")
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["env"] = dict(kwargs["env"])
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    result = wasm_runner.run_wasm_linked(tmp_path, wasm_path)

    assert result.returncode == 0
    env = cast(dict[str, str], recorded["env"])
    assert env.get("MOLT_WASM_TEST_CHILD_RLIMIT_GB") == "16"


@pytest.mark.parametrize("exit_code", [0, 7])
def test_run_wasm_linked_uses_product_cwd_without_rebasing_inputs(
    monkeypatch, tmp_path: Path, exit_code: int
) -> None:
    checkout = tmp_path / "checkout"
    checkout.mkdir()
    products = tmp_path / "products"
    products.mkdir()
    wasm_path = products / "output_linked.wasm"
    wasm_path.write_bytes(b"guest product")
    manifest = products / "manifest.json"
    manifest.write_text(json.dumps({"bundle": "bundle.data"}), encoding="utf-8")
    (products / "bundle.data").write_text("bundled input", encoding="utf-8")
    explicit_input = tmp_path / "explicit.data"
    explicit_input.write_text("explicit input", encoding="utf-8")
    host_temp = tmp_path / "host-temp"
    host_temp.mkdir()
    for name, value in {
        "TMPDIR": str(host_temp),
        "TEMP": "explicit-relative-temp",
        "TMP": "explicit-other-temp",
        "MOLT_CWD_PROBE_INPUT": str(explicit_input),
    }.items():
        monkeypatch.setenv(name, value)
    inherited = dict(os.environ)
    monkeypatch.chdir(tmp_path)
    selections = []

    def select_host(root, *, cargo_profile, target_dir):
        selections.append((root, cargo_profile, target_dir))
        return sys.executable

    monkeypatch.setattr(wasm_runner, "resolve_molt_wasm_host_binary", select_host)
    real_run = wasm_runner._run_wasm_test_process
    calls = []

    def run(command, **kwargs):
        calls.append((list(command), kwargs["cwd"]))
        assert command == [sys.executable, str(manifest)]
        assert kwargs["cwd"] == products
        for name in ("TMPDIR", "TEMP", "TMP", "MOLT_CWD_PROBE_INPUT"):
            assert kwargs["env"][name] == inherited[name]
        assert kwargs["env"]["MOLT_WASM_HOST_DEBUG"] == "1"
        # Exercise the real guarded process cwd and relative filesystem writes;
        # this is a custody oracle, not a compiler or WASI execution substitute.
        probe = """
import json, os, pathlib, sys
manifest = pathlib.Path(sys.argv[1])
assert manifest.is_absolute()
assert pathlib.Path.cwd() == manifest.parent
assert (manifest.parent / json.loads(manifest.read_text(encoding='utf-8'))['bundle']).read_text(encoding='utf-8') == 'bundled input'
assert pathlib.Path(os.environ['MOLT_CWD_PROBE_INPUT']).read_text(encoding='utf-8') == 'explicit input'
assert os.environ['TEMP'] == 'explicit-relative-temp'
assert os.environ['TMP'] == 'explicit-other-temp'
assert os.environ['MOLT_WASM_HOST_DEBUG'] == '1'
pathlib.Path('relative-output').write_text('guest output', encoding='utf-8')
print('owned cwd and explicit inputs')
sys.exit(int(sys.argv[2]))
"""
        return real_run(
            [sys.executable, "-B", "-c", probe, command[1], str(exit_code)],
            **kwargs,
        )

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", run)
    result = wasm_runner.run_wasm_linked(
        checkout,
        Path("products/output_linked.wasm"),
        env_overrides={"MOLT_WASM_HOST_DEBUG": "1"},
    )
    assert result.returncode == exit_code, result.stderr
    assert result.stdout == "owned cwd and explicit inputs\n"
    assert len(selections) == 1
    assert selections[0][:2] == (checkout, "dev-fast")
    assert selections[0][2] == wasm_runner.wasm_test_cargo_target_dir(checkout)
    assert calls == [([sys.executable, str(manifest)], products)]
    assert (products / "relative-output").read_text(encoding="utf-8") == "guest output"
    assert not (checkout / "relative-output").exists()
    assert not (tmp_path / "relative-output").exists()
    assert list(checkout.iterdir()) == []
    assert wasm_path.read_bytes() == b"guest product"
    assert explicit_input.read_text(encoding="utf-8") == "explicit input"


def test_run_wasm_linked_preserves_explicit_env_overrides(
    monkeypatch,
    tmp_path: Path,
) -> None:
    wasm_path = tmp_path / "output_linked.wasm"
    wasm_path.write_bytes(b"\x00asm")
    (tmp_path / "manifest.json").write_text("{}", encoding="utf-8")
    host_bin = tmp_path / molt_wasm_host_exe_name()
    host_bin.write_bytes(b"host")
    monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host_bin))
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded.setdefault("envs", []).append(dict(kwargs["env"]))
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)

    result = wasm_runner.run_wasm_linked(
        tmp_path,
        wasm_path,
        env_overrides={
            "MOLT_WASM_HOST_DEBUG": "1",
            "MOLT_WASM_TEST_CHILD_RLIMIT_GB": "0",
        },
    )
    assert result.returncode == 0

    envs = cast(list[dict[str, str]], recorded["envs"])
    assert envs[0].get("MOLT_WASM_HOST_DEBUG") == "1"
    assert envs[0].get("MOLT_WASM_TEST_CHILD_RLIMIT_GB") == "0"


def test_run_wasm_linked_scrubs_stale_artifact_env(
    monkeypatch,
    tmp_path: Path,
) -> None:
    wasm_path = tmp_path / "output_linked.wasm"
    wasm_path.write_bytes(b"\x00asm")
    (tmp_path / "manifest.json").write_text("{}", encoding="utf-8")
    host_bin = tmp_path / molt_wasm_host_exe_name()
    host_bin.write_bytes(b"host")
    monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host_bin))
    monkeypatch.setenv("MOLT_WASM_TABLE_BASE", "123")
    monkeypatch.setenv("MOLT_RUNTIME_WASM", "/tmp/stale-runtime.wasm")
    monkeypatch.setenv("MOLT_WASM_MANIFEST_PATH", "/tmp/stale-manifest.json")
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["env"] = dict(kwargs["env"])
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    result = wasm_runner.run_wasm_linked(tmp_path, wasm_path)
    assert result.returncode == 0
    env_names = set(cast(dict[str, str], recorded["env"]))
    assert not env_names.intersection(
        {"MOLT_WASM_TABLE_BASE", "MOLT_RUNTIME_WASM", "MOLT_WASM_MANIFEST_PATH"}
    )


def test_build_wasm_linked_treats_symlinked_ext_root_as_repo_local(
    monkeypatch,
    tmp_path: Path,
) -> None:
    # This is a durable/local project contract. A real hosted checkout is
    # source-only, and deliberately rejects artifact roots inside its tree.
    root = tmp_path / "repo"
    root.mkdir()
    alias_root = tmp_path / "repo-alias"
    try:
        alias_root.symlink_to(root, target_is_directory=True)
    except OSError as exc:
        pytest.skip(f"directory symlinks are unavailable on this host: {exc}")
    src = tmp_path / "probe.py"
    src.write_text("print('hi')\n", encoding="utf-8")
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["env"] = dict(kwargs["env"])
        out_dir = Path(args[0][args[0].index("--out-dir") + 1])
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "output_linked.wasm").write_bytes(b"\x00asm")
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    monkeypatch.delenv("MOLT_REQUIRE_EXTERNAL_ARTIFACTS", raising=False)
    monkeypatch.setenv("MOLT_PREFER_EXTERNAL_ARTIFACTS", "1")
    monkeypatch.setenv("MOLT_EXT_ROOT", str(alias_root))
    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    output = wasm_runner.build_wasm_linked(root, src, tmp_path)
    assert output.exists()
    env = cast(dict[str, str], recorded["env"])
    # The alias names the checkout itself, so the target is Cargo's own
    # <checkout>/target, not a checkouts/ entry under a shared root.
    assert Path(env["CARGO_TARGET_DIR"]) == root.resolve() / "target"


def test_build_wasm_linked_marks_repo_local_output_as_output_not_required_external(
    monkeypatch,
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    root.mkdir()
    src = tmp_path / "probe.py"
    src.write_text("print('hi')\n", encoding="utf-8")
    recorded: dict[str, Any] = {}

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        recorded["args"] = list(args[0])
        recorded["env"] = dict(kwargs["env"])
        out_dir = Path(args[0][args[0].index("--out-dir") + 1])
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "output_linked.wasm").write_bytes(b"\x00asm")
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    monkeypatch.delenv("MOLT_PREFER_EXTERNAL_ARTIFACTS", raising=False)
    monkeypatch.delenv("MOLT_REQUIRE_EXTERNAL_ARTIFACTS", raising=False)
    monkeypatch.delenv("CARGO_BUILD_JOBS", raising=False)
    monkeypatch.delenv("MOLT_USE_SCCACHE", raising=False)
    monkeypatch.delenv("MOLT_WASM_TEST_LANE", raising=False)
    monkeypatch.delenv("PYTEST_XDIST_WORKER", raising=False)
    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    output = wasm_runner.build_wasm_linked(root, src, tmp_path)

    assert output.exists()
    env = cast(dict[str, str], recorded["env"])
    assert Path(env["MOLT_EXT_ROOT"]).is_relative_to(root / "build" / "wasm")
    # Test outputs move beside the build; the Cargo target stays the
    # checkout's own, where every other build of this checkout goes.
    assert Path(env["CARGO_TARGET_DIR"]) == root.resolve() / "target"
    assert "MOLT_REQUIRE_EXTERNAL_ARTIFACTS" not in env
    assert "CARGO_BUILD_JOBS" not in env


def test_wasm_test_build_env_delegates_cargo_parallelism_and_cache_policy(
    monkeypatch,
    tmp_path: Path,
) -> None:
    calls: list[dict[str, object]] = []

    def _fake_development_artifact_env(
        repo_root: Path,
        base: dict[str, str],
        **kwargs: object,
    ) -> dict[str, str]:
        calls.append({"repo_root": repo_root, "base": dict(base), **kwargs})
        env = dict(base)
        env["PYTHONPATH"] = str(repo_root / "src")
        return env

    monkeypatch.delenv("CARGO_BUILD_JOBS", raising=False)
    monkeypatch.delenv("MOLT_USE_SCCACHE", raising=False)
    # The lane is a fact of the running process; an xdist worker names its
    # own (test_wasm_build_env_replaces_generated_pytest_session_with_stable_lane).
    monkeypatch.delenv("MOLT_WASM_TEST_LANE", raising=False)
    monkeypatch.delenv("PYTEST_XDIST_WORKER", raising=False)
    monkeypatch.setattr(
        wasm_runner,
        "development_artifact_env",
        _fake_development_artifact_env,
    )

    env = wasm_runner.wasm_test_build_env(
        tmp_path,
        linked=False,
        session_prefix="browser-proof",
        session_id="browser-proof-local",
        create_dirs=False,
    )

    assert env["PYTHONPATH"] == str(tmp_path / "src")
    assert env["MOLT_WASM_LINKED"] == "0"
    assert "CARGO_BUILD_JOBS" not in env
    assert len(calls) == 1
    assert calls[0]["repo_root"] == tmp_path
    assert calls[0]["session_prefix"] == "browser-proof"
    assert calls[0]["session_id"] == "browser-proof-local"
    assert calls[0]["create_dirs"] is False

    calls.clear()
    wasm_runner.wasm_test_build_env(
        tmp_path,
        base_env={
            "MOLT_SESSION_ID": "pytest-ephemeral",
            "MOLT_SESSION_ID_GENERATED": "1",
        },
        create_dirs=False,
    )
    assert calls[0]["session_id"] == "test-wasm-local"

    explicit = wasm_runner.wasm_test_build_env(
        tmp_path,
        base_env={
            "CARGO_BUILD_JOBS": "3",
            "MOLT_USE_SCCACHE": "0",
        },
    )
    assert explicit["CARGO_BUILD_JOBS"] == "3"
    assert explicit["MOLT_USE_SCCACHE"] == "0"


def test_wasm_builds_share_one_persistent_target_per_checkout(
    monkeypatch,
    tmp_path: Path,
) -> None:
    # HF-45: builds of one checkout reuse one Cargo target although each pins
    # its own session; two checkouts under one artifact root never share one,
    # and molt-wasm-host is looked up where the builds went.
    ext_root = tmp_path / "ext"
    ext_root.mkdir()
    first_checkout = tmp_path / "first"
    second_checkout = tmp_path / "second"
    first_checkout.mkdir()
    second_checkout.mkdir()
    src = tmp_path / "probe.py"
    src.write_text("print('hi')\n", encoding="utf-8")
    targets: list[Path] = []

    def _fake_build(*args, **kwargs):  # type: ignore[no-untyped-def]
        targets.append(Path(kwargs["env"]["CARGO_TARGET_DIR"]))
        out_dir = Path(args[0][args[0].index("--out-dir") + 1])
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "output_linked.wasm").write_bytes(b"\x00asm")
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    monkeypatch.delenv("MOLT_REQUIRE_EXTERNAL_ARTIFACTS", raising=False)
    monkeypatch.delenv("MOLT_WASM_HOST_BIN", raising=False)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))
    monkeypatch.setenv("MOLT_PREFER_EXTERNAL_ARTIFACTS", "1")
    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_build)
    for checkout, out_name in (
        (first_checkout, "a"),
        (first_checkout, "b"),
        (second_checkout, "c"),
    ):
        wasm_runner.build_wasm_linked(checkout, src, tmp_path / out_name)

    first, again, second = targets
    shared = ext_root.resolve() / "target"
    assert first == again
    assert first != second
    assert first.is_relative_to(shared) and second.is_relative_to(shared)
    assert shared not in {first, second}
    assert "sessions" not in first.parts

    host = first / "dev-fast" / molt_wasm_host_exe_name()
    host.parent.mkdir(parents=True)
    host.write_bytes(b"host")
    wasm_path = tmp_path / "output_linked.wasm"
    wasm_path.write_bytes(b"\x00asm")
    (tmp_path / "manifest.json").write_text("{}", encoding="utf-8")
    monkeypatch.setenv("MOLT_SESSION_ID", "pinned-run-session")
    monkeypatch.delenv("MOLT_SESSION_ID_GENERATED", raising=False)
    executed: list[str] = []

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        executed.append(args[0][0])
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    assert wasm_runner.run_wasm_linked(first_checkout, wasm_path).returncode == 0
    assert executed == [str(host.absolute())]


def test_build_wasm_linked_does_not_mutate_process_runtime_env(
    monkeypatch,
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "probe.py"
    src.write_text("print('hi')\n", encoding="utf-8")
    monkeypatch.delenv("MOLT_RUNTIME_WASM", raising=False)

    def _fake_run(*args, **kwargs):  # type: ignore[no-untyped-def]
        out_dir = Path(args[0][args[0].index("--out-dir") + 1])
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "output_linked.wasm").write_bytes(b"\x00asm")
        return subprocess.CompletedProcess(args[0], 0, "", "")

    monkeypatch.setattr(wasm_runner, "_run_wasm_test_process", _fake_run)
    output = wasm_runner.build_wasm_linked(root, src, tmp_path)
    assert output.exists()
    assert "MOLT_RUNTIME_WASM" not in os.environ


@pytest.mark.slow
def test_run_wasm_linked_does_not_require_runtime_sidecar_when_linked(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    wasm_runner.require_wasm_toolchain()
    src = root / "examples" / "hello.py"
    output_wasm = wasm_runner.build_wasm_linked(root, src, tmp_path)
    result = wasm_runner.run_wasm_linked(root, output_wasm)
    assert result.returncode == 0, result.stderr
    assert result.stdout == "42\n"
    # Exercise both finite lifetime owners against the same built artifact;
    # flushing in one host must not hide missing teardown in the other.
    node = wasm_runner._run_wasm_test_process(
        [
            wasm_runner.require_node_binary(),
            "wasm/run_wasm.js",
            str(wasm_runtime_manifest_path(output_wasm)),
        ],
        cwd=root,
        env=os.environ.copy(),
        capture_output=True,
        text=True,
        timeout=20,
        check=False,
    )
    assert node.returncode == 0, node.stderr
    assert node.stdout == result.stdout


@pytest.mark.slow
def test_run_wasm_linked_bench_sum_has_no_table_signature_trap(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    wasm_runner.require_wasm_toolchain()
    src = root / "tests" / "benchmarks" / "bench_sum.py"
    output_wasm = wasm_runner.build_wasm_linked(root, src, tmp_path)
    result = wasm_runner.run_wasm_linked(root, output_wasm)
    assert result.returncode == 0, result.stderr
    assert result.stdout == "49999995000000\n"
    assert "null function or function signature mismatch" not in result.stderr


@pytest.mark.slow
def test_run_wasm_direct_bootstraps_split_runtime_before_main(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    wasm_runner.require_wasm_build_toolchain()
    node_bin = wasm_runner.require_node_binary()
    src = tmp_path / "direct_bootstrap.py"
    src.write_text("import abc\nprint('after')\n", encoding="utf-8")

    env = wasm_runner.wasm_test_build_env(root)
    build = wasm_runner._run_wasm_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--build-profile",
            "dev",
            "--profile",
            "browser",
            "--target",
            "wasm",
            "--split-runtime",
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
    )
    assert build.returncode == 0, build.stderr

    result = wasm_runner._run_wasm_test_process(
        [node_bin, "wasm/run_wasm.js", str(tmp_path / "manifest.json")],
        cwd=root,
        env=os.environ.copy(),
        capture_output=True,
        text=True,
        timeout=20,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip().splitlines() == ["after"]


@pytest.mark.slow
def test_linked_wasm_exports_table_base_setter_when_available(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    wasm_runner.require_wasm_toolchain()
    src = root / "examples" / "hello.py"
    output_wasm = wasm_runner.build_wasm_linked(root, src, tmp_path)
    node_bin = wasm_runner.require_node_binary()
    probe = wasm_runner._run_wasm_test_process(
        [
            node_bin,
            "-e",
            (
                "const fs=require('fs');"
                "const p=process.argv[1];"
                "WebAssembly.compile(fs.readFileSync(p)).then((m)=>{"
                "const names=WebAssembly.Module.exports(m).map((e)=>e.name);"
                "console.log(JSON.stringify(names));"
                "}).catch((err)=>{console.error(String(err));process.exit(1);});"
            ),
            str(output_wasm),
        ],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    assert probe.returncode == 0, probe.stderr
    names = json.loads(probe.stdout)
    assert "molt_set_wasm_table_base" in names
