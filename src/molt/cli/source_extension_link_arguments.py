"""One linker operand grammar for upstream projection and final-link admission.

An upstream extension's image/output policy belongs to that producer, not to the
executable that will consume Molt's static extension. Only source-plan projection
may consume these options; explicit final-link requirements remain strict.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from typing import Literal

from molt.cli.source_extension_target import SourceExtensionLinkDialect as Dialect

_GNU = frozenset({Dialect.ELF_GNU, Dialect.COFF_GNU, Dialect.WASM})
_UNIX = _GNU | {Dialect.MACHO}
_MSVC = frozenset({Dialect.COFF_MSVC})
_MACHO = frozenset({Dialect.MACHO})
_ALL = frozenset(Dialect)

# Closed sets: resource/search/ABI/symbol options are never inferred to be
# producer policy simply because they look like flags.
_PRODUCT_FLAGS = {
    "/nologo": _MSVC,
    "/dll": _MSVC,
    "/incremental": _MSVC,
    "/incremental:no": _MSVC,
    "/brepro": _MSVC,
    "/opt:ref": _MSVC,
    "/debug": _MSVC,
    "/debug:full": _MSVC,
    "/debug:none": _MSVC,
    "/debug:fastlink": _MSVC,
    "-shared": _UNIX - {Dialect.WASM},
    "--shared": _GNU - {Dialect.WASM},
    "--gc-sections": _GNU,
    "--eh-frame-hdr": frozenset({Dialect.ELF_GNU}),
    "--no-entry": frozenset({Dialect.WASM}),
    "-bundle": _MACHO,
    "-dylib": _MACHO,
    "-dead_strip": _MACHO,
    "-headerpad_max_install_names": _MACHO,
}
_PRODUCT_VALUES = {
    "/out": _MSVC,
    "/implib": _MSVC,
    "/pdb": _MSVC,
    "/map": _MSVC,
    "-o": _UNIX,
    "--output": _GNU,
    "-soname": frozenset({Dialect.ELF_GNU}),
    "--soname": frozenset({Dialect.ELF_GNU}),
    "-install_name": _MACHO,
    "-compatibility_version": _MACHO,
    "-current_version": _MACHO,
}
_SCOPES = {
    "--start-group": _GNU,
    "--end-group": _GNU,
    "--whole-archive": _GNU,
    "--no-whole-archive": _GNU,
    "--as-needed": frozenset({Dialect.ELF_GNU}),
    "--no-as-needed": frozenset({Dialect.ELF_GNU}),
}


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkArgument:
    kind: Literal[
        "product",
        "input",
        "forced",
        "framework",
        "scope",
        "dependency",
        "symbol",
        "library",
        "default-library",
        "thread-runtime",
    ]
    arguments: tuple[str, ...]
    value: str
    dialects: frozenset[Dialect] = _ALL

    def validate_dialect(self, dialect: Dialect) -> None:
        if dialect not in self.dialects:
            raise ValueError(
                f"source-extension {dialect.value} does not support linker operand "
                f"{self.arguments!r}"
            )


def _linker_tokens(arguments: Sequence[str]) -> tuple[str, ...]:
    tokens: list[str] = []
    iterator = iter(arguments)
    for argument in iterator:
        if not argument or "\x00" in argument:
            raise ValueError("source-extension linker arguments must be non-empty")
        if argument == "-Xlinker":
            value = next(iterator, None)
            if not value or value == "-Xlinker" or "\x00" in value:
                raise ValueError("source-extension -Xlinker requires one operand")
            tokens.append(value)
        elif argument.startswith("-Wl,"):
            # Commas are separators in this driver syntax. Use -Xlinker for
            # literal commas; never split those payloads a second time.
            values = argument[4:].split(",")
            if any(not value for value in values):
                raise ValueError("source-extension -Wl contains an empty operand")
            tokens.extend(values)
        else:
            tokens.append(argument)
    return tuple(tokens)


def source_extension_link_arguments(
    arguments: Sequence[str],
) -> tuple[SourceExtensionLinkArgument, ...]:
    tokens = _linker_tokens(arguments)
    result: list[SourceExtensionLinkArgument] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        index += 1
        key = token.lower() if token.startswith("/") else token
        if key in _PRODUCT_FLAGS:
            result.append(
                SourceExtensionLinkArgument(
                    "product", (token,), token, _PRODUCT_FLAGS[key]
                )
            )
            continue
        if key in _SCOPES:
            result.append(
                SourceExtensionLinkArgument("scope", (f"-Wl,{key}",), key, _SCOPES[key])
            )
            continue
        option, separator, value = key.partition(":" if key.startswith("/") else "=")
        if option in _PRODUCT_VALUES:
            if separator:
                value = token[len(option) + 1 :]
            elif not option.startswith("/") and index < len(tokens):
                value = tokens[index]
                index += 1
            else:
                value = ""
            if not value or value.startswith("-"):
                raise ValueError(f"source-extension {option} requires one output value")
            args = (f"{option}:{value}",) if option.startswith("/") else (option, value)
            result.append(
                SourceExtensionLinkArgument(
                    "product", args, value, _PRODUCT_VALUES[option]
                )
            )
            continue
        if key in {"-force_load", "-framework", "-u", "--undefined", "-l"}:
            if (
                index == len(tokens)
                or not tokens[index]
                or tokens[index].startswith("-")
            ):
                raise ValueError(f"source-extension {key} requires one operand")
            value = tokens[index]
            index += 1
            if key == "-force_load":
                result.append(
                    SourceExtensionLinkArgument(
                        "forced", ("-Xlinker", key, "-Xlinker", value), value, _MACHO
                    )
                )
            elif key == "-framework":
                result.append(
                    SourceExtensionLinkArgument(
                        "framework", (key, value), value, _MACHO
                    )
                )
            elif key == "-l":
                result.append(
                    SourceExtensionLinkArgument(
                        "library", (f"-l{value}",), value, _UNIX
                    )
                )
            else:
                canonical = (
                    f"-Wl,-u,{value}" if key == "-u" else f"-Wl,--undefined={value}"
                )
                result.append(
                    SourceExtensionLinkArgument(
                        "symbol",
                        (canonical,),
                        value,
                        _UNIX if key == "-u" else _GNU,
                    )
                )
            continue
        if option in {"/wholearchive", "/defaultlib", "/include"} and separator:
            value = token[len(option) + 1 :]
            if not value:
                raise ValueError(f"source-extension {option} requires one operand")
            if option == "/wholearchive":
                span = SourceExtensionLinkArgument("forced", (token,), value, _MSVC)
            elif option == "/include":
                span = SourceExtensionLinkArgument("symbol", (token,), value, _MSVC)
            else:
                span = SourceExtensionLinkArgument(
                    "default-library", (token,), value, _MSVC
                )
            result.append(span)
            continue
        if token.startswith("--undefined="):
            result.append(
                SourceExtensionLinkArgument(
                    "symbol", (f"-Wl,{token}",), token[12:], _GNU
                )
            )
            continue
        if token == "-pthread":
            result.append(
                SourceExtensionLinkArgument(
                    "thread-runtime", (token,), "pthread", _UNIX - {Dialect.WASM}
                )
            )
            continue
        if token.startswith("-l") and len(token) > 2:
            result.append(
                SourceExtensionLinkArgument("library", (token,), token[2:], _UNIX)
            )
            continue
        kind = "dependency" if token.startswith(("-", "@")) else "input"
        result.append(SourceExtensionLinkArgument(kind, (token,), token))
    return tuple(result)
