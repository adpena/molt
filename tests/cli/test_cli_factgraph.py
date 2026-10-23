from __future__ import annotations

from molt.compilation_target import admit_target

import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any, cast

import msgpack
import pytest

import molt.cli as cli
from molt.backend_environment import CodegenSelection
from molt.cli import factgraph as factgraph_module
from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.cli import config_resolution as cli_config_resolution
from molt.cli import output as cli_output
from molt.cli import build_command as cli_build_command
from molt.cli.build_request import BuildRequest, FactGraphRequest


@pytest.mark.parametrize("mutation", [None, "rewrite", "delete"])
def test_factgraph_source_fence_preserves_previous_publication(
    tmp_path: Path, mutation: str | None
) -> None:
    from molt.cli.module_source import _ModuleSourceCatalog, _ModuleSourceLease

    source = tmp_path / "app.py"
    source.write_text("value = 1\n", encoding="utf-8")
    catalog = _ModuleSourceCatalog({"app": _ModuleSourceLease.path_backed(source)})
    output = tmp_path / "graph.json"
    output.write_bytes(b"previous graph")
    ir_file = tmp_path / "input.msgpack"
    ir_file.write_bytes(msgpack.packb({"functions": []}, use_bin_type=True))
    stages: list[Path] = []

    def backend(command, **kwargs):
        assert command[command.index("--ir-format") + 1] == "msgpack"
        lease = Path(command[command.index("--ir-file") + 1])
        assert msgpack.unpackb(lease.read_bytes(), raw=False) == {"functions": []}
        candidate = Path(command[command.index("--fact-graph-output") + 1])
        assert candidate != output
        assert output.read_bytes() == b"previous graph"
        candidate.write_bytes(b'{"kind":"molt_tir_fact_graph"}')
        stages.append(candidate)
        if mutation == "rewrite":
            source.write_text("value = 2\n", encoding="utf-8")
        elif mutation == "delete":
            source.unlink()
        return subprocess.CompletedProcess(command, 0, b"", b"")

    result = factgraph_module.execute_backend_fact_graph(
        request=FactGraphRequest(output, "main", "native"),
        is_luau_transpile=False,
        is_rust_transpile=False,
        is_wasm=False,
        target_triple=None,
        json_output=True,
        verbose=False,
        backend_bin=tmp_path / "backend",
        backend_env={},
        backend_timeout=1.0,
        entry_module="app",
        ensure_backend_ir_file_path=lambda: ir_file,
        run_subprocess_captured_to_tempfiles=backend,
        subprocess_output_text=cli_output.subprocess_output_text,
        fail=cli_output.fail,
        entry_override_env=cli_config_resolution.ENTRY_OVERRIDE_ENV,
        finalize_inputs=catalog.validate,
    )
    assert result is None if mutation is None else result == 2
    assert output.read_bytes() == (
        b'{"kind":"molt_tir_fact_graph"}' if mutation is None else b"previous graph"
    )
    assert len(stages) == 1 and not stages[0].exists()


def test_factgraph_cli_dispatches_typed_backend_request(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "app.py"
    source.write_text("def main():\n    return 1\n", encoding="utf-8")
    output = tmp_path / "graph.json"
    captured: list[BuildRequest] = []

    def fake_build(request: BuildRequest) -> int:
        captured.append(request)
        return 0

    monkeypatch.setattr(cli_build_command, "build", fake_build)
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.delenv("MOLT_BACKEND", raising=False)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "molt",
            "factgraph",
            str(source),
            "main",
            "--output",
            str(output),
            "--target",
            "llvm",
            "--profile",
            "release",
            "--python-version",
            "3.12",
            "--json",
        ],
    )

    assert cli.main() == 0

    [build_request] = captured
    request = build_request.fact_graph_request
    assert build_request.file_path == str(source)
    assert build_request.module is None
    assert build_request.target == admit_target("native")
    assert build_request.profile == "release"
    assert build_request.cache is False
    assert build_request.json_output is True
    assert build_request.python_version == "3.12"
    assert build_request.build_config is not None
    # The selection is a typed parameter; the process environment never
    # carries it into later in-process work (HF-60).
    assert build_request.codegen_backend == "llvm"
    assert "MOLT_BACKEND" not in os.environ
    assert isinstance(request, FactGraphRequest)
    assert request.output_path == output
    assert request.function_name == "main"
    assert request.requested_target == "llvm"


