from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any, cast

import molt.cli as cli
from molt.cli import factgraph as factgraph_module


def test_factgraph_cli_dispatches_typed_backend_request(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "app.py"
    source.write_text("def main():\n    return 1\n", encoding="utf-8")
    output = tmp_path / "graph.json"
    captured: dict[str, Any] = {}

    def fake_build(*args: object, **kwargs: Any) -> int:
        assert args == ()
        captured.update(kwargs)
        return 0

    monkeypatch.setattr(cli, "build", fake_build)
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

    request = captured["fact_graph_request"]
    assert captured["file_path"] == str(source)
    assert captured["module"] is None
    assert captured["target"] == "native"
    assert captured["profile"] == "release"
    assert captured["cache"] is False
    assert captured["json_output"] is True
    assert captured["python_version"] == "3.12"
    assert "build_config" in captured
    assert os.environ["MOLT_BACKEND"] == "llvm"
    assert isinstance(request, factgraph_module.FactGraphRequest)
    assert request.output_path == output
    assert request.function_name == "main"
    assert request.requested_target == "llvm"
    assert request.effective_backend == "llvm"


def test_factgraph_cli_dispatches_module_entry(tmp_path: Path, monkeypatch) -> None:
    output = tmp_path / "module-graph.json"
    captured: dict[str, Any] = {}

    def fake_build(*args: object, **kwargs: Any) -> int:
        assert args == ()
        captured.update(kwargs)
        return 0

    monkeypatch.setattr(cli, "build", fake_build)
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

    request = captured["fact_graph_request"]
    assert captured["file_path"] is None
    assert captured["module"] == "pkg.app"
    assert captured["target"] == "native"
    assert captured["profile"] == "release"
    assert "build_config" in captured
    assert isinstance(request, factgraph_module.FactGraphRequest)
    assert request.output_path == output
    assert request.function_name == "entry"
    assert request.requested_target == "native"
    assert request.effective_backend == "cranelift"


def test_execute_backend_fact_graph_uses_target_prefix_and_ir_lease(
    tmp_path: Path,
) -> None:
    backend_bin = tmp_path / "molt-backend"
    ir_file = tmp_path / "backend-ir.json"
    output = tmp_path / "facts" / "molt_main.json"
    ir_file.write_text('{"functions":[]}\n', encoding="utf-8")
    seen: dict[str, object] = {}

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        seen["cmd"] = list(cmd)
        seen["env"] = dict(cast(dict[str, str], kwargs["env"]))
        seen["timeout"] = kwargs["timeout"]
        graph_output = Path(cmd[cmd.index("--fact-graph-output") + 1])
        graph_output.parent.mkdir(parents=True, exist_ok=True)
        graph_output.write_text(
            '{"schema_version":3,"kind":"molt_tir_fact_graph"}\n',
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    error = factgraph_module.execute_backend_fact_graph(
        request=factgraph_module.FactGraphRequest(
            output_path=output,
            function_name="molt_main",
            requested_target="wasm",
            effective_backend="cranelift",
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
        subprocess_output_text=cli._subprocess_output_text,
        fail=cli._fail,
        entry_override_env=cli.ENTRY_OVERRIDE_ENV,
    )

    assert error is None
    assert seen["cmd"] == [
        str(backend_bin),
        "--target",
        "wasm",
        "--ir-file",
        str(ir_file),
        "--fact-graph-output",
        str(output),
        "--fact-graph-function",
        "molt_main",
    ]
    env = cast(dict[str, str], seen["env"])
    assert env["MOLT_WASM_DATA_BASE"] == "67108864"
    assert env[cli.ENTRY_OVERRIDE_ENV] == "pkg.app"
    assert seen["timeout"] == 7.0
    assert output.is_file()


def test_emit_pipeline_fact_graph_reports_requested_target_and_backend(
    tmp_path: Path,
) -> None:
    output = tmp_path / "facts" / "main.json"
    ir_file = tmp_path / "backend-ir.json"
    ir_file.write_text('{"functions":[]}\n', encoding="utf-8")
    emitted: list[dict[str, Any]] = []
    cleaned: list[bool] = []

    def fake_prepare_backend_dispatch(**kwargs: object) -> tuple[Any, None]:
        assert kwargs["start_daemon"] is False
        return (
            SimpleNamespace(
                backend_bin=tmp_path / "molt-backend",
                backend_env={},
            ),
            None,
        )

    def fake_run(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        del kwargs
        graph_output = Path(cmd[cmd.index("--fact-graph-output") + 1])
        graph_output.parent.mkdir(parents=True, exist_ok=True)
        graph_output.write_text(
            '{"schema_version":3,"kind":"molt_tir_fact_graph"}\n',
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    def fake_emit_json(payload: dict[str, Any], json_output: bool) -> None:
        assert json_output is True
        emitted.append(payload)

    rc = factgraph_module.emit_pipeline_fact_graph(
        request=factgraph_module.FactGraphRequest(
            output_path=output,
            function_name="main",
            requested_target="llvm",
            effective_backend="llvm",
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
            runtime_state=object(),
            ensure_runtime_wasm_shared=lambda _modules: True,
            ensure_runtime_wasm_reloc=lambda _modules: True,
        ),
        build_config=SimpleNamespace(
            runtime_cargo_profile="release",
            cargo_timeout=None,
            backend_cargo_profile="release",
            backend_timeout=3.0,
        ),
        build_roots=SimpleNamespace(project_root=tmp_path, molt_root=tmp_path),
        build_preamble=SimpleNamespace(
            diagnostics_enabled=False,
            phase_starts={},
            backend_daemon_config_digest=None,
            warnings=[],
        ),
        ir={"functions": []},
        resolved_modules=frozenset(),
        json_output=True,
        verbose=False,
        target="native",
        entry_module="app",
        prepare_backend_dispatch=fake_prepare_backend_dispatch,
        ensure_backend_ir_file_path=lambda: ir_file,
        cleanup_backend_ir_file_path=lambda: cleaned.append(True),
        run_subprocess_captured_to_tempfiles=fake_run,
        subprocess_output_text=cli._subprocess_output_text,
        fail=cli._fail,
        emit_json=fake_emit_json,
        json_payload=cli._json_payload,
        entry_override_env=cli.ENTRY_OVERRIDE_ENV,
    )

    assert rc == 0
    assert cleaned == [True]
    assert len(emitted) == 1
    data = emitted[0]["data"]
    assert data["target"] == "llvm"
    assert data["backend"] == "llvm"
    assert data["pipeline_target"] == "native"
