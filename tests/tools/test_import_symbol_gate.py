"""Teeth for tools/import_symbol_gate.py: static first-party from-import resolution."""

from __future__ import annotations

from pathlib import Path

import pytest

from tools import import_symbol_gate as gate


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")
    return path


@pytest.fixture
def repo(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """A miniature repository layout the gate resolves against."""
    monkeypatch.setattr(gate, "ROOT", tmp_path)
    monkeypatch.setattr(
        gate,
        "SOURCE_ROOTS",
        (("molt", tmp_path / "src" / "molt"), ("tools", tmp_path / "tools")),
    )
    monkeypatch.setattr(
        gate, "SCAN_DIRS", (tmp_path / "src", tmp_path / "tools", tmp_path / "tests")
    )
    _write(tmp_path / "src" / "molt" / "__init__.py", "")
    _write(
        tmp_path / "src" / "molt" / "authority.py",
        "def keep():\n    return 1\n\nCONST = 2\n\nif True:\n    def gated():\n        return 3\n",
    )
    _write(tmp_path / "tools" / "__init__.py", "")
    return tmp_path


def test_resolved_imports_are_silent(repo: Path) -> None:
    _write(
        repo / "tools" / "consumer.py",
        "from molt.authority import keep, CONST, gated\nfrom molt import authority\n",
    )
    assert gate.scan() == []


def test_missing_symbol_fails_closed(repo: Path) -> None:
    consumer = _write(
        repo / "tools" / "consumer.py",
        "from molt.authority import keep, moved_elsewhere\n",
    )
    findings = gate.scan()
    assert [(f.path, f.line, f.module, f.name) for f in findings] == [
        (consumer, 1, "molt.authority", "moved_elsewhere")
    ]
    assert "moved_elsewhere" in findings[0].render()
    assert gate.main([]) == 1


def test_pep562_forwarding_module_is_dynamic(repo: Path) -> None:
    _write(
        repo / "src" / "molt" / "cli" / "__init__.py",
        "def __getattr__(name):\n    raise AttributeError(name)\n",
    )
    _write(repo / "tools" / "consumer.py", "from molt.cli import _forwarded_helper\n")
    assert gate.scan() == []


def test_compiled_corpus_directories_are_not_host_imports(repo: Path) -> None:
    _write(
        repo / "tests" / "molt_only" / "basic" / "program.py",
        "from molt import CancellationToken\n",
    )
    _write(repo / "tests" / "test_host.py", "from molt.authority import vanished\n")
    findings = gate.scan()
    assert [f.name for f in findings] == ["vanished"]


def test_live_tree_resolves() -> None:
    """The real src/ and tools/ trees must stay clean; tests/ is covered by the gate command."""
    assert gate.scan((gate.ROOT / "src", gate.ROOT / "tools")) == []
