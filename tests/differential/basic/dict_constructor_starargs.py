"""Purpose: differential coverage for dict(*args, **kwargs) constructor patterns."""


# dict() — no args
d1 = dict()
print(f"dict()={d1}")

# dict(key=val) — keyword-only
d2 = dict(a=1, b=2)
print(f"dict(a=1,b=2)={d2}")

# dict(iterable) — from iterable of pairs
d3 = dict([("x", 10), ("y", 20)])
print(f"dict(pairs)={d3}")

# dict(mapping) — from another dict
d4 = dict({"p": 100, "q": 200})
print(f"dict(mapping)={d4}")

# dict(iterable, key=val) — iterable + kwargs
d5 = dict([("a", 1)], b=2, c=3)
print(f"dict(iter,kw)={d5}")

# dict(**mapping) — kwstar
m = {"x": 1, "y": 2}
d6 = dict(**m)
print(f"dict(**m)={d6}")

# dict(*args) where args is (mapping,)
args_tuple = ({"k1": "v1", "k2": "v2"},)
d7 = dict(*args_tuple)
print(f"dict(*args)={d7}")

# dict(*args, **kwargs) — combined
base = ({"a": 1},)
extra = {"b": 2, "c": 3}
d8 = dict(*base, **extra)
print(f"dict(*args,**kw)={d8}")

# {**a, **b} — dict unpacking display
a = {"x": 1}
b = {"y": 2, "z": 3}
d9 = {**a, **b}
print(f"unpack={d9}")

# {**a, **b} with overlap — last wins
c = {"x": 1, "y": 2}
dd = {"y": 99, "z": 3}
d10 = {**c, **dd}
print(f"overlap={d10}")

# dict(**kw) with non-dict mapping
class MyMapping:
    def keys(self):
        return ["alpha", "beta"]
    def __getitem__(self, key):
        return {"alpha": 111, "beta": 222}[key]

d11 = dict(**MyMapping())
print(f"dict(**custom)={d11}")


# Error cases
def show_err(label, func):
    try:
        func()
    except Exception as exc:
        print(f"{label}:{type(exc).__name__}:{exc}")


def star_too_many():
    # dict(*[a, b]) unpacks to dict(a, b) → TypeError
    dict(*[("a", 1), ("b", 2)])


def kwstar_non_string_key():
    dict(**{1: "bad"})


show_err("star-too-many", star_too_many)
show_err("kwstar-non-str", kwstar_non_string_key)
