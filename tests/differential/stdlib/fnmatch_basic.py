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
