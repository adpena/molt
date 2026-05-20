"""Purpose: dict subclass indexing honors __missing__ mapping semantics."""


class MissingDict(dict):
    def __missing__(self, key):
        print("missing", key)
        return 42


class PlainDictSubclass(dict):
    pass


d = MissingDict()
print("value", d["x"])

try:
    PlainDictSubclass()["y"]
    print("noerror")
except KeyError as exc:
    print("keyerror", exc.args[0])

try:
    print("plain", PlainDictSubclass()["z"])
except KeyError as exc:
    print("keyerror", exc.args[0])