def test_factgraph_cli_dispatches_module_entry(tmp_path: Path, monkeypatch) -> None:
    output = tmp_path / "module-graph.json"
    captured: list[BuildRequest] = []

    def fake_build(request: BuildRequest) -> int:
        captured.append(request)
        return 0

    monkeypatch.setattr(cli_build_command, "build", fake_build)
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "molt",
            "factgraph",
            "--module",
            "pkg.app",
            "entry",
            "--output",
            str(output),
        ],
    )

    assert cli.main() == 0

    [build_request] = captured
    request = build_request.fact_graph_request
    assert build_request.file_path is None
    assert build_request.module == "pkg.app"
    assert build_request.target == admit_target("native")
    assert build_request.profile == "release"
    assert build_request.build_config is not None
    assert isinstance(request, FactGraphRequest)
    assert request.output_path == output
    assert request.function_name == "entry"
    assert request.requested_target == "native"
    assert build_request.codegen_backend == "cranelift"


# A named output need not carry an extension; its stage still needs a name.
@pytest.mark.parametrize("output_name", ["molt_main.json", "molt_main"])
def test_execute_backend_fact_graph_uses_target_prefix_and_ir_lease(
    tmp_path: Path, output_name: str
) -> None:
    backend_bin = tmp_path / "molt-backend"
    ir_file = tmp_path / "backend-ir.msgpack"
    output = tmp_path / "facts" / output_name
    ir_file.write_bytes(msgpack.packb({"functions": []}, use_bin_type=True))
    seen: dict[str, object] = {}

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        assert msgpack.unpackb(ir_file.read_bytes(), raw=False) == {"functions": []}
        seen["cmd"] = list(cmd)
        seen["env"] = dict(cast(dict[str, str], kwargs["env"]))
        seen["timeout"] = kwargs["timeout"]
        graph_output = Path(cmd[cmd.index("--fact-graph-output") + 1])
        seen["candidate"] = graph_output
        graph_output.parent.mkdir(parents=True, exist_ok=True)
        graph_output.write_text(
            '{"schema_version":3,"kind":"molt_tir_fact_graph"}\n',
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    error = factgraph_module.execute_backend_fact_graph(
        request=FactGraphRequest(
            output_path=output,
            function_name="molt_main",
            requested_target="wasm",
        ),
        is_luau_transpile=False,
        is_rust_transpile=False,
        is_wasm=True,
        target_triple=None,
        json_output=False,
        verbose=True,
        backend_bin=backend_bin,
        backend_env={"MOLT_WASM_DATA_BASE": "67108864"},
        backend_timeout=7.0,
        entry_module="pkg.app",
        ensure_backend_ir_file_path=lambda: ir_file,
        run_subprocess_captured_to_tempfiles=fake_run,
        subprocess_output_text=cli_output.subprocess_output_text,
        fail=cli_output.fail,
        entry_override_env=cli_config_resolution.ENTRY_OVERRIDE_ENV,
    )

    assert error is None
    assert seen["cmd"] == [
        str(backend_bin),
        "--target",
        "wasm",
        "--ir-format",
        "msgpack",
        "--ir-file",
        str(ir_file),
        "--fact-graph-output",
        str(seen["candidate"]),
        "--fact-graph-function",
        "molt_main",
    ]
    env = cast(dict[str, str], seen["env"])
    assert env["MOLT_WASM_DATA_BASE"] == "67108864"
    assert env[cli_config_resolution.ENTRY_OVERRIDE_ENV] == "pkg.app"
    assert seen["timeout"] == 7.0
    assert output.is_file()
    candidate = seen["candidate"]
    assert isinstance(candidate, Path)
    assert not candidate.exists()


def test_backend_command_prefix_uses_canonical_split_table_boundary(
    tmp_path: Path,
) -> None:
    backend_bin = tmp_path / "molt-backend"

    assert factgraph_module.backend_command_prefix(
        backend_bin=backend_bin,
        is_luau_transpile=False,
        is_rust_transpile=False,
        is_wasm=True,
        target_triple=None,
        wasm_link=True,
        wasm_data_base=67_108_864,
        wasm_table_base=1,
        wasm_split_runtime_app_table_base=8_192,
        wasm_runtime_poll_slots=6,
    ) == [
        str(backend_bin),
        "--target",
        "wasm",
        "--wasm-link",
        "--wasm-data-base",
        "67108864",
        "--wasm-table-base",
        "1",
        "--wasm-split-runtime-app-table-base",
        "8192",
        "--wasm-runtime-poll-slots",
        "6",
    ]


@pytest.mark.parametrize("custody_failure", [False, True])
def test_emit_pipeline_fact_graph_reports_requested_target_and_backend(
    tmp_path: Path,
    monkeypatch,
    custody_failure: bool,
    capsys,
) -> None:
    from molt.cli import backend_compile
    from tests.cli.native_link_test_support import (
        native_codegen_binding,
        write_test_native_link_manifest,
        write_test_static_archive,
    )

    output = tmp_path / "facts" / "main.json"
    ir_file = tmp_path / "backend-ir.msgpack"
    ir_file.write_bytes(msgpack.packb({"functions": []}, use_bin_type=True))
    emitted: list[dict[str, Any]] = []
    cleaned: list[bool] = []
    produced: list[Path] = []

    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_bytes(b"admitted compiler fixture")
    runtime_lib = tmp_path / "runtime.a"
    write_test_static_archive(runtime_lib)
    binding = native_codegen_binding(
        runtime_lib, write_test_native_link_manifest(runtime_lib)
    )
    monkeypatch.setenv("MOLT_RUNTIME_CALLABLE_SYMBOLS", "foreign-operation")
    monkeypatch.setenv("MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256", "0" * 64)
    monkeypatch.setenv("MOLT_STDLIB_OBJ", str(tmp_path / "foreign-stdlib.o"))

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        env = kwargs["env"]
        # The backend process gets the selection in its own mapping, while
        # the caller's environment never holds it.
        assert env["MOLT_BACKEND"] == "llvm"
        assert "MOLT_BACKEND" not in os.environ
        assert env["MOLT_RUNTIME_CALLABLE_SYMBOLS"] == str(
            binding.callable_symbols.path
        )
        assert (
            env["MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256"]
            == binding.callable_symbols.sha256
        )
        # The operation's projection, not the caller's ambient partition.
        assert "MOLT_STDLIB_OBJ" not in env
        graph_output = Path(cmd[cmd.index("--fact-graph-output") + 1])
        produced.append(graph_output)
        graph_output.parent.mkdir(parents=True, exist_ok=True)
        graph_output.write_text(
            '{"schema_version":3,"kind":"molt_tir_fact_graph"}\n',
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    def fake_emit_json(payload: dict[str, Any], json_output: bool) -> None:
        assert json_output is True
        emitted.append(payload)

    closed: list[bool] = []

    def finalize_inputs() -> None:
        assert produced[0].is_file(), "backend must finish before input custody closes"
        assert not output.exists(), "input custody must close before publication"
        closed.append(True)
        if custody_failure:
            raise ValueError("owned producer did not exit")

    rc = factgraph_module.emit_pipeline_fact_graph(
        request=FactGraphRequest(
            output_path=output,
            function_name="main",
            requested_target="llvm",
        ),
        output_layout=SimpleNamespace(
            is_rust_transpile=False,
            is_luau_transpile=False,
            is_wasm=False,
            split_runtime=False,
            linked=False,
            target_triple=None,
        ),
        deterministic=True,
        profile="release",
        runtime_context=SimpleNamespace(
            runtime_state=SimpleNamespace(native_runtime_codegen_binding=binding),
            ensure_runtime_wasm_both=lambda _modules: True,
            backend_bin=backend_bin,
            backend_compiler_fingerprint=None,
        ),
        build_config=SimpleNamespace(
            runtime_cargo_profile="release",
            cargo_timeout=None,
            backend_cargo_profile="release",
            backend_timeout=3.0,
            codegen=CodegenSelection(backend="llvm"),
        ),
        build_roots=SimpleNamespace(project_root=tmp_path, molt_root=tmp_path),
        build_preamble=SimpleNamespace(
            diagnostics_enabled=False,
            phase_starts={},
            backend_daemon_config_digest=None,
            warnings=[],
            rust_cpu=RustCpuPolicy.BASELINE,
        ),
        json_output=True,
        verbose=False,
        target=admit_target("native"),
        entry_module="app",
        prepare_backend_dispatch=backend_compile._prepare_backend_dispatch,
        ensure_backend_ir_file_path=lambda: ir_file,
        cleanup_backend_ir_file_path=lambda: cleaned.append(True),
        run_subprocess_captured_to_tempfiles=fake_run,
        subprocess_output_text=cli_output.subprocess_output_text,
        fail=cli_output.fail,
        emit_json=fake_emit_json,
        json_payload=cli_output.json_payload,
        entry_override_env=cli_config_resolution.ENTRY_OVERRIDE_ENV,
        finalize_inputs=finalize_inputs,
    )

    assert closed == [True]
    assert cleaned == [True]
    if custody_failure:
        import json

        assert rc == 2
        assert emitted == []
        payload = json.loads(capsys.readouterr().out)
        assert payload["status"] == "error"
        assert payload["errors"] == [
            "Build input custody failed to close: owned producer did not exit"
        ]
        return
    assert rc == 0
    assert cleaned == [True]
    assert len(emitted) == 1
    data = emitted[0]["data"]
    assert data["target"] == "llvm"
    assert data["backend"] == "llvm"
    assert data["pipeline_target"] == "native"
