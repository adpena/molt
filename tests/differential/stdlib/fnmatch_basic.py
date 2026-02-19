"""Purpose: differential coverage for fnmatch basic."""

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

for name, pat in cases:
    print(repr(name), repr(pat), fnmatch.fnmatchcase(name, pat))

bytes_cases = [
    (b"foo.txt", b"*.txt"),
    (b"]", b"[]]"),
    (b"[", b"[[]"),
    (b"[]", b"[]"),
    (b"[!", b"[!"),
    (b"a", b"[z-a]"),
    (b"-", b"[z-a-]"),
    (b"e", b"[b-ae]"),
    (b"a", b"[b-ae]"),
    (b"^", b"[^]"),
    (b"a", b"[^]"),
    (b"*", b"[*]"),
    (b"?", b"[?]"),
    (b"!", b"[!!]"),
    (b"a", b"[!!]"),
    (b"]", b"[!]]"),
    (b"a", b"[!]]"),
    (b"[abc", b"[abc"),
    (b"[", b"[abc"),
]

for name, pat in bytes_cases:
    print(repr(name), repr(pat), fnmatch.fnmatchcase(name, pat))

print("normcase", fnmatch.fnmatch("Foo.TXT", "*.txt"))
print("normcase_bytes", fnmatch.fnmatch(b"Foo.TXT", b"*.txt"))
