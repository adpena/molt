"""Purpose: differential coverage for re subn escape."""

import re


text = "a1b2"
print(re.subn(r"\d", "X", text))
print(re.escape("a.b+c?"))
