"""Builtin exception classes called with keywords construct exactly as CPython.

AttributeError takes name and obj, NameError takes name, the ImportError family
takes name, path and name_from; every other class refuses keywords. Unknown,
excess and refused keywords raise CPython's errors. Positional arguments
evaluate before keyword values, left to right.
"""


class Owner:
    pass


owner = Owner()
attribute = AttributeError("missing", name="field", obj=owner)
print("attribute", attribute.args, attribute.name, attribute.obj is owner, str(attribute))
only_keywords = AttributeError(obj=owner)
print("attribute keywords only", only_keywords.args, only_keywords.name, only_keywords.obj is owner)
name_error = NameError("undefined", name="spam")
print("name", name_error.args, name_error.name)
unbound = UnboundLocalError("local", name="x")
print("unbound", unbound.args, unbound.name)
import_error = ImportError("no module", name="pkg", path="/tmp/pkg.py")
print("import", import_error.args, import_error.msg, import_error.name, import_error.path)
not_found = ModuleNotFoundError("gone", name="gone_module")
print("not found", type(not_found).__name__, not_found.name, not_found.path)
from_error = ImportError("cannot import", name="mod", name_from="thing")
print("name_from", from_error.name_from)

for label, make in (
    ("value keyword", lambda: ValueError("x", code=1)),
    ("oserror keyword", lambda: OSError(2, "missing", filename="f")),
    ("attribute unknown", lambda: AttributeError("m", bad=1)),
    ("name unknown", lambda: NameError(obj=1)),
    ("import unknown", lambda: ImportError(module="m")),
    ("key keyword", lambda: KeyError(name="k")),
):
    try:
        make()
    except TypeError as exc:
        print("refused", label, exc)

events = []


def mark(value):
    events.append(value)
    return value


ordered = AttributeError(mark("first"), mark("second"), obj=mark("obj"), name=mark("name"))
print("order", events, ordered.args, ordered.name, ordered.obj)

try:
    raise AttributeError("raised", name="attr", obj=None)
except AttributeError as exc:
    print("raised", exc.args, exc.name, exc.obj)
