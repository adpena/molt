"""Purpose: differential coverage for PEP 701 edge-case f-string behavior.

Behavior: debug-expression comments/newlines, nested format-spec expressions,
and backslash-containing string literals inside replacement fields.
Parity: ensure Molt matches CPython 3.12+ parser/evaluation behavior.
Pitfalls: requires a 3.12+ parser; older hosts fail to parse this file.
"""


def main() -> None:
    value = 7
    debug_comment = f"""{(
        value + 1  # comment is legal in debug expressions on 3.12+
    )=}"""
    debug_newline = f"""{(
        value
        + 2
    )=}"""

    assert debug_comment.endswith("=8"), debug_comment
    assert "comment is legal" not in debug_comment, debug_comment
    assert "\n" in debug_comment, debug_comment
    assert debug_newline.endswith("=9"), debug_newline
    assert "\n" in debug_newline, debug_newline
    backslash_expr = f"{'\\n'}"
    assert backslash_expr == "\\n", backslash_expr

    calls: list[str] = []

    def width() -> int:
        calls.append("width")
        return 7

    def precision() -> int:
        calls.append("precision")
        return 2

    formatted = f"""{12.3456:{(width() + 0)}.{(precision())}f}"""

    assert formatted == "  12.35", formatted
    assert calls == ["width", "precision"], calls

    print(debug_comment)
    print(debug_newline)
    print(backslash_expr)
    print(formatted)
    print(calls)


main()
