from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

from molt.cli import backend_cache
from molt.cli import dependency_files, source_extensions


def test_depfile_parser_preserves_escaped_paths_and_continuations(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source file.c"
    header = tmp_path / "include" / "header value.h"
    source.write_text("#include \"header value.h\"\n", encoding="utf-8")
    header.parent.mkdir()
    header.write_text("#define VALUE 1\n", encoding="utf-8")
    depfile = tmp_path / "object.d"
    depfile.write_text(
        "object.o: source\\ file.c \\\n include/header\\ value.h\n",
        encoding="utf-8",
    )

    paths, error = dependency_files.parse_make_depfile(
        depfile,
        cwd=tmp_path,
        producer="compiler",
    )

    assert error is None
    assert paths == (source.resolve(), header.resolve())


def test_object_closure_identity_includes_checksummed_headers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = tmp_path / "module.c"
    header = tmp_path / "module.h"
    object_path = tmp_path / "module.o"
    source.write_text('#include "module.h"\n', encoding="utf-8")
    header.write_text("int exported(void);\n", encoding="utf-8")
    object_path.write_bytes(b"object")
    monkeypatch.setattr(
        backend_cache,
        "_native_object_global_symbol_sets",
        lambda _path, **_kwargs: ({"PyInit_module"}, set()),
    )

    fact, error = source_extensions._source_extension_object_fact(
        source_path=source,
        object_path=object_path,
        compile_command=("clang", "-c", str(source), "-o", str(object_path)),
        nm_command=("llvm-nm",),
        dependency_paths=(source, header),
    )

    assert error is None
    assert fact is not None
    assert fact.dependencies[0].path == header.resolve()
    assert fact.dependencies[0].sha256 == hashlib.sha256(header.read_bytes()).hexdigest()
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module",
        object_facts=(fact,),
    )
    assert errors == []
    assert closure is not None
    payload = closure.manifest_payload()
    assert payload["objects"][0]["dependencies"] == [
        {
            "path": str(header.resolve()),
            "sha256": hashlib.sha256(header.read_bytes()).hexdigest(),
        }
    ]
    assert payload["objects"][0]["compile_command"] == [
        "clang",
        "-c",
        str(source),
        "-o",
        "@object-root/module.o",
    ]
    assert payload["objects"][0]["symbol_command"] == ["llvm-nm"]
