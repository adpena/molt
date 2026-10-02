from __future__ import annotations

import hashlib
import subprocess
from dataclasses import replace
from pathlib import Path

import pytest

from molt.cli import native_symbol_inspection
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
        native_symbol_inspection,
        "_native_object_global_symbol_facts",
        lambda _path, **_kwargs: native_symbol_inspection._NativeGlobalSymbolFacts(
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
    weak: tuple[str, ...] = (),
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
        weak_defined_symbols=frozenset(weak),
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


@pytest.mark.parametrize("left_weak", [False, True])
@pytest.mark.parametrize("reverse", [False, True])
def test_eager_weak_overlap_preserves_every_root_and_dependency(
    tmp_path: Path, left_weak: bool, reverse: bool
) -> None:
    init = _root_closure_fact(tmp_path, "module", defined=("PyInit_module",))
    left = _root_closure_fact(
        tmp_path,
        "left",
        defined=("callback",),
        weak=("callback",) if left_weak else (),
        undefined=("cleanup",),
    )
    right = _root_closure_fact(
        tmp_path,
        "right",
        defined=("callback",),
        weak=("callback",),
        undefined=("registration",),
    )
    cleanup = _root_closure_fact(
        tmp_path, "cleanup", defined=("cleanup",), undefined=("error_handler",)
    )
    registration = _root_closure_fact(
        tmp_path, "registration", defined=("registration",), undefined=("callback",)
    )
    unused = _root_closure_fact(tmp_path, "unused", defined=("unused",))
    facts = (cleanup, right, unused, registration, init, left)
    forced = (left.object_path, right.object_path)
    if reverse:
        facts, forced = facts[::-1], forced[::-1]
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module",
        object_facts=facts,
        forced_object_paths=forced,
        retained_symbols=("callback",),
    )
    assert errors == []
    assert closure is not None
    assert closure.objects == tuple(f for f in facts if f is not unused)
    assert closure.undefined_symbols == ("error_handler",)
    assert closure.init_symbol_owner is init


@pytest.mark.parametrize("reverse", [False, True])
@pytest.mark.parametrize("force_left", [False, True])
def test_weak_competing_provider_is_not_admitted_by_traversal(
    tmp_path: Path, reverse: bool, force_left: bool
) -> None:
    init = _root_closure_fact(
        tmp_path, "module", defined=("PyInit_module",), undefined=("callback", "unique")
    )
    left = _root_closure_fact(
        tmp_path, "left", defined=("callback",), weak=("callback",)
    )
    right = _root_closure_fact(
        tmp_path, "right", defined=("callback", "unique"), weak=("callback",)
    )
    # Resolving the unique dependency will select right, but must never promote
    # its competing callback definition into the fixed set of eager roots.
    facts = (init, left, right)
    if reverse:
        facts = facts[::-1]
        init = replace(init, undefined_symbols=init.undefined_symbols[::-1])
        facts = tuple(init if f.object_path == init.object_path else f for f in facts)
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module",
        object_facts=facts,
        forced_object_paths=(left.object_path,) if force_left else (),
    )
    assert closure is None
    assert any(
        "callback" in e and "lazy/COMDAT selection is unsupported" in e for e in errors
    )


@pytest.mark.parametrize("weak", [(), ("PyInit_module",)])
def test_weak_binding_never_relaxes_unique_function_init_root(
    tmp_path: Path, weak: tuple[str, ...]
) -> None:
    init = _root_closure_fact(tmp_path, "module", defined=("PyInit_module",))
    other = _root_closure_fact(tmp_path, "other", defined=("PyInit_module",), weak=weak)
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module",
        object_facts=(init, other),
        forced_object_paths=(other.object_path,),
    )
    assert closure is None
    assert len(errors) == 1 and "root 'PyInit_module' is ambiguous" in errors[0]
    data_init = replace(
        init, defined_function_symbols=(), weak_defined_symbols=frozenset(weak)
    )
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module",
        object_facts=(data_init,),
    )
    assert closure is None
    assert len(errors) == 1 and "not a function symbol" in errors[0]


