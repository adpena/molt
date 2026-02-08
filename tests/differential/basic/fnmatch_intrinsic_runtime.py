from __future__ import annotations

import fnmatch


cases = [
    ("foo.txt", "*.txt"),
    ("]", "[]]"),
    ("[", "[[]"),
    ("[]", "[]"),
    ("[!", "[!"),
    ("a", "[z-a]"),
    ("-", "[z-a-]"),
    ("e", "[b-ae]"),
    ("a", "[b-ae]"),
    ("^", "[^]"),
    ("a", "[^]"),
    ("*", "[*]"),
    ("?", "[?]"),
    ("!", "[!!]"),
    ("a", "[!!]"),
    ("]", "[!]]"),
    ("a", "[!]]"),
    ("[abc", "[abc"),
    ("[", "[abc"),
]

print("matches")
for name, pat in cases:
    print(repr(name), repr(pat), fnmatch.fnmatchcase(name, pat))

names = ["a.py", "b.py", "README", "notes.txt"]
print("filter", fnmatch.filter(names, "*.py"))
print("translate", fnmatch.translate("*.py"))
