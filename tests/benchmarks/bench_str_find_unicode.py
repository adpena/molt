haystack = ("na\u00efve" * 2_000_000) + "\u2603"
print(haystack.find("\u2603"))
