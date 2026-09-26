"""Typed translation-unit language and compiler-role authority."""

from __future__ import annotations

from collections.abc import Sequence
from enum import StrEnum
from pathlib import Path
from typing import Literal

from molt.cli.compiler_target import (
    SourceExtensionCompilerDialect,
    compiler_argument_spans,
    source_extension_compiler_dialect,
)


class SourceExtensionLanguage(StrEnum):
    C = "c"
    CPP = "cpp"
    OBJC = "objc"
    OBJCPP = "objcpp"

    @property
    def compiler_role(self) -> Literal["c", "cpp"]:
        return "cpp" if self in {self.CPP, self.OBJCPP} else "c"

    @property
    def driver_language(self) -> str:
        return {
            self.C: "c",
            self.CPP: "c++",
            self.OBJC: "objective-c",
            self.OBJCPP: "objective-c++",
        }[self]


def require_source_extension_language(value: object) -> SourceExtensionLanguage:
    """Parse a persisted canonical language; never infer it from retained paths."""
    if not isinstance(value, str):
        raise ValueError("source-extension language must be c, cpp, objc, or objcpp")
    try:
        return SourceExtensionLanguage(value)
    except ValueError as exc:
        raise ValueError(
            f"source-extension language is not canonical: {value!r}"
        ) from exc


def _input_language(value: str) -> SourceExtensionLanguage:
    aliases = {
        "c++": "cpp",
        "objective-c": "objc",
        "objective-c++": "objcpp",
    }
    return require_source_extension_language(aliases.get(value, value))


def _source_path_language(path: Path) -> SourceExtensionLanguage:
    # Clang distinguishes .C/.M from .c/.m even on case-insensitive hosts.
    suffixes = {
        ".C": SourceExtensionLanguage.CPP,
        ".M": SourceExtensionLanguage.OBJCPP,
        ".c": SourceExtensionLanguage.C,
        ".cc": SourceExtensionLanguage.CPP,
        ".cpp": SourceExtensionLanguage.CPP,
        ".cxx": SourceExtensionLanguage.CPP,
        ".c++": SourceExtensionLanguage.CPP,
        ".m": SourceExtensionLanguage.OBJC,
        ".mm": SourceExtensionLanguage.OBJCPP,
    }
    language = suffixes.get(path.suffix) or suffixes.get(path.suffix.lower())
    if language is None:
        raise ValueError(f"source-extension input has no declared language: {path}")
    return language


def resolve_source_extension_compile_language(
    *,
    source_path: Path,
    language: str | None,
    compile_args: Sequence[str],
) -> tuple[SourceExtensionLanguage, tuple[str, ...]]:
    """Normalize producer input once, removing language switches from replay.

    Upstream explicit language switches take precedence over its source-group
    declaration. A direct source uses its original suffix only at this boundary.
    The caller emits the resolved language before the source operand.
    """
    declared = _input_language(language) if language is not None else None
    selected = declared
    args: list[str] = []
    for span in compiler_argument_spans(compile_args):
        if span.context != "driver":
            args.extend(span.raw)
            continue
        argument = span.option
        value: str | None = None
        if argument == "-x":
            value = span.arguments[1]
        elif argument.startswith("-x") and len(argument) > 2:
            value = argument[2:]
        elif argument in {"/TC", "/TP"}:
            value = "c" if argument == "/TC" else "cpp"
        elif argument.startswith(("/Tc", "/Tp")):
            raise ValueError(
                "source-extension per-file /Tc or /Tp must be normalized by the compile database"
            )
        if value is None:
            args.extend(span.raw)
        else:
            selected = declared if value == "none" else _input_language(value)
    if selected is None:
        selected = _source_path_language(source_path)
    return selected, tuple(args)


def source_extension_compile_io_args(
    language: SourceExtensionLanguage,
    compiler_command: Sequence[str],
    source: Path,
    output: Path,
) -> tuple[str, ...]:
    dialect = source_extension_compiler_dialect(compiler_command)
    if dialect is SourceExtensionCompilerDialect.CLANG_CL:
        if language not in {SourceExtensionLanguage.C, SourceExtensionLanguage.CPP}:
            raise ValueError(
                f"clang-cl has no admitted {language.value} language capability"
            )
        return (
            "/TC" if language is SourceExtensionLanguage.C else "/TP",
            "/c",
            str(source),
            f"/Fo{output}",
        )
    return ("-x", language.driver_language, "-c", str(source), "-o", str(output))


def validate_source_extension_language_command(
    language: SourceExtensionLanguage,
    command: Sequence[str],
) -> None:
    """Require the producer's explicit language clause at its exact source.

    This validates emitted commands, not arbitrary upstream driver syntax. The
    latter is normalized before compilation. Input custody rewrites the retained
    ``source`` path but deliberately preserves the original command operands.
    Neither the retained path nor a declared default supplies missing evidence.
    """
    dialect = source_extension_compiler_dialect(command)
    cl = dialect is SourceExtensionCompilerDialect.CLANG_CL
    spans = compiler_argument_spans(command)
    compile_indexes = [
        span.index
        for span in spans
        if span.context == "driver" and span.option in {"-c", "/c"}
    ]
    if len(compile_indexes) != 1:
        raise ValueError(
            "source-extension compile command requires one -c source operand"
        )
    compile_index = compile_indexes[0]
    if (
        compile_index + 1 >= len(command)
        or not command[compile_index + 1]
        or command[compile_index + 1].startswith(("-", "/Fo"))
        or command[compile_index] != ("/c" if cl else "-c")
    ):
        raise ValueError(
            "source-extension compile command has no canonical source operand"
        )
    selector_indexes = [
        span.index
        for span in spans
        if span.context == "driver"
        and span.index != compile_index + 1
        and (
            span.option.startswith("-x")
            or span.option in {"/TC", "/TP"}
            or span.option.startswith(("/Tc", "/Tp"))
        )
    ]
    if cl:
        if (
            language not in {SourceExtensionLanguage.C, SourceExtensionLanguage.CPP}
            or compile_index < 2
            or selector_indexes != [compile_index - 1]
        ):
            raise ValueError(
                "source-extension compile command requires one canonical /TC or /TP immediately before /c source"
            )
        expected = "/TC" if language is SourceExtensionLanguage.C else "/TP"
        if command[compile_index - 1] != expected:
            raise ValueError(
                "source-extension compile command differs from declared language"
            )
        return
    if (
        compile_index < 3
        or selector_indexes != [compile_index - 2]
        or command[compile_index - 2] != "-x"
    ):
        raise ValueError(
            "source-extension compile command requires exactly one canonical "
            "-x language immediately before -c source"
        )
    if command[compile_index - 1] != language.driver_language:
        raise ValueError(
            f"source-extension compile command language {command[compile_index - 1]!r} "
            f"differs from declared language {language.value}"
        )
