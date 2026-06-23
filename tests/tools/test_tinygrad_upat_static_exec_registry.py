from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

import pytest


ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / "tools" / "tinygrad_upat_static_exec_registry.py"


def _load_tool() -> ModuleType:
    spec = importlib.util.spec_from_file_location(
        "molt_test_tinygrad_upat_static_exec_registry", TOOL
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


MATCHER_SOURCE = """# match for ('tinygrad/uop/spec.py', 153)
def compiled_match(uop, ctx):
  if a0 == 7 and (_ret:=_fxn(x=uop, ctx=ctx)) is not None: return _ret
  return None"""


def test_rendered_registry_binds_matcher_globals_without_runtime_exec() -> None:
    tool = _load_tool()
    record = tool.MatcherRecord(
        source=MATCHER_SOURCE,
        globals_keys=("_fxn", "a0"),
    )
    registry_source = tool.render_registry_module([record])
    assert "def _factory_" in registry_source
    assert "exec(" not in registry_source
    namespace: dict[str, object] = {}

    exec(registry_source, namespace)
    runtime_locals: dict[str, object] = {}
    runtime_globals = {
        "_fxn": lambda *, x, ctx: ("matched", x, ctx),
        "a0": 7,
    }
    namespace["exec_static"](MATCHER_SOURCE, runtime_globals, runtime_locals)  # type: ignore[index]

    compiled = runtime_locals["compiled_match"]
    assert compiled("uop-value", "ctx-value") == ("matched", "uop-value", "ctx-value")


def test_rendered_registry_fails_closed_for_unknown_matcher_source() -> None:
    tool = _load_tool()
    record = tool.MatcherRecord(
        source=MATCHER_SOURCE,
        globals_keys=("_fxn", "a0"),
    )
    namespace: dict[str, object] = {}
    exec(tool.render_registry_module([record]), namespace)

    with pytest.raises(RuntimeError, match="MOLT_COMPAT_ERROR: static exec registry"):
        namespace["exec_static"]("# match for ('other.py', 1)\npass", {}, {})  # type: ignore[index]


def test_manifest_output_deduplicates_matcher_records(tmp_path: Path) -> None:
    tool = _load_tool()
    record = tool.MatcherRecord(
        source=MATCHER_SOURCE,
        globals_keys=("_fxn", "a0"),
    )
    manifest = tool.write_outputs(
        records=[record, record],
        manifest_output=tmp_path / "manifest.json",
        module_output=tmp_path / "_molt_static_exec_registry.py",
        suite_root=ROOT / "bench/friends/repos/tinygrad_off_the_shelf",
        workload="all",
        iterations=1,
    )

    assert manifest["record_count"] == 2
    assert manifest["unique_count"] == 1
    assert manifest["records"][0]["sha256"] == record.sha256
    generated = (tmp_path / "_molt_static_exec_registry.py").read_text()
    assert f"_factory_{record.sha256[:16]}" in generated


def test_capture_sweeps_loaded_tinygrad_pattern_matchers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    tool = _load_tool()
    calls: list[tuple[object, object]] = []

    class FakePatternMatcher:
        def __init__(self) -> None:
            self.patterns = [("pattern", "fxn")]

    tinygrad = ModuleType("tinygrad")
    uop = ModuleType("tinygrad.uop")
    ops = ModuleType("tinygrad.uop.ops")
    upat = ModuleType("tinygrad.uop.upat")
    loaded = ModuleType("tinygrad.loaded")
    matcher = FakePatternMatcher()
    loaded.matcher = matcher
    ops.PatternMatcher = FakePatternMatcher
    upat.upat_compile = lambda pattern, fxn: calls.append((pattern, fxn))

    monkeypatch.setitem(sys.modules, "tinygrad", tinygrad)
    monkeypatch.setitem(sys.modules, "tinygrad.uop", uop)
    monkeypatch.setitem(sys.modules, "tinygrad.uop.ops", ops)
    monkeypatch.setitem(sys.modules, "tinygrad.uop.upat", upat)
    monkeypatch.setitem(sys.modules, "tinygrad.loaded", loaded)

    assert tool._compile_loaded_tinygrad_pattern_matchers() == 1
    assert calls == [("pattern", "fxn")]


def test_cli_fails_closed_when_capture_produces_no_records(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    tool = _load_tool()

    def fake_capture(**_: object) -> tuple[int, list[object]]:
        return 0, []

    monkeypatch.setattr(tool, "_capture_tinygrad_upat_matchers", fake_capture)
    with pytest.raises(RuntimeError, match="MOLT_COMPAT_ERROR"):
        tool.main(
            [
                "--suite-root",
                str(ROOT / "bench/friends/repos/tinygrad_off_the_shelf"),
                "--manifest-output",
                str(tmp_path / "manifest.json"),
                "--module-output",
                str(tmp_path / "_molt_static_exec_registry.py"),
            ]
        )
