from __future__ import annotations

import ast
import codecs
import hashlib
from pathlib import Path

import pytest

from molt.cli.module_source import PythonSourceSnapshot, _read_module_source
from molt.cli.python_import_resolution import LocalPythonModuleResolver
from molt.compiler_analysis.python_binding_flow import python_ast_digest


@pytest.mark.parametrize(
    ("content", "expected"),
    [
        (b"value = 'hello'\n", "value = 'hello'\n"),
        (
            b"# coding: latin-1\r\nvalue = 'caf\xe9'\r\n",
            "# coding: latin-1\nvalue = 'café'\n",
        ),
        (
            b"#!/usr/bin/python\n# coding: latin-1\nvalue = 'caf\xe9'\n",
            "#!/usr/bin/python\n# coding: latin-1\nvalue = 'café'\n",
        ),
        (codecs.BOM_UTF8 + b"value = 1\r\n", "value = 1\n"),
        (
            codecs.BOM_UTF8 + b"# coding: utf-8\nvalue = 1\n",
            "# coding: utf-8\nvalue = 1\n",
        ),
        (b"a = 1\r\nb = 2\rc = 3\n", "a = 1\nb = 2\nc = 3\n"),
        (b"", ""),
    ],
)
def test_snapshot_decodes_one_captured_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, content: bytes, expected: str
) -> None:
    path = tmp_path / "source.py"
    path.write_bytes(content)
    read_bytes = Path.read_bytes
    reads: list[Path] = []

    def counted_read(self: Path) -> bytes:
        reads.append(self)
        return read_bytes(self)

    monkeypatch.setattr(Path, "read_bytes", counted_read)
    snapshot = PythonSourceSnapshot.capture(path)
    assert snapshot.content == content
    assert snapshot.text == expected
    assert snapshot.text is snapshot.text
    assert snapshot.sha256 == hashlib.sha256(content).hexdigest()
    assert ast.dump(snapshot.tree) == ast.dump(ast.parse(content))
    assert snapshot.tree is snapshot.tree
    assert snapshot.ast_digest == python_ast_digest(snapshot.tree)
    assert reads == [path]
    assert _read_module_source(path) == expected
    assert reads == [path, path]


def test_snapshot_properties_do_not_reopen_mutated_source(tmp_path: Path) -> None:
    path = tmp_path / "source.py"
    original = b"# coding: latin-1\nvalue = 'caf\xe9'\n"
    path.write_bytes(original)
    resolver = LocalPythonModuleResolver((tmp_path,))
    snapshot = resolver.capture_source(path)
    path.write_bytes(b"value = 'changed'\n")
    assert snapshot.content == original
    assert snapshot.text == "# coding: latin-1\nvalue = 'café'\n"
    assert snapshot.sha256 == hashlib.sha256(original).hexdigest()
    assert ast.literal_eval(snapshot.tree.body[0].value) == "café"
    assert snapshot.ast_digest == python_ast_digest(ast.parse(original))
    path.unlink()
    assert snapshot.text.endswith("'café'\n")
    assert snapshot.tree is snapshot.tree


@pytest.mark.parametrize(
    "content",
    [
        codecs.BOM_UTF8 + b"# coding: latin-1\nvalue = 1\n",
        b"# coding: not-a-real-encoding\nvalue = 1\n",
    ],
)
def test_snapshot_rejects_invalid_encoding_declarations(content: bytes) -> None:
    snapshot = PythonSourceSnapshot(Path("invalid.py"), content)
    with pytest.raises(SyntaxError):
        _ = snapshot.text
    with pytest.raises(ValueError, match="cannot parse local Python source"):
        _ = snapshot.tree


def test_snapshot_host_parse_preserves_diagnostic_path() -> None:
    snapshot = PythonSourceSnapshot(Path("broken.py"), b"def broken(:\n")
    with pytest.raises(ValueError, match="cannot parse local Python source broken.py"):
        _ = snapshot.tree
