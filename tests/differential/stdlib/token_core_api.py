"""Purpose: differential coverage for token module core API parity."""

from __future__ import annotations

import token


CORE_CONSTANTS = (
    "ENDMARKER",
    "NAME",
    "NUMBER",
    "STRING",
    "NEWLINE",
    "INDENT",
    "DEDENT",
    "OP",
    "ERRORTOKEN",
    "ENCODING",
    "N_TOKENS",
    "NT_OFFSET",
)

BASE_EXPORTS = [
    "tok_name",
    "ISTERMINAL",
    "ISNONTERMINAL",
    "ISEOF",
    "EXACT_TOKEN_TYPES",
]


def _check_constants() -> None:
    values = {}
    for name in CORE_CONSTANTS:
        value = getattr(token, name)
        assert isinstance(value, int), (name, value)
        values[name] = value
        assert token.tok_name[value] == name, (name, value)

    assert token.ENDMARKER == 0
    assert token.NAME == 1
    assert token.NT_OFFSET == 256
    assert token.N_TOKENS < token.NT_OFFSET
    assert token.ERRORTOKEN < token.ENCODING < token.N_TOKENS

    print(
        "constants",
        len(CORE_CONSTANTS),
        values["ENDMARKER"],
        values["NAME"],
        values["OP"],
        values["N_TOKENS"],
        values["NT_OFFSET"],
    )


def _check_tok_name_and_exact_types() -> None:
    assert isinstance(token.tok_name, dict)
    assert isinstance(token.EXACT_TOKEN_TYPES, dict)
    assert token.EXACT_TOKEN_TYPES

    expected_symbols = {
        "!=": "NOTEQUAL",
        "**=": "DOUBLESTAREQUAL",
        "//": "DOUBLESLASH",
        ":": "COLON",
        ":=": "COLONEQUAL",
        "...": "ELLIPSIS",
        "|=": "VBAREQUAL",
    }
    for symbol, expected_name in expected_symbols.items():
        token_value = token.EXACT_TOKEN_TYPES[symbol]
        assert isinstance(token_value, int), (symbol, token_value)
        assert token.tok_name[token_value] == expected_name, (symbol, token_value)
        assert token_value == getattr(token, expected_name), (symbol, expected_name)

    assert all(isinstance(symbol, str) for symbol in token.EXACT_TOKEN_TYPES)
    assert all(
        isinstance(value, int) and value in token.tok_name
        for value in token.EXACT_TOKEN_TYPES.values()
    )
    assert all(
        token.ISTERMINAL(value) and not token.ISNONTERMINAL(value)
        for value in token.EXACT_TOKEN_TYPES.values()
    )

    print(
        "exact",
        len(token.EXACT_TOKEN_TYPES),
        token.EXACT_TOKEN_TYPES["!="],
        token.EXACT_TOKEN_TYPES["**="],
        token.EXACT_TOKEN_TYPES["//"],
        token.tok_name[token.EXACT_TOKEN_TYPES["|="]],
    )


def _check_predicates_and_exports() -> None:
    assert isinstance(token.ISTERMINAL(token.NAME), bool)
    assert isinstance(token.ISNONTERMINAL(token.NT_OFFSET), bool)
    assert isinstance(token.ISEOF(token.ENDMARKER), bool)

    assert token.ISTERMINAL(token.NAME)
    assert token.ISTERMINAL(token.NT_OFFSET - 1)
    assert not token.ISTERMINAL(token.NT_OFFSET)

    assert token.ISNONTERMINAL(token.NT_OFFSET)
    assert token.ISNONTERMINAL(token.NT_OFFSET + 1)
    assert not token.ISNONTERMINAL(token.ENDMARKER)

    assert token.ISEOF(token.ENDMARKER)
    assert not token.ISEOF(token.NAME)
    assert not token.ISEOF(token.NT_OFFSET)

    assert token.__all__[:5] == BASE_EXPORTS
    required_exports = tuple(BASE_EXPORTS) + CORE_CONSTANTS
    assert all(name in token.__all__ for name in required_exports)
    assert all(name in token.__all__ for name in token.tok_name.values())

    print("exports", len(token.__all__), token.__all__[:5], token.__all__[-5:])
    print(
        "predicates",
        token.ISTERMINAL(token.NAME),
        token.ISTERMINAL(token.NT_OFFSET),
        token.ISNONTERMINAL(token.NT_OFFSET),
        token.ISNONTERMINAL(token.ENDMARKER),
        token.ISEOF(token.ENDMARKER),
        token.ISEOF(token.NAME),
    )


def main() -> None:
    _check_constants()
    _check_tok_name_and_exact_types()
    _check_predicates_and_exports()
    print("ok")


if __name__ == "__main__":
    main()
