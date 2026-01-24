"""Purpose: differential coverage for re split maxsplit."""

import re


text = "a1b2c3"
print(re.split(r"\d", text))
print(re.split(r"\d", text, maxsplit=2))
