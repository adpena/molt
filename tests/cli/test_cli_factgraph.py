from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
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
            '{"schema_version":1,"kind":"molt_tir_fact_graph"}\n',
            encoding="utf-8",
        )
        return subprocess.CompletedProcess(cmd, 0, b"", b"")

    error = factgraph_module.execute_backend_fact_graph(
        request=factgraph_module.FactGraphRequest(
            output_path=output,
            function_name="molt_main",
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
