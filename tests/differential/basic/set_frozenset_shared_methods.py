"""Purpose: set and frozenset methods that share a body keep distinct identities.

The frozenset methods only call the set bodies, so an optimizing build may give
both methods one code address (HF-881). Every call below goes through a
dynamically selected method object, so the runtime binder must tell the two
families apart by identity, not by address.
"""


def call(owner, name, *args):
    method = getattr(owner, name)
    return method(*args)


def show(label, thunk):
    try:
        result = thunk()
    except Exception as exc:  # noqa: BLE001 - the exception type is the oracle
        print(label, "error", type(exc).__name__, exc)
    else:
        if isinstance(result, (set, frozenset)):
            result = (type(result).__name__, sorted(result))
        print(label, result)


fs = frozenset({1, 2})
s = {1, 2}
other = [1, 2, 3]

UNARY = ["issubset", "issuperset", "isdisjoint", "union", "intersection",
         "difference", "symmetric_difference"]

for name in UNARY:
    show(f"frozenset.{name} bound", lambda name=name: call(fs, name, other))
    show(f"set.{name} bound", lambda name=name: call(s, name, other))
    show(f"frozenset.{name} descriptor",
         lambda name=name: getattr(frozenset, name)(fs, other))
    show(f"set.{name} descriptor", lambda name=name: getattr(set, name)(s, other))
    # Unpacked and keyword calls go through the identity-keyed binder.
    show(f"frozenset.{name} star", lambda name=name: getattr(fs, name)(*[other]))
    show(f"set.{name} star", lambda name=name: getattr(s, name)(*[other]))
    show(f"frozenset.{name} empty kwargs",
         lambda name=name: getattr(fs, name)(other, **{}))
    show(f"frozenset.{name} descriptor star",
         lambda name=name: getattr(frozenset, name)(*[fs, other]))
    show(f"set.{name} descriptor star",
         lambda name=name: getattr(set, name)(*[s, other]))
    # A descriptor of one family refuses a receiver of the other.
    show(f"set.{name} on frozenset",
         lambda name=name: getattr(set, name)(fs, other))
    show(f"frozenset.{name} on set",
         lambda name=name: getattr(frozenset, name)(s, other))

for name in ["copy", "__len__", "__contains__"]:
    args = (1,) if name == "__contains__" else ()
    show(f"frozenset.{name}", lambda name=name: call(fs, name, *args))
    show(f"set.{name}", lambda name=name: call(s, name, *args))

copy_method = getattr(fs, "copy")
print("frozenset copy identity", copy_method() is fs)
print("set copy identity", getattr(s, "copy")() is s)
print("iterator types", type(getattr(fs, "__iter__")()).__name__,
      type(getattr(s, "__iter__")()).__name__)
print("method names", getattr(fs, "issubset").__qualname__,
      getattr(s, "issubset").__qualname__)
print("descriptor identity", frozenset.issubset is set.issubset,
      frozenset.issubset == set.issubset)
