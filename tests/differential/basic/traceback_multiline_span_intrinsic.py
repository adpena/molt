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

entries = [(filename, 1, 3, 8, 5, "demo", "alpha = (")]
formatted = traceback.format_list(entries)
caret_lines = [line for line in formatted if "^" in line]

print(any("alpha = (" in line for line in formatted))
print(any("1 +" in line for line in formatted))
print(any("2" in line for line in formatted))
print(len(caret_lines) >= 2)
