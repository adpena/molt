from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

from molt.cli import backend_cache
from molt.cli import dependency_files, source_extensions
from molt.cli.source_extension_language import SourceExtensionLanguage


def test_depfile_parser_preserves_escaped_paths_and_continuations(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source file.c"
    header = tmp_path / "include" / "header value.h"
    source.write_text('#include "header value.h"\n', encoding="utf-8")
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
        "_native_object_global_symbol_facts",
        lambda _path, **_kwargs: backend_cache._NativeGlobalSymbolFacts(
            defined=frozenset({"PyInit_module"}),
            undefined=frozenset(),
            defined_functions=frozenset({"PyInit_module"}),
            artifact_digest=hashlib.sha256(object_path.read_bytes()).hexdigest(),
        ),
    )

    fact, error = source_extensions._source_extension_object_fact(
        source_path=source,
        object_path=object_path,
        language=SourceExtensionLanguage.C,
        compile_command=("clang", "-x", "c", "-c", str(source), "-o", str(object_path)),
        nm_command=("llvm-nm",),
        dependency_paths=(source, header),
    )

    assert error is None
    assert fact is not None
    assert fact.dependencies[0].path == header.resolve()
    assert (
        fact.dependencies[0].sha256 == hashlib.sha256(header.read_bytes()).hexdigest()
    )
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
        "-x",
        "c",
        "-c",
        str(source),
        "-o",
        "@object-root/module.o",
    ]
    assert payload["objects"][0]["symbol_command"] == ["llvm-nm"]


def _root_closure_fact(
    root: Path,
    name: str,
    *,
    defined: tuple[str, ...],
    undefined: tuple[str, ...] = (),
    suffix: str = ".o",
) -> source_extensions._SourceExtensionObjectFact:
    return source_extensions._SourceExtensionObjectFact(
        source_path=root / f"{name}.c",
        language=SourceExtensionLanguage.C,
        object_path=root / f"{name}{suffix}",
        source_sha256="0" * 64,
        object_sha256="1" * 64,
        defined_symbols=defined,
        undefined_symbols=undefined,
        defined_function_symbols=defined,
        compile_command=(),
        symbol_authority=source_extensions.SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY,
        symbol_command=("llvm-nm",),
    )


@pytest.mark.parametrize("suffix", [".o", ".obj"])
@pytest.mark.parametrize("root_kind", ["forced", "retained"])
def test_object_closure_all_roots_share_transitive_selection(
    tmp_path: Path, suffix: str, root_kind: str
) -> None:
    init = _root_closure_fact(
        tmp_path, "module", defined=("PyInit_module",), suffix=suffix
    )
    forced = _root_closure_fact(
        tmp_path,
        "registration",
        defined=("register",),
        undefined=("helper",),
        suffix=suffix,
    )
    helper = _root_closure_fact(
        tmp_path,
        "helper",
        defined=("helper",),
        undefined=("register", "external"),
        suffix=suffix,
    )
    unused = _root_closure_fact(tmp_path, "unused", defined=("unused",), suffix=suffix)
    # Output ordering belongs to the source plan, not DFS/root insertion order.
    facts = (helper, init, unused, forced)
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module",
        object_facts=facts,
        forced_object_paths=(
            (forced.object_path, forced.object_path) if root_kind == "forced" else ()
        ),
        retained_symbols=("register", "register") if root_kind == "retained" else (),
    )
    assert errors == []
    assert closure is not None
    assert closure.objects == (helper, init, forced)
    assert closure.undefined_symbols == ("external",)
    assert closure.init_symbol_owner == init

    lazy, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module", object_facts=facts
    )
    assert errors == []
    assert lazy is not None
    assert lazy.objects == (init,)


def test_object_closure_retained_external_root_is_not_lost(tmp_path: Path) -> None:
    init = _root_closure_fact(tmp_path, "module", defined=("PyInit_module",))
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module",
        object_facts=(init,),
        retained_symbols=("external_registration", "external_registration"),
    )
    assert errors == []
    assert closure is not None
    assert closure.undefined_symbols == ("external_registration",)


def test_object_closure_rejects_missing_forced_member(tmp_path: Path) -> None:
    init = _root_closure_fact(tmp_path, "module", defined=("PyInit_module",))
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module",
        object_facts=(init,),
        forced_object_paths=(tmp_path / "uncompiled.obj",),
    )
    assert closure is None
    assert len(errors) == 1
    assert "forced object" in errors[0]
    assert "uncompiled.obj" in errors[0]
    assert "no compiled fact" in errors[0]


@pytest.mark.parametrize("root_kind", ["forced", "retained"])
def test_object_closure_rejects_ambiguous_admitted_roots(
    tmp_path: Path, root_kind: str
) -> None:
    init = _root_closure_fact(tmp_path, "module", defined=("PyInit_module",))
    left = _root_closure_fact(tmp_path, "left", defined=("registration",))
    right = _root_closure_fact(tmp_path, "right", defined=("registration",))
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module",
        object_facts=(init, left, right),
        forced_object_paths=(
            (left.object_path, right.object_path) if root_kind == "forced" else ()
        ),
        retained_symbols=("registration",) if root_kind == "retained" else (),
    )
    assert closure is None
    assert any(
        "registration" in error
        and "ambiguous" in error
        and "left.o" in error
        and "right.o" in error
        for error in errors
    )


def test_object_closure_rejects_duplicate_compiled_identity(tmp_path: Path) -> None:
    from dataclasses import replace

    init = _root_closure_fact(tmp_path, "module", defined=("PyInit_module",))
    alias = replace(
        init, defined_symbols=("another",), defined_function_symbols=("another",)
    )
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module", object_facts=(init, alias)
    )
    assert closure is None
    assert len(errors) == 1
    assert "object identity is duplicated" in errors[0]
