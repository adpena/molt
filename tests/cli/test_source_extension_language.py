from __future__ import annotations

from pathlib import Path

import pytest

from molt.cli.source_extension_language import (
    SourceExtensionLanguage,
    require_source_extension_language,
    resolve_source_extension_compile_language,
    validate_source_extension_language_command,
    source_extension_compile_io_args,
)
from molt.cli.source_extensions import (
    _compile_command_semantic_args,
    _compile_command_args_and_include_dirs,
)


@pytest.mark.parametrize(
    ("suffix", "language", "role"),
    [
        (".c", "c", "c"),
        (".C", "cpp", "cpp"),
        (".cc", "cpp", "cpp"),
        (".cpp", "cpp", "cpp"),
        (".CPP", "cpp", "cpp"),
        (".cxx", "cpp", "cpp"),
        (".c++", "cpp", "cpp"),
        (".m", "objc", "c"),
        (".M", "objcpp", "cpp"),
        (".mm", "objcpp", "cpp"),
    ],
)
def test_original_input_language_is_resolved_once(
    suffix: str, language: str, role: str
) -> None:
    resolved, args = resolve_source_extension_compile_language(
        source_path=Path("unit" + suffix), language=None, compile_args=("-O3",)
    )
    assert resolved == language
    assert resolved.compiler_role == role
    assert args == ("-O3",)


@pytest.mark.parametrize("language", list(SourceExtensionLanguage))
def test_declared_language_survives_digest_addressing(
    language: SourceExtensionLanguage,
) -> None:
    resolved, args = resolve_source_extension_compile_language(
        source_path=Path("provenance/compiled-inputs/sha256/aa/" + "a" * 64),
        language=language,
        compile_args=("-O2",),
    )
    assert resolved is language
    assert args == ("-O2",)


@pytest.mark.parametrize(
    ("args", "expected"),
    [
        (("-x", "c++", "-O3"), "cpp"),
        (("-xobjective-c++", "-O3"), "objcpp"),
        (("/TP", "-O3"), "cpp"),
        (("/TC", "-O3"), "c"),
        (("-x", "c++", "-x", "none", "-O3"), "c"),
    ],
)
def test_explicit_language_normalizes_before_replay(
    args: tuple[str, ...], expected: str
) -> None:
    language, remaining = resolve_source_extension_compile_language(
        source_path=Path("unit.c"), language="c", compile_args=args
    )
    assert language == expected
    assert remaining == ("-O3",)


@pytest.mark.parametrize("forwarder", ["-mllvm", "-Xassembler", "-Xpreprocessor"])
@pytest.mark.parametrize("operand", ["-xc++", "-c", "/TP", "--", "--driver-mode=cl"])
@pytest.mark.parametrize("clang_cl", [False, True])
def test_opaque_operands_cannot_select_language_or_corrupt_receipt_validation(
    forwarder, operand, clang_cl
):
    transport = (lambda value: "/clang:" + value) if clang_cl else (lambda value: value)
    opaque = (transport(forwarder), transport(operand))
    language, remaining = resolve_source_extension_compile_language(
        source_path=Path("unit.c"), language="c", compile_args=opaque
    )
    assert language is SourceExtensionLanguage.C and remaining == opaque
    compiler = ("clang-cl" if clang_cl else "clang",)
    canonical = source_extension_compile_io_args(
        language, compiler, Path("unit.c"), Path("unit.o")
    )
    validate_source_extension_language_command(
        language, (*compiler, *opaque, *canonical)
    )


def test_ordinary_driver_operands_are_not_language_selectors():
    language, remaining = resolve_source_extension_compile_language(
        source_path=Path("unit.c"),
        language="c",
        compile_args=("-D", "-xc++", "-I", "/TP"),
    )
    assert language is SourceExtensionLanguage.C
    assert remaining == ("-D", "-xc++", "-I", "/TP")


@pytest.mark.parametrize("value", [None, False, 0, {}, [], "", "c++", "cuda"])
def test_persisted_language_requires_canonical_fact(value: object) -> None:
    with pytest.raises(ValueError, match="language"):
        require_source_extension_language(value)


@pytest.mark.parametrize("args", [("-x",), ("-x", "cuda"), ("-x", "")])
def test_invalid_explicit_language_is_not_ignored(args: tuple[str, ...]) -> None:
    with pytest.raises(ValueError, match="language"):
        resolve_source_extension_compile_language(
            source_path=Path("unit.c"), language=None, compile_args=args
        )


def test_retained_filename_cannot_supply_missing_language() -> None:
    with pytest.raises(ValueError, match="no declared language"):
        resolve_source_extension_compile_language(
            source_path=Path("a" * 64), language=None, compile_args=()
        )


def test_compile_database_language_is_positional(tmp_path: Path) -> None:
    source = (tmp_path / "unit.c").resolve()
    args = _compile_command_semantic_args(
        ["clang", "-x", "c++", "-c", str(source), "-x", "c", "-O2"],
        source_path=source,
        directory=tmp_path,
    )
    language, remaining = resolve_source_extension_compile_language(
        source_path=source, language="c", compile_args=args
    )
    assert language is SourceExtensionLanguage.CPP
    assert remaining == ("-O2",)


@pytest.mark.parametrize("selector", ["/Tp", "/Tc"])
@pytest.mark.parametrize("joined", [True, False])
def test_compile_database_per_file_language_selectors(
    tmp_path: Path, selector: str, joined: bool
) -> None:
    source = (tmp_path / "unit.c").resolve()
    arguments = [selector + str(source)] if joined else [selector, str(source)]
    args = _compile_command_semantic_args(
        ["clang-cl", *arguments, "/O2"], source_path=source, directory=tmp_path
    )
    language, remaining = resolve_source_extension_compile_language(
        source_path=source, language=None, compile_args=args
    )
    assert language == ("cpp" if selector == "/Tp" else "c")
    assert remaining == ("/O2",)


