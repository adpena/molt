from __future__ import annotations

from pathlib import Path

import pytest

from tools import check_core_lane_lowering as gate


def test_core_manifest_excludes_newer_syntax_before_parsing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    future = tmp_path / "future.py"
    future.write_text(
        "# MOLT_META: min_py=3.13\ntype Alias[T = int] = T\n", encoding="utf-8"
    )
    common = tmp_path / "common.py"
    common.write_text("import math\n", encoding="utf-8")
    manifest = tmp_path / "manifest.txt"
    manifest.write_text(f"{future}\n{common}\n", encoding="utf-8")
    monkeypatch.setattr(gate, "sys", type("Host", (), {"version_info": (3, 12)}))
    original_parse = gate.ast.parse

    def parse(source, *, filename):
        assert filename != str(future), "excluded source reached the parser"
        return original_parse(source, filename=filename)

    monkeypatch.setattr(gate.ast, "parse", parse)
    assert gate._collect_seed_modules(manifest, {"math"}) == {"math"}
    output = capsys.readouterr().out
    assert f"[SKIP] {future} (min_py 3.13)" in output
    assert "admitted=1 excluded=1" in output


def test_core_manifest_keeps_applicable_syntax_errors_visible(tmp_path: Path) -> None:
    source = tmp_path / "broken.py"
    source.write_text("# MOLT_META: min_py=3.12\ndef broken(\n", encoding="utf-8")
    manifest = tmp_path / "manifest.txt"
    manifest.write_text(f"{source}\n", encoding="utf-8")
    with pytest.raises(SyntaxError):
        gate._collect_seed_modules(manifest, set())
