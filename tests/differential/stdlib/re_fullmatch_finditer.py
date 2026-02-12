"""Purpose: differential coverage for re fullmatch finditer."""

import re


text = "abc123"
print(bool(re.fullmatch(r"[a-z]+\d+", text)))
print([m.group(0) for m in re.finditer(r"\d", text)])