def test_msvc_side_outputs_do_not_replace_forced_header_or_runtime_flags(tmp_path):
    source = tmp_path / "unit.cpp"
    args = _compile_command_semantic_args(
        [
            "clang-cl",
            "/c",
            str(source),
            "/Fddebug.pdb",
            "/Fa",
            "listing.asm",
            "/Feprogram.exe",
            "/Fiout.i",
            "/FRbrowse.sbr",
            "/sourceDependencies",
            "deps.json",
            "/scanDependencies",
            "scan.json",
            "/MP8",
            "/FS",
            "/FAcs",
            "/MD",
            "/MTd",
            "/FIconfig.h",
            "-include",
            "other.h",
            "/utf-8",
            "/std:c++17",
        ],
        source_path=source,
        directory=tmp_path,
    )
    compile_args, includes = _compile_command_args_and_include_dirs(
        args, directory=tmp_path
    )
    assert includes == ()
    assert compile_args == (
        "/MD",
        "/MTd",
        "/FI",
        str(tmp_path / "config.h"),
        "-include",
        str(tmp_path / "other.h"),
        "/utf-8",
        "/std:c++17",
    )


@pytest.mark.parametrize(
    "selector",
    [
        "/Yuprefix.h",
        "/Ycprefix.h",
        "/Fpprefix.pch",
        "-include-pch",
        "-fmodule-file=module.pcm",
    ],
)
def test_compile_database_rejects_unowned_precompiled_inputs(tmp_path, selector):
    source = tmp_path / "unit.c"
    with pytest.raises(ValueError, match="precompiled-header/module input custody"):
        _compile_command_semantic_args(
            ["clang-cl", "/c", str(source), selector],
            source_path=source,
            directory=tmp_path,
        )


def test_command_cannot_contradict_persisted_language() -> None:
    with pytest.raises(ValueError, match="differs from declared language"):
        validate_source_extension_language_command(
            SourceExtensionLanguage.CPP, ("clang++", "-x", "c", "-c", "unit")
        )


@pytest.mark.parametrize("language", list(SourceExtensionLanguage))
def test_command_language_is_explicit_at_its_source(
    language: SourceExtensionLanguage,
) -> None:
    validate_source_extension_language_command(
        language,
        (
            "clang",
            "--target=wasm32-wasip1",
            "-x",
            language.driver_language,
            "-c",
            "@source/original.c",
            "-o",
            "@object-root/output.o",
        ),
    )


@pytest.mark.parametrize(
    "command",
    [
        ("clang", "-c", "native.c"),
        ("clang",),
        ("clang", "-x", "c++", "-c"),
        ("clang", "-x", "c++", "-c", "-o", "out.o"),
        ("clang", "-c", "native.c", "-x", "c++"),
        ("clang", "-x", "c", "-c", "native.c", "-x", "c++"),
        ("clang", "-x", "c++", "-c", "native.c", "-x", "c"),
        ("clang", "-x", "c++", "-c", "native.c", "-x", "c++"),
        ("clang", "-xc++", "-c", "native.c"),
        ("clang", "/TP", "-c", "native.c"),
        ("clang", "-x", "c++", "-c", "native.c", "-c", "other.c"),
        ("clang", "--", "-x", "c++", "-c", "native.c"),
    ],
)
def test_noncanonical_command_cannot_borrow_declared_language(
    command: tuple[str, ...],
) -> None:
    with pytest.raises(ValueError, match="source-extension compile command"):
        validate_source_extension_language_command(SourceExtensionLanguage.CPP, command)


@pytest.mark.parametrize(
    "language, selector",
    [(SourceExtensionLanguage.C, "/TC"), (SourceExtensionLanguage.CPP, "/TP")],
)
def test_clang_cl_emission_and_recorded_language_share_one_contract(language, selector):
    args = source_extension_compile_io_args(
        language, ("clang-cl",), Path("digest"), Path("out.obj")
    )
    assert args == (selector, "/c", "digest", "/Foout.obj")
    validate_source_extension_language_command(language, ("clang-cl", *args))
    with pytest.raises(ValueError, match="differs from declared language"):
        validate_source_extension_language_command(
            language, ("clang-cl", "/TP" if selector == "/TC" else "/TC", *args[1:])
        )
    with pytest.raises(ValueError, match="source-extension compile command"):
        validate_source_extension_language_command(
            language, ("clang-cl", *args, "/clang:-xc++")
        )


@pytest.mark.parametrize(
    "language", [SourceExtensionLanguage.OBJC, SourceExtensionLanguage.OBJCPP]
)
def test_clang_cl_rejects_unadmitted_objc_language(language):
    with pytest.raises(ValueError, match="language capability"):
        source_extension_compile_io_args(
            language, ("clang-cl",), Path("unit"), Path("out.obj")
        )


def test_per_file_msvc_language_dominates_global_selectors_and_discards_dependencies(
    tmp_path,
):
    source = (tmp_path / "unit.c").resolve()
    args = _compile_command_semantic_args(
        [
            "clang-cl",
            "/Tp" + str(source),
            "/TC",
            "/Tcother.c",
            "/clang:-MD",
            "/clang:-MFout.d",
            "/Foout.obj",
            "/showIncludes",
            "/O2",
        ],
        source_path=source,
        directory=tmp_path,
    )
    language, remaining = resolve_source_extension_compile_language(
        source_path=source, language=None, compile_args=args
    )
    assert language is SourceExtensionLanguage.CPP
    assert remaining == ("/O2",)
