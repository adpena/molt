"""Purpose: differential coverage for PEP 701 f-string full grammar.

Behavior: nested f-strings, debug expressions, and multi-line f-string expressions.
Parity: ensure Molt matches CPython 3.12+ f-string parsing/formatting semantics.
Pitfalls: requires a 3.12+ parser; older hosts will fail to parse this file.
"""


def main() -> None:
    value = 2
    nested = f"{f'{value + 1}'}"
    continued = f"{(1 +
        2)}"
    debug = f"{value=}"
    print(nested)
    print(continued)
    print(debug)


main()
