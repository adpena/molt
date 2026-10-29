from __future__ import annotations

from tests.wasm_execution_manifest import wasm_execution_launch

from molt._wasm_abi_generated import WASM_EXECUTION_MANIFEST_VERSION

import hashlib
import json
from pathlib import Path

import pytest

import tools.bench_wasm as bench_wasm
from molt.node_runtime import NodeRuntime
from molt.cli.wasm_host import molt_wasm_host_exe_name


@pytest.mark.parametrize(
    "outcome,relative",
    [
        ("prebuilt", False),
        ("explicit", False),
        ("missing-explicit", False),
        ("built", False),
        ("built", True),
        ("missing-output", False),
        ("build-failed", False),
    ],
)
def test_wasmtime_runner_uses_one_selected_host_build(
    monkeypatch, tmp_path: Path, outcome: str, relative: bool
) -> None:
    root = tmp_path / "source"
    target = (root if relative else tmp_path) / "selected target"
    selected = "selected target" if relative else str(target)
    host = target / "release" / molt_wasm_host_exe_name()
    decoy = root / "target" / "release" / molt_wasm_host_exe_name()
    decoy.parent.mkdir(parents=True)
    decoy.write_bytes(b"unselected host")
    monkeypatch.setattr(bench_wasm, "REPO_ROOT", root)
    monkeypatch.setenv("CARGO_TARGET_DIR", selected)
    monkeypatch.delenv("MOLT_WASM_HOST_BIN", raising=False)
    if outcome in {"prebuilt", "explicit"}:
        host.parent.mkdir(parents=True)
        host.write_bytes(b"selected host")
    if outcome in {"explicit", "missing-explicit"}:
        monkeypatch.setenv("MOLT_WASM_HOST_BIN", str(host))
    calls = []

    def build(command, **kwargs):
        calls.append((command, kwargs["env"]["CARGO_TARGET_DIR"], kwargs["cwd"]))
        if outcome == "built":
            host.parent.mkdir(parents=True)
            host.write_bytes(b"newly built host")
        return bench_wasm._RunResult(
            returncode=1 if outcome == "build-failed" else 0,
            stderr="deliberate build failure",
        )

    monkeypatch.setattr(bench_wasm, "_run_cmd", build)
    if outcome in {"missing-explicit", "missing-output", "build-failed"}:
        with pytest.raises(
            RuntimeError,
            match={
                "missing-explicit": "MOLT_WASM_HOST_BIN",
                "missing-output": "selected target after build",
                "build-failed": "deliberate build failure",
            }[outcome],
        ):
            bench_wasm._resolve_runner(
                "wasmtime", tty=False, log=None, node_max_old_space_mb=None
            )
    else:
        assert bench_wasm._resolve_runner(
            "wasmtime", tty=False, log=None, node_max_old_space_mb=None
        ) == [str(host)]
    assert len(calls) == (
        1 if outcome in {"built", "missing-output", "build-failed"} else 0
    )
    if calls:
        assert calls[0] == (
            ["cargo", "build", "--locked", "--release", "--package", "molt-wasm-host"],
            selected,
            root,
        )


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
                "version": WASM_EXECUTION_MANIFEST_VERSION,
                "launch": wasm_execution_launch(),
                "mode": "linked",
                "modules": {"linked": _module_descriptor(linked, manifest)},
            }
        ),
        encoding="utf-8",
    )


