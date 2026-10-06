"""Purpose: validate multiline traceback caret shaping from intrinsic payload spans."""

import os
import tempfile
import traceback


root = tempfile.mkdtemp(prefix="molt_traceback_multiline_")
filename = os.path.join(root, "sample.py")
with open(filename, "w", encoding="utf-8") as handle:
    handle.write("alpha = (\n")
    handle.write("    1 +\n")
    handle.write("    2\n")
    handle.write(")\n")

entries = [
    traceback.FrameSummary(
        filename=filename,
        lineno=1,
        end_lineno=3,
        colno=8,
        end_colno=5,
        name="demo",
        line=None,
    ),
]
formatted = "".join(traceback.format_list(entries)).splitlines()
caret_lines = [line for line in formatted if "^" in line]

print(any(line.strip() == "alpha = (" for line in formatted))
print(any(line.strip() == "1 +" for line in formatted))
print(any(line.strip() == "2" for line in formatted))
print(len(caret_lines) >= 2)

# Exact spacing and multi-line anchor placement, excluding only the temporary
# filename header. Captured source must survive later file modification.
print("rendered", repr("\n".join(formatted[1:])))
with open(filename, "w", encoding="utf-8") as handle:
    handle.write("changed = 1\n")
print("captured_source", "".join(traceback.format_list(entries)).splitlines()[1:] == formatted[1:])

for source, first, last, start, end in (
    ("    value = (\n        a +\n        b\n    )", 2, 3, 8, 9),
    ("value = f(\n    a,\n    b,\n)", 1, 4, 8, 1),
    ("value = (a\n    + b\n    + c\n    + d\n    + e\n    + f\n    + g\n)", 1, 7, 9, 7),
):
    with open(filename, "w", encoding="utf-8") as handle:
        handle.write(source)
    # Distinct filenames prevent CPython's linecache from reusing prior source.
    distinct = filename + str(first) + str(last)
    with open(distinct, "w", encoding="utf-8") as handle:
        handle.write(source)
    frame = traceback.FrameSummary(
        filename=distinct, lineno=first, end_lineno=last, colno=start,
        end_colno=end, name="span", line=None,
    )
    print("span", repr("\n".join("".join(traceback.format_list([frame])).splitlines()[1:])))

# Filesystem source and explicit source differ in 3.12: linecache adds a final
# newline, while an explicit line is preserved exactly. Trailing spaces count.
for index, suffix in enumerate(("", "\n", "  \n", "\r\n")):
    source = "    value = source" + suffix
    distinct = filename + "ending" + str(index)
    with open(distinct, "w", encoding="utf-8", newline="") as handle:
        handle.write(source)
    for line in (None, source):
        frame = traceback.FrameSummary(
            filename=distinct, lineno=1, end_lineno=1, colno=12,
            end_colno=18, name="ending", line=line,
        )
        print("ending", index, line is None,
              repr("\n".join("".join(traceback.format_list([frame])).splitlines()[1:])))
