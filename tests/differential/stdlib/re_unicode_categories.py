"""Purpose: differential coverage for re unicode categories."""

import re


text = "\x11\x03\na_1"
print(re.findall(r"\w+", text))
print(re.findall(r"\W+", text))
print(bool(re.search(r"\d+", text)))
