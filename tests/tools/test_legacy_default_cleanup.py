from __future__ import annotations

import importlib.util
import sys
import uuid
from pathlib import Path
from types import ModuleType

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]


def _load_tool_module(path: Path) -> ModuleType:
    name = f"{path.stem}_{uuid.uuid4().hex}"
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_browser_html_defaults_point_to_dist_outputs() -> None:
    browser_host = (REPO_ROOT / "wasm" / "browser_host.html").read_text(
        encoding="utf-8"
    )
    bench_pyodide = (REPO_ROOT / "wasm" / "bench_pyodide.html").read_text(
        encoding="utf-8"
    )

    assert 'id="wasm-url" value="../dist/output.wasm"' in browser_host
    assert 'id="linked-url" value="../dist/output_linked.wasm"' in browser_host
    assert 'id="molt-url" type="text" value="../dist/output_linked.wasm"' in (
        bench_pyodide
    )


def test_test_report_defaults_use_repo_local_reports_root(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    mod = _load_tool_module(REPO_ROOT / "tools" / "test_report.py")

    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    assert mod._reports_root() == REPO_ROOT / "tmp" / "molt_testing" / "test_reports"

    ext_root = tmp_path / "external"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))
    assert mod._reports_root() == ext_root / "test_reports"

    override = tmp_path / "custom" / "reports"
    assert mod._reports_root(str(override)) == override


def test_nightly_suite_defaults_use_repo_local_artifact_root(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    mod = _load_tool_module(REPO_ROOT / "tools" / "nightly_test_suite.py")

    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    expected_root = REPO_ROOT / "tmp" / "molt_testing"
    assert mod._ext_root() == expected_root
    assert mod._report_dir().parent == expected_root / "test_reports"
    assert mod._fuzz_results_dir() == expected_root / "fuzz_results"

    ext_root = tmp_path / "external"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))
    assert mod._ext_root() == ext_root


def test_mutation_defaults_use_repo_local_temp_and_target_roots(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    mod = _load_tool_module(REPO_ROOT / "tools" / "mutation_test.py")

    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)

    assert mod._temp_root() == REPO_ROOT / "tmp" / "mutation_tmp"
    assert mod._default_cargo_target_dir() == REPO_ROOT / "target"

    ext_root = tmp_path / "external"
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)

    assert mod._temp_root() == ext_root / "mutation_tmp"
    assert mod._default_cargo_target_dir() == ext_root / "target"


def test_wasm_strip_unused_defaults_output_next_to_input(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    mod = _load_tool_module(REPO_ROOT / "tools" / "wasm_strip_unused.py")

    wasm_path = tmp_path / "dist" / "sample.wasm"
    wasm_path.parent.mkdir(parents=True, exist_ok=True)
    wasm_path.write_bytes(b"\x00asm\x01\x00\x00\x00")

    captured: dict[str, Path] = {}

    class _DummyResult:
        file_size_bytes = 8

    monkeypatch.setattr(mod, "analyze", lambda path: _DummyResult())
    monkeypatch.setattr(
        mod,
        "strip_imports",
        lambda wasm, output, result: captured.setdefault("output", output),
    )
    monkeypatch.setattr(mod, "print_json", lambda result: None)
    monkeypatch.setattr(mod, "print_report", lambda result, verbose=False: None)
    monkeypatch.setattr(
        sys,
        "argv",
        ["tools/wasm_strip_unused.py", str(wasm_path), "--strip", "--json"],
    )

    mod.main()

    assert captured["output"] == wasm_path.with_name("sample-stripped.wasm")


def test_wasm_strip_unused_copy_fallback_publishes_atomically(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    mod = _load_tool_module(REPO_ROOT / "tools" / "wasm_strip_unused.py")

    wasm_path = tmp_path / "input.wasm"
    output_path = tmp_path / "output.wasm"
    wasm_path.write_bytes(b"\x00asm\x01\x00\x00\x00copy")
    monkeypatch.setattr(mod.shutil, "which", lambda _name: "/usr/bin/wasm-tools")
    original_publish = mod.artifact_publish.publish_validated_outputs
    published_sources: list[Path] = []

    def record_publish(pairs: list[tuple[Path, Path]]) -> None:
        assert len(pairs) == 1
        staged, final = pairs[0]
        assert final == output_path
        published_sources.append(staged)
        original_publish(pairs)

    monkeypatch.setattr(
        mod.artifact_publish,
        "publish_validated_outputs",
        record_publish,
    )

    class _NoStrippable:
        strippable_imports: list[object] = []

    assert mod.strip_imports(wasm_path, output_path, _NoStrippable()) == output_path

    assert output_path.read_bytes() == wasm_path.read_bytes()
    assert len(published_sources) == 1
    assert published_sources[0].name.startswith(".output.wasm.")
    assert published_sources[0].name.endswith(".tmp")
    assert list(tmp_path.glob(".*.tmp")) == []


def test_wasm_strip_unused_strip_writes_temp_before_final_publish(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    mod = _load_tool_module(REPO_ROOT / "tools" / "wasm_strip_unused.py")

    wasm_path = tmp_path / "input.wasm"
    output_path = tmp_path / "output.wasm"
    wasm_path.write_bytes(b"\x00asm\x01\x00\x00\x00input")
    monkeypatch.setattr(mod.shutil, "which", lambda _name: "/usr/bin/wasm-tools")
    monkeypatch.setattr(mod.harness_memory_guard, "limits_from_env", lambda _prefix: None)
    original_publish = mod.artifact_publish.publish_validated_outputs
    published_sources: list[Path] = []
    seen_commands: list[list[str]] = []

    class _Proc:
        returncode = 0
        stderr = ""

    def fake_guarded_completed_process(cmd: list[str], **_kwargs: object) -> _Proc:
        seen_commands.append(cmd)
        output_index = cmd.index("-o") + 1
        temp_output = Path(cmd[output_index])
        assert temp_output != output_path
        assert temp_output.name.startswith(".output.wasm.")
        temp_output.write_bytes(b"\x00asm\x01\x00\x00\x00stripped")
        return _Proc()

    def record_publish(pairs: list[tuple[Path, Path]]) -> None:
        assert len(pairs) == 1
        staged, final = pairs[0]
        assert final == output_path
        published_sources.append(staged)
        original_publish(pairs)

    monkeypatch.setattr(
        mod.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )
    monkeypatch.setattr(
        mod.artifact_publish,
        "publish_validated_outputs",
        record_publish,
    )

    class _Strippable:
        strippable_imports = [object()]

    assert mod.strip_imports(wasm_path, output_path, _Strippable()) == output_path

    assert seen_commands
    assert output_path.read_bytes() == b"\x00asm\x01\x00\x00\x00stripped"
    assert len(published_sources) == 1
    assert published_sources[0].name.startswith(".output.wasm.")
    assert published_sources[0].name.endswith(".tmp")
    assert list(tmp_path.glob(".*.tmp")) == []
