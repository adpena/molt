from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import pytest

import molt.cli as cli


def _tiny_ir() -> dict[str, Any]:
    return {
        "module": "__main__",
        "filename": "test.py",
        "ops": [],
        "functions": [],
        "classes": [],
        "constants": {},
        "imports": [],
    }


@pytest.fixture
def isolated_compiler_source(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Path:
    source = tmp_path / "runtime" / "molt-backend" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn marker() -> u8 { 1 }\n", encoding="utf-8")

    monkeypatch.setattr(
        cli,
        "_backend_source_paths",
        lambda root, backend_features: [source],
    )
    monkeypatch.setattr(cli, "_runtime_source_paths", lambda root: [])
    monkeypatch.setattr(cli, "_rustc_version", lambda: "rustc-test")
    monkeypatch.setattr(cli, "_cache_tooling_fingerprint", lambda: "tooling-test")
    return source


def test_cache_key_changes_when_compiler_source_content_changes_in_process(
    isolated_compiler_source: Path,
) -> None:
    first = cli._cache_key(_tiny_ir(), "native", None, "variant")

    isolated_compiler_source.write_text(
        "pub fn marker() -> u8 { 2 }\n",
        encoding="utf-8",
    )

    second = cli._cache_key(_tiny_ir(), "native", None, "variant")

    assert second != first


def test_cache_key_changes_when_compiler_source_mtime_changes_in_process(
    isolated_compiler_source: Path,
) -> None:
    first = cli._cache_key(_tiny_ir(), "native", None, "variant")

    stat = isolated_compiler_source.stat()
    next_mtime_ns = stat.st_mtime_ns + 5_000_000
    os.utime(isolated_compiler_source, ns=(next_mtime_ns, next_mtime_ns))

    second = cli._cache_key(_tiny_ir(), "native", None, "variant")

    assert second != first


def test_cache_tooling_fingerprint_changes_when_tooling_source_changes_in_process(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    cli_source = root / "src" / "molt" / "cli.py"
    frontend_source = root / "src" / "molt" / "frontend" / "__init__.py"
    cli_source.parent.mkdir(parents=True)
    frontend_source.parent.mkdir(parents=True)
    cli_source.write_text("CLI_MARKER = 1\n", encoding="utf-8")
    frontend_source.write_text("FRONTEND_MARKER = 1\n", encoding="utf-8")

    monkeypatch.setattr(cli, "Path", lambda _value: cli_source)

    first = cli._cache_tooling_fingerprint()

    cli_source.write_text("CLI_MARKER = 2\n", encoding="utf-8")

    second = cli._cache_tooling_fingerprint()

    assert second != first


def test_cache_tooling_fingerprint_tracks_frontend_helper_modules(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    cli_source = root / "src" / "molt" / "cli.py"
    frontend_init = root / "src" / "molt" / "frontend" / "__init__.py"
    cfg_analysis = root / "src" / "molt" / "frontend" / "cfg_analysis.py"
    tv_hooks = root / "src" / "molt" / "frontend" / "tv_hooks.py"
    type_facts = root / "src" / "molt" / "type_facts.py"
    for source in (cli_source, frontend_init, cfg_analysis, tv_hooks, type_facts):
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_text(f"{source.stem.upper()}_MARKER = 1\n", encoding="utf-8")

    monkeypatch.setattr(cli, "Path", lambda _value: cli_source)

    first = cli._cache_tooling_fingerprint()

    cfg_analysis.write_text("CFG_ANALYSIS_MARKER = 2\n", encoding="utf-8")
    tv_hooks.write_text("TV_HOOKS_MARKER = 2\n", encoding="utf-8")
    type_facts.write_text("TYPE_FACTS_MARKER = 2\n", encoding="utf-8")

    second = cli._cache_tooling_fingerprint()

    assert second != first


def test_cache_tooling_fingerprint_ignores_frontend_bytecode_cache(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    cli_source = root / "src" / "molt" / "cli.py"
    frontend_init = root / "src" / "molt" / "frontend" / "__init__.py"
    pycache = (
        root
        / "src"
        / "molt"
        / "frontend"
        / "__pycache__"
        / "cfg_analysis.cpython-312.pyc"
    )
    for source in (cli_source, frontend_init, pycache):
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_bytes(b"marker-1\n")

    monkeypatch.setattr(cli, "Path", lambda _value: cli_source)

    first = cli._cache_tooling_fingerprint()

    pycache.write_bytes(b"marker-2\n")

    second = cli._cache_tooling_fingerprint()

    assert second == first