def test_resolve_runner_node_enforces_stable_wasm_flags(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    node_path = tmp_path / "node runtime"
    monkeypatch.setattr(
        bench_wasm,
        "resolve_node_runtime",
        lambda **kwargs: NodeRuntime(node_path, "24.0.0", 24),
    )
    monkeypatch.delenv("MOLT_WASM_NODE_OPTIONS", raising=False)
    cmd = bench_wasm._resolve_runner(
        "node", tty=False, log=None, node_max_old_space_mb=None
    )
    assert cmd[0] == str(node_path)
    assert "--no-warnings" in cmd
    assert "--no-wasm-tier-up" in cmd
    assert "--no-wasm-dynamic-tiering" in cmd
    assert "--wasm-num-compilation-tasks=1" in cmd
    assert cmd[-1] == str(bench_wasm.REPO_ROOT / "wasm" / "run_wasm.js")


def test_python_cmd_reuses_active_harness_interpreter() -> None:
    assert bench_wasm._python_cmd() == [bench_wasm.sys.executable]


@pytest.mark.parametrize("keep_temp", [False, True])
def test_prepare_wasm_binary_sets_linked_table_base(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    keep_temp: bool,
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
        molt_args,
        tty: bool,
        log,
        limits=None,
        use_molt_build_cache=True,
        source_cwd=None,
    ) -> float:
        del tty, log, limits, use_molt_build_cache
        assert molt_args == []
        assert source_cwd is None
        captured_env.update(env)
        _write_linked_build(output_path)
        return 0.01

    monkeypatch.setattr(bench_wasm, "_build_wasm_output", _fake_build)

    wasm = bench_wasm.prepare_wasm_binary(
        "tests/benchmarks/bench_sum.py",
        molt_args=[],
        tty=False,
        log=None,
        keep_temp=keep_temp,
    )
    assert wasm is not None
    assert pruned_envs == [base_env]
    assert "MOLT_WASM_RELOCATABLE" not in captured_env
    assert captured_env.get("MOLT_WASM_TABLE_BASE") == "2354"
    from molt.temporary_artifacts import OwnedTemporaryDirectory

    assert type(wasm.temp_dir) is OwnedTemporaryDirectory
    wasm.temp_dir.cleanup()


@pytest.mark.parametrize(
    "failure",
    ["build", "interrupt", "refusal", "missing", "manifest", "stat", "metrics"],
)
@pytest.mark.parametrize("cleanup_fails", [False, True])
def test_prepare_wasm_binary_owns_every_failed_preparation(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    failure: str,
    cleanup_fails: bool,
) -> None:
    from molt.temporary_artifacts import OwnedTemporaryDirectory

    owners = []
    cleaned = []
    build_calls = []
    primary = (
        KeyboardInterrupt("build interrupted")
        if failure == "interrupt"
        else ValueError("independent preparation failure")
    )
    cleanup_error = OSError("independent cleanup refusal")
    original_cleanup = OwnedTemporaryDirectory.cleanup
    original_linked = bench_wasm._linked_wasm_output

    def allocate(*, prefix):
        owner = OwnedTemporaryDirectory(prefix=prefix, dir=tmp_path)
        owners.append(owner)
        return owner

    def cleanup(owner):
        cleaned.append(owner)
        if cleanup_fails:
            raise cleanup_error
        original_cleanup(owner)

    def build(_python, _env, output, _script, **kwargs):
        build_calls.append(output)
        (output.parent / "preparation-evidence.txt").write_text(
            "retain on refusal", encoding="utf-8"
        )
        if failure in {"build", "interrupt"}:
            raise primary
        if failure == "refusal":
            bench_wasm._LAST_BUILD_FAILURE_DETAIL = "independent build refusal"
            return None
        if failure == "missing":
            output.write_bytes(b"\0asm\x01\0\0\0")
        else:
            _write_linked_build(output)
            if failure == "manifest":
                bench_wasm.wasm_runtime_manifest_path(
                    output.with_name("output_linked.wasm")
                ).write_text("not a manifest", encoding="utf-8")
        return 0.01

    def linked(output):
        path = original_linked(output)
        if failure == "stat":
            assert path is not None
            path.unlink()  # Real disappearance between admission and size observation.
        return path

    def metrics(_path):
        if failure == "metrics":
            raise primary
        pytest.fail("failed preparation reached import metrics")

    monkeypatch.setattr(bench_wasm, "OwnedTemporaryDirectory", allocate)
    monkeypatch.setattr(OwnedTemporaryDirectory, "cleanup", cleanup)
    monkeypatch.setattr(bench_wasm, "_base_env", lambda: {"MOLT_WASM_TABLE_BASE": "0"})
    monkeypatch.setattr(bench_wasm, "_prune_backend_daemons", lambda _: None)
    monkeypatch.setattr(bench_wasm, "_python_cmd", lambda: ["selected-python"])
    monkeypatch.setattr(bench_wasm, "_build_wasm_output", build)
    monkeypatch.setattr(bench_wasm, "_linked_wasm_output", linked)
    monkeypatch.setattr(bench_wasm, "_read_wasm_import_metrics", metrics)

    def prepare():
        return bench_wasm.prepare_wasm_binary(
            "program.py",
            molt_args=[],
            tty=False,
            log=None,
            keep_temp=False,
            limits=object(),
        )

    try:
        if failure == "refusal" and not cleanup_fails:
            assert prepare() is None
        else:
            expected_type = (
                OSError
                if failure == "refusal"
                else RuntimeError
                if failure in {"missing", "manifest"}
                else FileNotFoundError
                if failure == "stat"
                else type(primary)
            )
            with pytest.raises(expected_type) as caught:
                prepare()
            if failure in {"build", "interrupt", "metrics"}:
                assert caught.value is primary
            elif failure in {"missing", "manifest"}:
                assert str(caught.value) == "linked wasm required"
            if cleanup_fails:
                assert caught.value.benchmark_temporary_directories == tuple(owners)
                assert any(
                    (
                        "independent build refusal"
                        if failure == "refusal"
                        else "independent cleanup refusal"
                    )
                    in note
                    for note in caught.value.__notes__
                )
        assert len(build_calls) == len(owners) == 1
        assert cleaned == owners
        root = Path(owners[0].name)
        assert root.exists() is cleanup_fails
        if cleanup_fails:
            assert (root / "preparation-evidence.txt").read_text(
                encoding="utf-8"
            ) == "retain on refusal"
    finally:
        for owner in owners:
            original_cleanup(owner)


def test_build_wasm_output_reuses_molt_build_cache_by_default(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    output_path = tmp_path / "output.wasm"
    commands: list[list[str]] = []
    monkeypatch.setattr(bench_wasm, "molt_args_for_benchmark", lambda _script: [])

    def fake_run_cmd(
        command, *, env, capture, tty, log, timeout_s=None, limits=None, cwd=None
    ):
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
            molt_args=[],
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

    def fake_run_cmd(
        command, *, env, capture, tty, log, timeout_s=None, limits=None, cwd=None
    ):
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
            molt_args=[],
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
                "version": WASM_EXECUTION_MANIFEST_VERSION,
                "launch": wasm_execution_launch(),
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
        ),
        encoding="utf-8",
    )

    run = __import__("subprocess").run(
        ["node", str(script)],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
        encoding="utf-8",
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
        ),
        encoding="utf-8",
    )

    run = __import__("subprocess").run(
        ["node", str(script)],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
        encoding="utf-8",
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
                "version": WASM_EXECUTION_MANIFEST_VERSION,
                "launch": wasm_execution_launch(),
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
        ),
        encoding="utf-8",
    )

    run = __import__("subprocess").run(
        ["node", str(script)],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
        encoding="utf-8",
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
                "version": WASM_EXECUTION_MANIFEST_VERSION,
                "launch": wasm_execution_launch(),
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
        ),
        encoding="utf-8",
    )

    run = __import__("subprocess").run(
        ["node", str(script)],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
        encoding="utf-8",
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
        "const {withRuntimeOwnedValues} = require(%r);\n"
        "const events = [];\n"
        "const runtime = { exports: {\n"
        "  molt_exception_pending: () => 0n,\n"
        "  molt_runtime_execution_enter: () => { events.push('enter'); return 41n; },\n"
        "  molt_runtime_execution_leave: (token) => events.push(`leave:${token}`),\n"
        "  molt_runtime_shutdown: () => 1n,\n"
        "  molt_dec_ref_obj: (value) => { events.push(`release:${value}`); throw new Error('release'); },\n"
        "} };\n"
        "try { mod.withRuntimeExecution(runtime, () => { events.push('body'); throw new Error('boom'); }); }\n"
        "catch (error) { if (error.message !== 'boom') throw error; }\n"
        "try { withRuntimeOwnedValues(runtime, [99n], value => value, () => { throw new Error('decode'); }); }\n"
        "catch (error) { if (!(error instanceof AggregateError) || error.cause !== error.errors[0] ||\n"
        "  error.errors[0].message !== 'decode' || error.errors[1].message !== 'release') throw error; }\n"
        "console.log(JSON.stringify(events));\n"
        % (
            str(repo_root / "wasm" / "run_wasm.js"),
            str(repo_root / "wasm" / "runtime_lifecycle.js"),
        ),
        encoding="utf-8",
    )
    run = __import__("subprocess").run(
        ["node", str(script)],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
        encoding="utf-8",
    )
    assert run.returncode == 0, run.stderr
    assert json.loads(run.stdout) == [
        "enter",
        "body",
        "leave:41",
        "release:99",
    ]
