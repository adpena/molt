from __future__ import annotations

import ast
import codecs
import hashlib
from pathlib import Path

import pytest

from molt.cli.module_source import PythonSourceSnapshot, _read_module_source
from molt.cli.python_import_resolution import LocalPythonModuleResolver
from molt.compiler_analysis.python_source_keys import python_ast_digest


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
    foreign = tmp_path / "foreign.bin"
    foreign.write_bytes(b"unrelated descriptor traffic")
    read_bytes = Path.read_bytes
    reads: list[Path] = []

    def counted_read(self: Path) -> bytes:
        if self == path:
            reads.append(self)
        return read_bytes(self)

    monkeypatch.setattr(Path, "read_bytes", counted_read)
    assert foreign.read_bytes() == b"unrelated descriptor traffic"
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


@pytest.mark.parametrize("operation", ["read", "validate"])
@pytest.mark.parametrize("change", ["same-stat", "remove"])
def test_source_lease_never_adopts_a_later_generation(
    tmp_path: Path, operation: str, change: str
) -> None:
    import os
    from molt.cli.module_source import _ModuleSourceLease, PythonSourceChangedError

    path = tmp_path / "source.py"
    content = b"VALUE = 1\n"
    path.write_bytes(content)
    metadata = path.stat()
    lease = _ModuleSourceLease.path_backed(path)
    if change == "remove":
        path.unlink()
    else:
        path.write_bytes(b"VALUE = 2\n")
        os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    with pytest.raises(PythonSourceChangedError, match="during compile"):
        getattr(lease, operation)()
    assert lease.source_sha256 == hashlib.sha256(content).hexdigest()


def test_inline_and_raw_source_leases_keep_distinct_generation_kinds(
    tmp_path: Path,
) -> None:
    from molt.cli.module_source import _ModuleSourceLease

    path = tmp_path / "source.py"
    path.write_bytes(b"# coding: latin-1\r\nVALUE = '\xe9'\r\n")
    raw = _ModuleSourceLease.path_backed(path)
    inline = _ModuleSourceLease.inline(path, PythonSourceSnapshot.capture(path).text)
    assert raw.source_kind == "path"
    assert inline.source_kind == "inline"
    assert raw.source_sha256 != inline.source_sha256
    for lease in (raw, inline):
        decoded = _ModuleSourceLease.from_worker_payload(lease.worker_payload())
        assert decoded == lease
        assert decoded.read() == "# coding: latin-1\nVALUE = 'é'\n"
    path.unlink()
    assert inline.read().endswith("'é'\n")
    inline.validate()


@pytest.mark.parametrize("field", ["source_sha256", "source_size", "kind", "path"])
def test_worker_source_lease_rejects_missing_generation_identity(
    tmp_path: Path, field: str
) -> None:
    from molt.cli.module_source import _ModuleSourceLease

    lease = _ModuleSourceLease.inline(tmp_path / "source.py", "VALUE = 1\n")
    payload = lease.worker_payload()
    del payload[field]
    with pytest.raises(ValueError):
        _ModuleSourceLease.from_worker_payload(payload)


def test_source_catalog_requires_admitted_rows(tmp_path: Path) -> None:
    from molt.cli.module_source import _ModuleSourceCatalog

    path = tmp_path / "source.py"
    path.write_text("VALUE = 1\n", encoding="utf-8")
    catalog = _ModuleSourceCatalog({})
    with pytest.raises(ValueError, match="lacks captured source lease"):
        catalog.read_source("new_module", path)


@pytest.mark.parametrize(
    "content",
    [
        codecs.BOM_UTF8 + b"# coding: latin-1\nx = 1\n",
        b"# coding: does-not-exist\nx = 1\n",
        b"# coding: utf-8\nvalue = '\xff'\n",
        b"value = '\xff'\n",
    ],
)
def test_snapshot_encoding_failure_is_native_parser_diagnostic(
    tmp_path: Path,
    content: bytes,
) -> None:
    path = tmp_path / "physical.py"
    path.write_bytes(content)
    snapshot = PythonSourceSnapshot.capture(path)
    path.write_bytes(b"replacement = 1\n")
    filename = "/logical/package/source.py"
    with pytest.raises(SyntaxError) as expected:
        ast.parse(content, filename=filename)
    with pytest.raises(SyntaxError) as actual:
        snapshot.text_for_filename(filename)
    assert type(actual.value) is type(expected.value)
    assert actual.value.args == expected.value.args
    for name in (
        "msg",
        "filename",
        "lineno",
        "offset",
        "text",
        "end_lineno",
        "end_offset",
        "print_file_and_line",
    ):
        assert getattr(actual.value, name) == getattr(expected.value, name)


def test_snapshot_success_does_not_parse_and_decoder_disagreement_is_visible(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import molt.cli.module_source as module_source

    def unexpected_parse(*args, **kwargs):
        raise AssertionError("successful decoding must not invoke the parser")

    monkeypatch.setattr(module_source.ast, "parse", unexpected_parse)
    snapshot = PythonSourceSnapshot(Path("valid.py"), b"value = 1\n")
    assert snapshot.text_for_filename("logical.py") == "value = 1\n"
    assert snapshot.text is snapshot.text

    # This is an impossible decoder/parser disagreement, not an alternate source.
    monkeypatch.setattr(
        module_source.ast,
        "parse",
        lambda *args, **kwargs: ast.Module(body=[], type_ignores=[]),
    )
    broken = PythonSourceSnapshot(Path("invalid.py"), b"# coding: no-such-codec\n")
    with pytest.raises(RuntimeError, match="decoder rejected bytes accepted") as caught:
        _ = broken.text
    assert isinstance(caught.value.__cause__, SyntaxError)
