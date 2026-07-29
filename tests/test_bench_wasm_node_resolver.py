from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

import tools.bench_wasm as bench_wasm


def _module_descriptor(path: Path, manifest_path: Path) -> dict[str, object]:
    payload = path.read_bytes()
    return {
        "path": path.relative_to(manifest_path.parent).as_posix(),
        "size": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    }


def _write_linked_build(output_path: Path) -> None:
    output_path.write_bytes(b"\x00asm\x01\x00\x00\x00")
    linked = output_path.with_name("output_linked.wasm")
    linked.write_bytes(b"\x00asm\x01\x00\x00\x00")
    manifest = output_path.with_name("manifest.json")
    manifest.write_text(
        json.dumps(
            {
                "version": 2,
                "mode": "linked",
                "modules": {"linked": _module_descriptor(linked, manifest)},
            }
        ),
        encoding="utf-8",
    )


def _reset_cache(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(bench_wasm, "_NODE_BIN_CACHE", None)


def test_resolve_node_binary_accepts_env_override(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _reset_cache(monkeypatch)
    monkeypatch.setenv("MOLT_NODE_BIN", "/custom/node")
    monkeypatch.setattr(
        bench_wasm,
        "_node_major_for_binary",
        lambda path: 20 if path == "/custom/node" else None,
    )
    assert bench_wasm.resolve_node_binary() == "/custom/node"


def test_resolve_node_binary_rejects_old_env_override(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _reset_cache(monkeypatch)
    monkeypatch.setenv("MOLT_NODE_BIN", "/old/node")
    monkeypatch.setattr(bench_wasm, "_node_major_for_binary", lambda _path: 14)
    with pytest.raises(RuntimeError, match="Node >="):
        bench_wasm.resolve_node_binary()


def test_resolve_node_binary_prefers_highest_major(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _reset_cache(monkeypatch)
    monkeypatch.delenv("MOLT_NODE_BIN", raising=False)
    monkeypatch.setattr(bench_wasm.shutil, "which", lambda _name: "/usr/local/bin/node")
    majors = {
        "/usr/local/bin/node": 14,
        "/opt/homebrew/bin/node": 25,
    }
    monkeypatch.setattr(
        bench_wasm, "_node_major_for_binary", lambda path: majors.get(path)
    )
    assert bench_wasm.resolve_node_binary() == "/opt/homebrew/bin/node"


def test_resolve_node_binary_errors_when_none_valid(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _reset_cache(monkeypatch)
    monkeypatch.delenv("MOLT_NODE_BIN", raising=False)
    monkeypatch.setattr(bench_wasm.shutil, "which", lambda _name: None)
    monkeypatch.setattr(bench_wasm, "_node_major_for_binary", lambda _path: None)
    with pytest.raises(RuntimeError, match="Node binary not found"):
        bench_wasm.resolve_node_binary()


def test_resolve_runner_node_enforces_stable_wasm_flags(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(bench_wasm, "resolve_node_binary", lambda: "/usr/bin/node")
    monkeypatch.delenv("MOLT_WASM_NODE_OPTIONS", raising=False)
    cmd = bench_wasm._resolve_runner(
        "node", tty=False, log=None, node_max_old_space_mb=None
    )
    assert cmd[0] == "/usr/bin/node"
    assert "--no-warnings" in cmd
    assert "--no-wasm-tier-up" in cmd
    assert "--no-wasm-dynamic-tiering" in cmd
    assert "--wasm-num-compilation-tasks=1" in cmd
    assert cmd[-1] == "wasm/run_wasm.js"


def test_python_cmd_reuses_active_harness_interpreter() -> None:
    assert bench_wasm._python_cmd() == [bench_wasm.sys.executable]


def test_prepare_wasm_binary_sets_linked_table_base(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    reloc_runtime = tmp_path / "molt_runtime_reloc.wasm"
    reloc_runtime.write_bytes(b"\x00asm")
    monkeypatch.setattr(bench_wasm, "RUNTIME_WASM_RELOC", reloc_runtime)
    monkeypatch.setattr(bench_wasm, "RUNTIME_WASM", tmp_path / "molt_runtime.wasm")
    base_env = {"MOLT_SESSION_ID": "wasm-unit", "CARGO_TARGET_DIR": str(tmp_path)}
    pruned_envs: list[dict[str, str]] = []
    monkeypatch.setattr(bench_wasm, "_base_env", lambda: base_env.copy())
    monkeypatch.setattr(
        bench_wasm,
        "_prune_backend_daemons",
        lambda env=None: pruned_envs.append(dict(env or {})),
    )
    monkeypatch.setattr(bench_wasm, "_python_cmd", lambda: ["python3"])
    monkeypatch.setattr(bench_wasm, "_read_wasm_table_min", lambda _path: 2354)
    captured_env: dict[str, str] = {}

    def _fake_build(
        _python_cmd: list[str],
        env: dict[str, str],
        output_path: Path,
        _script: str,
        *,
        tty: bool,
        log,
        limits=None,
        use_molt_build_cache=True,
    ) -> float:
        del tty, log, limits, use_molt_build_cache
        captured_env.update(env)
        _write_linked_build(output_path)
        return 0.01

    monkeypatch.setattr(bench_wasm, "_build_wasm_output", _fake_build)

    wasm = bench_wasm.prepare_wasm_binary(
        "tests/benchmarks/bench_sum.py",
        tty=False,
        log=None,
        keep_temp=False,
    )
    assert wasm is not None
    assert pruned_envs == [base_env]
    assert "MOLT_WASM_LINK" not in captured_env
    assert captured_env.get("MOLT_WASM_TABLE_BASE") == "2354"


def test_build_wasm_output_reuses_molt_build_cache_by_default(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    output_path = tmp_path / "output.wasm"
    commands: list[list[str]] = []
    monkeypatch.setattr(bench_wasm, "molt_args_for_benchmark", lambda _script: [])

    def fake_run_cmd(command, *, env, capture, tty, log, timeout_s=None, limits=None):
        del env, capture, tty, log, timeout_s, limits
        commands.append(list(command))
        _write_linked_build(output_path)
        return bench_wasm._RunResult(returncode=0)

    monkeypatch.setattr(bench_wasm, "_run_cmd", fake_run_cmd)

    assert (
        bench_wasm._build_wasm_output(
            ["python3"],
            {},
            output_path,
            "tests/benchmarks/bench_sum.py",
            tty=False,
            log=None,
        )
        is not None
    )

    assert "--cache" in commands[0]
    assert "--no-cache" not in commands[0]
    assert "--linked" in commands[0]
    assert "--require-linked" in commands[0]


def test_build_wasm_output_can_disable_molt_build_cache(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    output_path = tmp_path / "output.wasm"
    commands: list[list[str]] = []
    monkeypatch.setattr(bench_wasm, "molt_args_for_benchmark", lambda _script: [])

    def fake_run_cmd(command, *, env, capture, tty, log, timeout_s=None, limits=None):
        del env, capture, tty, log, timeout_s, limits
        commands.append(list(command))
        _write_linked_build(output_path)
        return bench_wasm._RunResult(returncode=0)

    monkeypatch.setattr(bench_wasm, "_run_cmd", fake_run_cmd)

    assert (
        bench_wasm._build_wasm_output(
            ["python3"],
            {},
            output_path,
            "tests/benchmarks/bench_sum.py",
            tty=False,
            log=None,
            use_molt_build_cache=False,
        )
        is not None
    )

    assert "--no-cache" in commands[0]
    assert "--cache" not in commands[0]


def test_run_wasm_resolves_linked_module_only_from_manifest(
    tmp_path: Path,
) -> None:
    repo_root = Path(__file__).resolve().parents[1]
    explicit_dir = tmp_path / "artifacts"
    module_dir = tmp_path / "wasm"
    dist_dir = tmp_path / "dist"
    explicit_dir.mkdir()
    module_dir.mkdir()
    dist_dir.mkdir()

    explicit_linked = explicit_dir / "output_linked.wasm"
    explicit_linked.write_bytes(b"\x00asm")
    manifest_path = explicit_dir / "release.json"
    manifest_path.write_text(
        json.dumps(
            {
                "mode": "linked",
                "modules": {
                    "linked": _module_descriptor(explicit_linked, manifest_path)
                },
            }
        ),
        encoding="utf-8",
    )

    script = tmp_path / "resolve_wasm_paths.cjs"
    script.write_text(
        "const mod = require(%r);\n"
        "const resolved = mod.resolveWasmPaths({\n"
        "  manifestPath: %r,\n"
        "  moduleDir: %r,\n"
        "  tmpDir: %r,\n"
        "});\n"
        "console.log(JSON.stringify(resolved));\n"
        % (
            str(repo_root / "wasm" / "run_wasm.js"),
            str(manifest_path),
            str(module_dir),
            str(tmp_path),
        )
    )

    run = __import__("subprocess").run(
        ["node", str(script)],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
    )
    assert run.returncode == 0, run.stderr
    resolved = __import__("json").loads(run.stdout)
    assert resolved["manifestPath"] == str(manifest_path)
    assert resolved["wasmPath"] == str(explicit_linked)
    assert resolved["linkedPath"] == str(explicit_linked)
    assert resolved["runtimePath"] is None


def test_run_wasm_rejects_module_path_as_discovery_authority(
    tmp_path: Path,
) -> None:
    repo_root = Path(__file__).resolve().parents[1]
    module_dir = tmp_path / "wasm"
    tmp_dir = tmp_path / "tmp"
    module_dir.mkdir()
    tmp_dir.mkdir()

    temp_wasm = tmp_dir / "output.wasm"
    temp_linked = tmp_dir / "output_linked.wasm"

    temp_wasm.write_bytes(b"\x00asm")
    temp_linked.write_bytes(b"\x00asm")

    script = tmp_path / "resolve_wasm_paths_default.cjs"
    script.write_text(
        "const mod = require(%r);\n"
        "const resolved = mod.resolveWasmPaths({\n"
        "  manifestPath: %r,\n"
        "  moduleDir: %r,\n"
        "  tmpDir: %r,\n"
        "});\n"
        "console.log(JSON.stringify(resolved));\n"
        % (
            str(repo_root / "wasm" / "run_wasm.js"),
            str(temp_wasm),
            str(module_dir),
            str(tmp_dir),
        )
    )

    run = __import__("subprocess").run(
        ["node", str(script)],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
    )
    assert run.returncode != 0, run.stdout
    assert "accepts a runtime manifest path, not a module path" in run.stderr


def test_run_wasm_resolves_split_modules_relative_to_manifest(
    tmp_path: Path,
) -> None:
    repo_root = Path(__file__).resolve().parents[1]
    explicit_dir = tmp_path / "artifacts"
    module_dir = tmp_path / "wasm"
    canonical_runtime_dir = tmp_path / "canonical-wasm"
    explicit_dir.mkdir()
    module_dir.mkdir()
    canonical_runtime_dir.mkdir()

    explicit_wasm = explicit_dir / "app-prod.wasm"
    sibling_runtime = explicit_dir / "runtime-prod.wasm"

    explicit_wasm.write_bytes(b"\x00asm")
    sibling_runtime.write_bytes(b"\x00asm")
    manifest_path = explicit_dir / "release.json"
    manifest_path.write_text(
        json.dumps(
            {
                "mode": "split-runtime",
                "modules": {
                    "app": _module_descriptor(explicit_wasm, manifest_path),
                    "runtime": _module_descriptor(sibling_runtime, manifest_path),
                },
            }
        ),
        encoding="utf-8",
    )

    script = tmp_path / "resolve_runtime_sidecar.cjs"
    script.write_text(
        "const mod = require(%r);\n"
        "const resolved = mod.resolveWasmPaths({\n"
        "  manifestPath: %r,\n"
        "  moduleDir: %r,\n"
        "});\n"
        "console.log(JSON.stringify(resolved));\n"
        % (
            str(repo_root / "wasm" / "run_wasm.js"),
            str(manifest_path),
            str(module_dir),
        )
    )

    run = __import__("subprocess").run(
        ["node", str(script)],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
    )
    assert run.returncode == 0, run.stderr
    resolved = __import__("json").loads(run.stdout)
    assert resolved["runtimePath"] == str(sibling_runtime)
    assert resolved["wasmPath"] == str(explicit_wasm)
    assert resolved["linkedPath"] is None


def test_run_wasm_rejects_manifest_module_digest_drift(
    tmp_path: Path,
) -> None:
    repo_root = Path(__file__).resolve().parents[1]
    explicit_dir = tmp_path / "artifacts"
    module_dir = tmp_path / "wasm"
    canonical_runtime_dir = tmp_path / "canonical-wasm"
    explicit_dir.mkdir()
    module_dir.mkdir()
    canonical_runtime_dir.mkdir()

    explicit_wasm = explicit_dir / "output.wasm"
    runtime = explicit_dir / "runtime-prod.wasm"

    explicit_wasm.write_bytes(b"\x00asm")
    runtime.write_bytes(b"\x00asm")
    manifest_path = explicit_dir / "manifest.json"
    runtime_descriptor = _module_descriptor(runtime, manifest_path)
    runtime_descriptor["sha256"] = "0" * 64
    manifest_path.write_text(
        json.dumps(
            {
                "mode": "split-runtime",
                "modules": {
                    "app": _module_descriptor(explicit_wasm, manifest_path),
                    "runtime": runtime_descriptor,
                },
            }
        ),
        encoding="utf-8",
    )

    script = tmp_path / "resolve_runtime_canonical.cjs"
    script.write_text(
        "const mod = require(%r);\n"
        "const resolved = mod.resolveWasmPaths({\n"
        "  manifestPath: %r,\n"
        "  moduleDir: %r,\n"
        "});\n"
        "console.log(JSON.stringify(resolved));\n"
        % (
            str(repo_root / "wasm" / "run_wasm.js"),
            str(manifest_path),
            str(module_dir),
        )
    )

    run = __import__("subprocess").run(
        ["node", str(script)],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
    )
    assert run.returncode != 0
    assert "runtime wasm SHA-256 mismatch" in run.stderr


def test_run_wasm_execution_and_owned_value_guards_release_on_throw(
    tmp_path: Path,
) -> None:
    repo_root = Path(__file__).resolve().parents[1]
    script = tmp_path / "guard_runtime_custody.cjs"
    script.write_text(
        "const mod = require(%r);\n"
        "const events = [];\n"
        "const runtime = { exports: {\n"
        "  molt_runtime_execution_enter: () => { events.push('enter'); return 41n; },\n"
        "  molt_runtime_execution_leave: (token) => events.push(`leave:${token}`),\n"
        "} };\n"
        "try { mod.withRuntimeExecution(runtime, () => { events.push('body'); throw new Error('boom'); }); }\n"
        "catch (error) { if (error.message !== 'boom') throw error; }\n"
        "try { mod.withOwnedValue(99n, (value) => events.push(`release:${value}`), () => { throw new Error('decode'); }); }\n"
        "catch (error) { if (error.message !== 'decode') throw error; }\n"
        "console.log(JSON.stringify(events));\n"
        % str(repo_root / "wasm" / "run_wasm.js"),
        encoding="utf-8",
    )
    run = __import__("subprocess").run(
        ["node", str(script)],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
    )
    assert run.returncode == 0, run.stderr
    assert json.loads(run.stdout) == [
        "enter",
        "body",
        "leave:41",
        "release:99",
    ]
