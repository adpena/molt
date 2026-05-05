from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import uuid
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = REPO_ROOT / "tools" / "representation_report.py"


def _load_module() -> ModuleType:
    name = f"representation_report_{uuid.uuid4().hex}"
    spec = importlib.util.spec_from_file_location(name, MODULE_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _backend_report(
    *,
    values_by_repr: dict[str, int] | None = None,
    opcodes: dict[str, Any] | None = None,
) -> dict[str, Any]:
    values_by_repr = values_by_repr or {"i64": 3, "dynbox": 1}
    opcodes = opcodes or {
        "Add": {
            "total": 1,
            "result_reprs": {"i64": 1},
            "operand_repr_tuples": {"i64,i64": 1},
            "boxed_result_values": 0,
        }
    }
    return {
        "schema": "molt.typed_repr_report.v1",
        "verified": True,
        "functions": [
            {
                "name": "f",
                "blocks": 1,
                "passes": [],
                "stats": {
                    "values_by_repr": values_by_repr,
                    "values_by_type": {"i64": values_by_repr.get("i64", 0)},
                    "scalar_values": sum(
                        count
                        for repr_name, count in values_by_repr.items()
                        if repr_name != "dynbox"
                    ),
                    "boxed_values": values_by_repr.get("dynbox", 0),
                    "opcodes": opcodes,
                },
                "verification": {"lir_errors": [], "repr_violations": []},
            }
        ],
        "aggregate": {
            "functions": 1,
            "values_by_repr": values_by_repr,
            "values_by_type": {"i64": values_by_repr.get("i64", 0)},
            "scalar_values": sum(
                count
                for repr_name, count in values_by_repr.items()
                if repr_name != "dynbox"
            ),
            "boxed_values": values_by_repr.get("dynbox", 0),
            "lir_errors": 0,
            "repr_violations": 0,
            "opcodes": opcodes,
        },
    }


def test_analyze_file_delegates_to_backend_lir_reporter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    mod = _load_module()
    source = tmp_path / "sample.py"
    source.write_text(
        "def f(a: int, b: int) -> int:\n    return a + b\n", encoding="utf-8"
    )
    captured: dict[str, Any] = {}

    def fake_compile_to_tir(
        source_text: str, *, type_hint_policy: str
    ) -> dict[str, Any]:
        captured["source"] = source_text
        captured["type_hint_policy"] = type_hint_policy
        return {
            "functions": [{"name": "f", "ops": [{"kind": "add", "fast_int": True}]}]
        }

    def fake_run_backend_report(ir: dict[str, Any]) -> dict[str, Any]:
        captured["ir"] = ir
        return _backend_report()

    monkeypatch.setattr(mod, "compile_to_tir", fake_compile_to_tir)
    monkeypatch.setattr(mod, "run_backend_report", fake_run_backend_report)

    report = mod.analyze_file(source, "check")

    assert report.path == source
    assert captured["type_hint_policy"] == "check"
    assert captured["ir"]["functions"][0]["ops"][0]["fast_int"] is True
    assert report.report["aggregate"]["values_by_repr"]["i64"] == 3


def test_backend_command_uses_typed_repr_report_binary(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    mod = _load_module()
    captured: dict[str, Any] = {}

    def fake_run(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess[str]:
        captured["args"] = args
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(
            args[0], 0, stdout=json.dumps(_backend_report()), stderr=""
        )

    monkeypatch.setattr(mod.subprocess, "run", fake_run)

    report = mod.run_backend_report({"functions": []})

    command = captured["args"][0]
    assert command[:4] == ["cargo", "run", "--quiet", "--profile"]
    assert command[7:10] == ["--bin", "typed_repr_report", "--"]
    assert captured["kwargs"]["cwd"] == mod.REPO_ROOT
    assert json.loads(captured["kwargs"]["input"]) == {"functions": []}
    assert report["schema"] == "molt.typed_repr_report.v1"


def test_backend_failure_is_loud(monkeypatch: pytest.MonkeyPatch) -> None:
    mod = _load_module()

    def fake_run(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(args[0], 2, stdout="", stderr="bad LIR")

    monkeypatch.setattr(mod.subprocess, "run", fake_run)

    with pytest.raises(mod.BackendReportError, match="bad LIR"):
        mod.run_backend_report({"functions": []})


def test_json_output_aggregates_backend_reports(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    mod = _load_module()
    first = tmp_path / "first.py"
    second = tmp_path / "second.py"
    first.write_text("def f():\n    return 1\n", encoding="utf-8")
    second.write_text("def g():\n    return 2\n", encoding="utf-8")
    reports = [
        mod.FileReport(first, _backend_report(values_by_repr={"i64": 2})),
        mod.FileReport(
            second, _backend_report(values_by_repr={"bool1": 1, "dynbox": 1})
        ),
    ]

    monkeypatch.setattr(mod, "analyze_file", lambda path, type_hints: reports.pop(0))

    rc = mod.main([str(first), str(second), "--json"])

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["type_hints"] == "check"
    assert payload["aggregate"]["values_by_repr"] == {
        "bool1": 1,
        "dynbox": 1,
        "i64": 2,
    }
    assert payload["aggregate"]["scalar_values"] == 3
    assert payload["aggregate"]["boxed_values"] == 1
