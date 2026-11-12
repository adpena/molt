from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any

import pytest

from tools import representation_report as mod

# Cargo runs in this file are fakes; build capacity is not under test.
pytestmark = pytest.mark.usefixtures("admitted_build_capacity")


SCALAR_REPRS = {"raw_i64_safe", "raw_i64_full_deopt", "bool", "float_unboxed"}
BOXED_REPRS = {"maybe_bigint", "boxed_immortal", "dynbox"}


def _backend_report(
    *,
    values_by_repr: dict[str, int] | None = None,
    opcodes: dict[str, Any] | None = None,
) -> dict[str, Any]:
    values_by_repr = values_by_repr or {"raw_i64_safe": 3, "dynbox": 1}
    scalar_values = sum(
        count for name, count in values_by_repr.items() if name in SCALAR_REPRS
    )
    boxed_values = sum(
        count for name, count in values_by_repr.items() if name in BOXED_REPRS
    )
    opcodes = opcodes or {
        "Add": {
            "total": 1,
            "result_reprs": {"raw_i64_safe": 1},
            "operand_repr_tuples": {"raw_i64_safe,raw_i64_safe": 1},
            "boxed_result_values": 0,
        }
    }
    stats = {
        "values_by_repr": values_by_repr,
        "values_by_type": {"i64": values_by_repr.get("raw_i64_safe", 0)},
        "scalar_values": scalar_values,
        "boxed_values": boxed_values,
        "opcodes": opcodes,
    }
    return {
        "schema": "molt.typed_repr_report.v2",
        "verified": True,
        "functions": [
            {
                "name": "f",
                "blocks": 1,
                "passes": [],
                "stats": stats,
                "verification": {"tir_errors": []},
            }
        ],
        "aggregate": {"functions": 1, **stats, "tir_errors": 0},
    }


def test_analyze_file_delegates_to_backend_reporter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
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
        document = {
            "functions": [{"name": "f", "ops": [{"kind": "add", "type_hint": "int"}]}],
            "function_metadata": {"f": {"gpu_body_origin": {"role": "buffer_get"}}},
        }
        captured["frontend_ir"] = document
        return document

    def fake_run_backend_report(ir: dict[str, Any]) -> dict[str, Any]:
        captured["ir"] = ir
        return _backend_report()

    monkeypatch.setattr(mod, "compile_to_tir", fake_compile_to_tir)
    monkeypatch.setattr(mod, "run_backend_report", fake_run_backend_report)

    report = mod.analyze_file(source, "check")

    assert report.path == source
    assert captured["type_hint_policy"] == "check"
    assert captured["ir"]["functions"][0]["ops"][0]["type_hint"] == "int"
    assert "function_metadata" not in captured["ir"]
    assert "function_metadata" in captured["frontend_ir"]
    assert captured["ir"]["functions"] is captured["frontend_ir"]["functions"]
    assert report.report["aggregate"]["values_by_repr"]["raw_i64_safe"] == 3


def test_backend_command_uses_typed_repr_report_binary(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_run(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess[str]:
        captured["args"] = args
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(
            args[0], 0, stdout=json.dumps(_backend_report()), stderr=""
        )

    monkeypatch.setattr(mod.harness_memory_guard, "guarded_completed_process", fake_run)

    report = mod.run_backend_report({"functions": []})

    command = captured["args"][0]
    assert command[:4] == ["cargo", "run", "--quiet", "--profile"]
    assert command[7:10] == ["--bin", "typed_repr_report", "--"]
    assert captured["kwargs"]["cwd"] == mod.REPO_ROOT
    assert json.loads(captured["kwargs"]["input"]) == {"functions": []}
    assert report["schema"] == "molt.typed_repr_report.v2"


def test_backend_failure_is_loud(monkeypatch: pytest.MonkeyPatch) -> None:
    def fake_run(*args: Any, **kwargs: Any) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(args[0], 2, stdout="", stderr="bad TIR")

    monkeypatch.setattr(mod.harness_memory_guard, "guarded_completed_process", fake_run)

    with pytest.raises(mod.BackendReportError, match="bad TIR"):
        mod.run_backend_report({"functions": []})


def test_json_output_aggregates_backend_reports(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    first = tmp_path / "first.py"
    second = tmp_path / "second.py"
    first.write_text("def f():\n    return 1\n", encoding="utf-8")
    second.write_text("def g():\n    return 2\n", encoding="utf-8")
    reports = [
        mod.FileReport(first, _backend_report(values_by_repr={"raw_i64_safe": 2})),
        mod.FileReport(
            second,
            _backend_report(values_by_repr={"bool": 1, "maybe_bigint": 2, "dynbox": 1}),
        ),
    ]

    monkeypatch.setattr(mod, "analyze_file", lambda path, type_hints: reports.pop(0))

    rc = mod.main([str(first), str(second), "--json"])

    assert rc == 0
    payload = json.loads(capsys.readouterr().out)
    assert payload["type_hints"] == "check"
    assert payload["aggregate"]["values_by_repr"] == {
        "bool": 1,
        "dynbox": 1,
        "maybe_bigint": 2,
        "raw_i64_safe": 2,
    }
    assert payload["aggregate"]["scalar_values"] == 3
    assert payload["aggregate"]["boxed_values"] == 3
    assert payload["aggregate"]["tir_errors"] == 0


def test_text_output_reports_boxed_values(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "sample.py"
    source.write_text("def f():\n    return 1\n", encoding="utf-8")
    report = mod.FileReport(
        source,
        _backend_report(
            values_by_repr={"raw_i64_safe": 1, "maybe_bigint": 1, "dynbox": 1}
        ),
    )
    monkeypatch.setattr(mod, "analyze_file", lambda path, type_hints: report)

    rc = mod.main([str(source)])

    assert rc == 0
    output = capsys.readouterr().out
    assert "scalar_values=1" in output
    assert "boxed_values=2" in output
    assert "maybe_bigint" in output


def test_markdown_output_documents_maybe_bigint_as_boxed_lane(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "sample.py"
    source.write_text("def f():\n    return 1\n", encoding="utf-8")
    markdown = tmp_path / "report.md"
    report = mod.FileReport(
        source, _backend_report(values_by_repr={"raw_i64_safe": 1, "maybe_bigint": 1})
    )
    monkeypatch.setattr(mod, "analyze_file", lambda path, type_hints: report)

    rc = mod.main([str(source), "--markdown-out", str(markdown)])

    assert rc == 0
    text = markdown.read_text(encoding="utf-8")
    assert "<!-- GENERATED by tools/representation_report.py --update-doc." in text
    assert "| `maybe_bigint` | boxed |" in text
    assert "- scalar_values: `1`" in text
    assert "- boxed_values: `1`" in text


def test_update_doc_uses_default_generated_report_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    doc_path = tmp_path / "backend_representation.generated.md"
    monkeypatch.setattr(mod, "DEFAULT_MARKDOWN_PATH", doc_path)
    report = mod.FileReport(
        tmp_path / "sample.py",
        _backend_report(values_by_repr={"bool": 1, "maybe_bigint": 1}),
    )
    monkeypatch.setattr(mod, "analyze_file", lambda path, type_hints: report)

    rc = mod.main(["--update-doc"])

    assert rc == 0
    text = doc_path.read_text(encoding="utf-8")
    assert "- scalar_values: `1`" in text
    assert "- boxed_values: `1`" in text
