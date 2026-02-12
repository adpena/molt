"""Purpose: validate traceback caret rendering for tabs and clipping edge cases."""

import traceback


entries = [
    (
        "<tabs-case>",
        5,
        5,
        1,
        999,
        "boom",
        "\tassert value and (",
    )
]
formatted = traceback.format_list(entries)
source_lines = [line for line in formatted if "assert value and (" in line]
caret_lines = [line for line in formatted if "^" in line]
first_caret = caret_lines[0] if caret_lines else ""

print(bool(source_lines))
print(bool(caret_lines))
print(first_caret.startswith("    \t") if first_caret else False)
print(
    first_caret.count("^") <= len(source_lines[0].rstrip())
    if source_lines and first_caret
    else False
)
