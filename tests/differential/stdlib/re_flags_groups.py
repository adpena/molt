"""Purpose: differential coverage for re flags groups."""

import re


text = "Line1\nLine2\nLINE3"
pattern = re.compile(r"^line", re.IGNORECASE | re.MULTILINE)
print(pattern.findall(text))

m = re.search(r"(?P<word>Line)\d", text)
print(m.group(0), m.group("word"))

m2 = re.search(r"(Line)(\d)\n(.*)", text, re.DOTALL)
print(m2.group(1), m2.group(2), m2.group(3))

print(re.sub(r"(Line)(\d)", r"\1-\2", text))
