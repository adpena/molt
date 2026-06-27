from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
from types import ModuleType


REPO_ROOT = Path(__file__).resolve().parents[2]
TOOL_PATH = REPO_ROOT / "tools" / "check_runtime_bridge_test_stubs.py"


def _load_tool() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "molt_tools_check_runtime_bridge_test_stubs",
        TOOL_PATH,
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _write_fixture(root: Path, *, stub_body: str = "") -> Path:
    core = root / "runtime" / "molt-runtime-core" / "src"
    core.mkdir(parents=True)
    (core / "lib.rs").write_text(
        """
unsafe extern "C" {
    pub fn molt_core_symbol(value: u64) -> u64;
}
""",
        encoding="utf-8",
    )

    satellite = root / "runtime" / "molt-runtime-compression" / "src"
    satellite.mkdir(parents=True)
    (satellite / "bridge.rs").write_text(
        """
extern "C" {
    pub fn molt_bridge_plain(value: i64) -> u64;
}

unsafe extern "C" {
    fn molt_bridge_unsafe(ptr: *mut u8);
}
""",
        encoding="utf-8",
    )

    stub = core / "bridge_test_stubs.rs"
    stub.write_text(stub_body, encoding="utf-8")
    return stub


def _point_tool_at(tool: ModuleType, monkeypatch, root: Path, stub: Path) -> None:
    monkeypatch.setattr(tool, "REPO_ROOT", root)
    monkeypatch.setattr(tool, "STUB_PATH", stub)


def test_parser_covers_core_plain_and_unsafe_externs(monkeypatch, tmp_path):
    tool = _load_tool()
    stub = _write_fixture(tmp_path)
    _point_tool_at(tool, monkeypatch, tmp_path, stub)

    assert tool.declared_bridge_symbols() == [
        "molt_bridge_plain",
        "molt_bridge_unsafe",
        "molt_core_symbol",
    ]


def test_fix_writes_canonical_stub(monkeypatch, tmp_path):
    tool = _load_tool()
    stub = _write_fixture(tmp_path)
    _point_tool_at(tool, monkeypatch, tmp_path, stub)

    assert tool.main(["--fix"]) == 0
    assert tool.main([]) == 0
    body = stub.read_text(encoding="utf-8")
    assert "aborting_bridge_stub!(molt_bridge_plain);" in body
    assert "aborting_bridge_stub!(molt_bridge_unsafe);" in body
    assert "aborting_bridge_stub!(molt_core_symbol);" in body


def test_missing_stub_fails_closed(monkeypatch, tmp_path, capsys):
    tool = _load_tool()
    stub = _write_fixture(
        tmp_path,
        stub_body="aborting_bridge_stub!(molt_core_symbol);\n",
    )
    _point_tool_at(tool, monkeypatch, tmp_path, stub)

    assert tool.main([]) == 1
    output = capsys.readouterr().out
    assert "missing bridge test stubs" in output
    assert "molt_bridge_plain" in output
    assert "molt_bridge_unsafe" in output


def test_live_stub_file_is_in_sync():
    tool = _load_tool()

    assert tool.main([]) == 0
    symbols = tool.declared_bridge_symbols()
    assert "molt_bridge_alloc_bytes" in symbols
    assert "molt_bridge_runtime_state_get_or_init" in symbols
