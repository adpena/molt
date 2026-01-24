"""Purpose: differential coverage for re posix class."""

import re


text = "abc123_"  # includes underscore
pattern = re.compile(r"[[:alnum:]]+")
print(pattern.findall(text))
