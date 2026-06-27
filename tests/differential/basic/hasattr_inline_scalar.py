"""Purpose: differential coverage for hasattr/getattr on inline NaN-boxed scalar
receivers (inline int, bool, inline float) plus heap scalars (bigint, str).

Regression for the bug where ``hasattr(42, "bit_length")`` returned False: the
inline-scalar receiver path in ``molt_has_attr_name`` only handled ``__class__``
and never consulted the int/bool/float method tables that ``getattr`` already
used. ``hasattr``, ``getattr``, and ``getattr(_, default)`` now route every
inline scalar through one shared resolver, so they can never disagree about which
attributes a scalar exposes. The heap bigint branch was likewise made symmetric
with heap float (it gained the ``int``-class + ``object`` fallback).
"""


def agree(obj, name):
    """hasattr must equal 'getattr does not raise AttributeError' — CPython's own
    definition of hasattr. This holds in both interpreters whenever the hasattr
    and getattr resolution paths are consistent, independent of how complete the
    underlying method tables are, so it is the strongest cross-interpreter check
    of the structural invariant this fix establishes."""
    h = hasattr(obj, name)
    try:
        getattr(obj, name)
        g = True
    except AttributeError:
        g = False
    return h == g


# ---------------------------------------------------------------------------
# Part 1: direct hasattr parity for the scalar method/dunder surface molt
# resolves. Every name here is a real CPython attribute -> expect True; the
# "absent" rows are real non-attributes -> expect False.
# ---------------------------------------------------------------------------
int_present = [
    "bit_length", "bit_count", "to_bytes", "from_bytes", "as_integer_ratio",
    "conjugate", "is_integer", "__add__", "__abs__", "__and__", "__bool__",
    "__int__", "__index__", "__hash__", "__class__", "__init__", "__new__",
    "__repr__", "__str__", "__eq__", "__ne__", "__format__", "__dir__",
]
for nm in int_present:
    print("int", nm, hasattr(42, nm))
for nm in ["definitely_not_here", "no_such_method", "upper", "append"]:
    print("int-absent", nm, hasattr(42, nm))
print("int0", hasattr(0, "bit_length"))
print("int-neg", hasattr(-5, "to_bytes"))

bool_present = [
    "bit_length", "to_bytes", "__and__", "__bool__", "__class__", "__init__",
    "conjugate", "__hash__", "__eq__", "__repr__",
]
for nm in bool_present:
    print("bool", nm, hasattr(True, nm))
print("bool-absent", hasattr(False, "upper"))

float_present = [
    "is_integer", "hex", "as_integer_ratio", "conjugate", "fromhex",
    "__float__", "__class__", "__init__", "__hash__", "__repr__", "__str__",
    "__eq__", "__format__",
]
for nm in float_present:
    print("float", nm, hasattr(3.0, nm))
for nm in ["bit_length", "no_such_attr", "upper"]:
    print("float-absent", nm, hasattr(2.5, nm))

print("str upper", hasattr("hi", "upper"))
print("str len", hasattr("hi", "__len__"))
print("str absent", hasattr("hi", "bit_length"))

big = 10 ** 100
for nm in ["bit_length", "to_bytes", "conjugate", "__init__", "__hash__",
           "__class__", "__repr__", "__eq__"]:
    print("big", nm, hasattr(big, nm))
print("big-absent", hasattr(big, "definitely_absent"))

# ---------------------------------------------------------------------------
# Part 2: structural invariant — hasattr agrees with getattr for ANY name,
# including ones whose presence in molt's curated tables is incomplete (the
# printed value is the *agreement*, True in both interpreters when the two paths
# are consistent). This locks in that hasattr and getattr can never drift.
# ---------------------------------------------------------------------------
probe_names = [
    "bit_length", "to_bytes", "__add__", "__mul__", "__sub__", "__floordiv__",
    "__lshift__", "__or__", "__neg__", "real", "imag", "numerator",
    "denominator", "is_integer", "hex", "__class__", "__init__", "__doc__",
    "__sizeof__", "__reduce__", "nonexistent_attr_xyz", "upper", "__len__",
]
for obj, label in [(42, "int"), (True, "bool"), (3.0, "float"),
                   (10 ** 100, "big"), ("hi", "str")]:
    for nm in probe_names:
        print("agree", label, nm, agree(obj, nm))

# ---------------------------------------------------------------------------
# Part 3: resolved attributes are actually usable, and getattr-with-default
# returns the resolved attribute (not the default) when it exists — exercising
# the getattr(_, default) inline path that previously skipped the class-based
# fallback for inherited dunders such as object.__init__.
# ---------------------------------------------------------------------------
print((42).bit_length())
print((255).to_bytes(2, "big"))
print((10).as_integer_ratio())
print((-7).bit_length())
print(True.bit_length())
print((3.5).is_integer())
print((4.0).is_integer())
print((3.0).hex())
print(big.bit_length())
print(getattr(42, "bit_length")())
print(getattr(7, "__add__")(3))

print(callable(getattr(42, "bit_length", "DFLT")))
print(callable(getattr(42, "__init__", "DFLT")))
print(callable(getattr(3.0, "is_integer", "DFLT")))
print(callable(getattr(3.0, "__init__", "DFLT")))
print(callable(getattr(True, "bit_length", "DFLT")))
print(callable(getattr(big, "__init__", "DFLT")))
print(getattr(42, "missing_attr_zzz", "DFLT"))
print(getattr(3.0, "missing_attr_zzz", "DFLT"))