@pytest.mark.parametrize("data", [False, True])
@pytest.mark.parametrize(
    "bindings", [(True, True), (False, True), (False, False), (True,)]
)
def test_real_elf_eager_weak_closure_agrees_with_linker(
    tmp_path: Path, isolated_molt_cache: Path, data: bool, bindings: tuple[bool, ...]
) -> None:
    from molt.cli.llvm_wasi_tools import llvm_tool_candidates, llvm_linker_candidates
    from tests.native_artifact_fixtures import native_relocatable_object

    nm_candidates = llvm_tool_candidates("nm")
    linkers = llvm_linker_candidates("ld.lld")
    if not nm_candidates or not linkers:
        pytest.skip("canonical llvm-nm and ELF ld.lld are unavailable")
    nm, linker = str(nm_candidates[0]), str(linkers[0])
    target = "x86_64-unknown-linux-gnu"
    facts = []
    declarations = [("module", "PyInit_module", False, False)] + [
        (f"provider_{i}", "callback", weak, data) for i, weak in enumerate(bindings)
    ]
    for stem, symbol, weak, is_data in declarations:
        obj, src = tmp_path / f"{stem}.o", tmp_path / f"{stem}.c"
        src.write_text(
            "/* Independent ELF symbol fixture; not compiler output. */\n",
            encoding="utf-8",
        )
        obj.write_bytes(
            native_relocatable_object(
                target_triple=target,
                symbols=() if is_data else (symbol,),
                data_symbols=(symbol,) if is_data else (),
                weak_symbols=(symbol,) if weak else (),
            )
        )
        fact, error = source_extensions._source_extension_object_fact(
            source_path=src,
            object_path=obj,
            language=SourceExtensionLanguage.C,
            nm_command=(nm,),
            target_triple=target,
        )
        assert error is None and fact is not None
        assert fact.weak_defined_symbols == (
            frozenset({symbol}) if weak else frozenset()
        )
        assert fact.defined_function_symbols == (() if is_data else (symbol,))
        facts.append(fact)
    linked = subprocess.run(
        [
            linker,
            "-r",
            *[str(f.object_path) for f in facts],
            "-o",
            str(tmp_path / "linked.o"),
        ],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    accepted = bindings != (False, False)
    if accepted:
        assert linked.returncode == 0, linked.stderr
    else:
        assert linked.returncode != 0 and "duplicate symbol: callback" in linked.stderr
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module",
        object_facts=facts,
        forced_object_paths=tuple(f.object_path for f in facts),
        retained_symbols=("callback",),
    )
    if accepted:
        assert errors == []
        assert closure is not None and closure.objects == tuple(facts)
    else:
        assert closure is None and any(
            "callback" in e and "ambiguous" in e for e in errors
        )


@pytest.mark.parametrize("data", [False, True])
def test_wasm_binding_projection_reaches_eager_object_closure(
    tmp_path: Path, data: bool
) -> None:
    from tests.test_wasm_linking_symbols import _module, _function, _data, _section

    facts = []
    for stem, symbol in (
        ("module", "PyInit_module"),
        ("left", "callback"),
        ("right", "callback"),
    ):
        obj, src = tmp_path / f"{stem}.o", tmp_path / f"{stem}.c"
        src.write_text("/* Independent WASM symbol fixture. */\n", encoding="utf-8")
        weak = symbol == "callback"
        entry = (
            _data(symbol, flags=1)
            if data and weak
            else _function(symbol, flags=int(weak), index=0)
        )
        if data and weak:
            sections = _section(5, b"\x01\x00\x01") + _section(
                11, b"\x01\x00\x41\x00\x0b\x08" + bytes(8)
            )
        else:
            sections = (
                _section(1, b"\x01\x60\x00\x00")
                + _section(3, b"\x01\x00")
                + _section(10, b"\x01\x02\x00\x0b")
            )
        obj.write_bytes(_module(entry) + sections)
        fact, error = source_extensions._source_extension_object_fact(
            source_path=src,
            object_path=obj,
            language=SourceExtensionLanguage.C,
            target_triple="wasm32-wasip1",
        )
        assert error is None and fact is not None
        assert fact.weak_defined_symbols == (
            frozenset({symbol}) if weak else frozenset()
        )
        assert fact.defined_function_symbols == (() if data and weak else (symbol,))
        facts.append(fact)
    closure, errors = source_extensions._compute_source_extension_object_closure(
        init_symbol="PyInit_module",
        object_facts=facts,
        forced_object_paths=tuple(f.object_path for f in facts),
        retained_symbols=("callback",),
    )
    assert errors == []
    assert closure is not None and closure.objects == tuple(facts)
