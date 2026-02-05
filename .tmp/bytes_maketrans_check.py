print(hasattr(bytes, "maketrans"))
try:
    table = bytes.maketrans(b"ab", b"cd")
    print(type(table).__name__, len(table), table[ord("a")], table[ord("b")])
except Exception as exc:
    print(type(exc).__name__, exc)
