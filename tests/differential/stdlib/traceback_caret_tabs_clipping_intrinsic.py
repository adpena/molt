"""Purpose: validate traceback caret rendering for tabs and clipping edge cases."""

import traceback


entries = [
    traceback.FrameSummary(
        filename="<tabs-case>",
        lineno=5,
        end_lineno=5,
        colno=1,
        end_colno=999,
        name="boom",
        line="\tassert value and (",
    ),
]
formatted = "".join(traceback.format_list(entries)).splitlines()
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

# Compare exact public rendering against CPython. Quoted punctuation, nested
# operators and keyword arguments must not become guessed operator anchors.
for text, start, end in (
    ("'a + b'", 0, 7),
    ("a + b * c", 0, 9),
    ("(a + b) * c", 0, 11),
    ("f(key=1)", 0, 8),
    ("a['[x]']", 0, 8),
    ("a == b", 0, 6),
    ("x = a + b", 4, 9),
    ("x = obj.attr", 4, 12),
    ("x = a == b", 4, 10),
    ("x = café / 0", 4, 12),
    ("éé / 0", 0, 6),
    ("漢字 / 0", 0, 6),
    ("(éé) / 0", 0, 8),
    ("éé ** 0", 0, 7),
    ("éé + z + q", 0, 10),
    ("a+(b)", 0, 5),
    ("a\t+\tb", 2, 5),
    ("return f()", 7, 10),
):
    frame = traceback.FrameSummary(
        filename="<anchor-case>",
        lineno=1,
        end_lineno=1,
        colno=len(text[:start].encode("utf-8")),
        end_colno=len(text[:end].encode("utf-8")),
        name="probe",
        line=text,
    )
    print("anchor", repr("".join(traceback.format_list([frame]))))

# Public positions are UTF-8 bytes, including partial code points. Tracebacks
# use replacement decoding and East Asian widths, not UTF-8 boundary flooring
# or a generic terminal-width function (combining marks still occupy a column).
for text, start, end in (
    ("漢 + b", 1, 7),
    ("é + b", 1, 6),
    ("x = 'Ｘ' + b", 4, 15),
    ("x = 'a\u0301' + b", 4, 15),
    ("x = '⌚' + b", 4, 15),
    ("x = '\U0001fae9' + b", 4, 16),
):
    frame = traceback.FrameSummary(
        filename="<byte-case>", lineno=1, name="probe", line=text,
        end_lineno=1, colno=start, end_colno=end,
    )
    print("bytes", repr("".join(traceback.format_list([frame]))))
