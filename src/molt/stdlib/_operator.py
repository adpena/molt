# Shim churn audit: 38 intrinsic-direct / 41 total exports
"""Intrinsic-first stdlib module for `_operator`.

All pure-forwarding shims have been eliminated (MOL-215). Functions are wired
directly to their Rust intrinsic callables.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


# --- Direct intrinsic bindings (no Python wrapper overhead) ---

abs = _require_intrinsic("molt_operator_abs", globals())
add = _require_intrinsic("molt_operator_add", globals())
sub = _require_intrinsic("molt_operator_sub", globals())
mul = _require_intrinsic("molt_operator_mul", globals())
matmul = _require_intrinsic("molt_operator_matmul", globals())
truediv = _require_intrinsic("molt_operator_truediv", globals())
floordiv = _require_intrinsic("molt_operator_floordiv", globals())
mod = _require_intrinsic("molt_operator_mod", globals())
pow = _require_intrinsic("molt_operator_pow", globals())
lshift = _require_intrinsic("molt_operator_lshift", globals())
rshift = _require_intrinsic("molt_operator_rshift", globals())
and_ = _require_intrinsic("molt_operator_and", globals())
or_ = _require_intrinsic("molt_operator_or", globals())
xor = _require_intrinsic("molt_operator_xor", globals())
neg = _require_intrinsic("molt_operator_neg", globals())
pos = _require_intrinsic("molt_operator_pos", globals())
invert = _require_intrinsic("molt_operator_invert", globals())
not_ = _require_intrinsic("molt_operator_not", globals())
truth = _require_intrinsic("molt_operator_truth", globals())
eq = _require_intrinsic("molt_operator_eq", globals())
ne = _require_intrinsic("molt_operator_ne", globals())
lt = _require_intrinsic("molt_operator_lt", globals())
le = _require_intrinsic("molt_operator_le", globals())
gt = _require_intrinsic("molt_operator_gt", globals())
ge = _require_intrinsic("molt_operator_ge", globals())
is_ = _require_intrinsic("molt_operator_is", globals())
is_not = _require_intrinsic("molt_operator_is_not", globals())
contains = _require_intrinsic("molt_operator_contains", globals())
getitem = _require_intrinsic("molt_operator_getitem", globals())
setitem = _require_intrinsic("molt_operator_setitem", globals())
delitem = _require_intrinsic("molt_operator_delitem", globals())
countOf = _require_intrinsic("molt_operator_countof", globals())
length_hint = _require_intrinsic("molt_operator_length_hint", globals())
concat = _require_intrinsic("molt_operator_concat", globals())
iconcat = _require_intrinsic("molt_operator_iconcat", globals())
iadd = _require_intrinsic("molt_operator_iadd", globals())
isub = _require_intrinsic("molt_operator_isub", globals())
imul = _require_intrinsic("molt_operator_imul", globals())
imatmul = _require_intrinsic("molt_operator_imatmul", globals())
itruediv = _require_intrinsic("molt_operator_itruediv", globals())
ifloordiv = _require_intrinsic("molt_operator_ifloordiv", globals())
imod = _require_intrinsic("molt_operator_imod", globals())
ipow = _require_intrinsic("molt_operator_ipow", globals())
ilshift = _require_intrinsic("molt_operator_ilshift", globals())
irshift = _require_intrinsic("molt_operator_irshift", globals())
iand = _require_intrinsic("molt_operator_iand", globals())
ior = _require_intrinsic("molt_operator_ior", globals())
ixor = _require_intrinsic("molt_operator_ixor", globals())
index = _require_intrinsic("molt_operator_index", globals())

# Type-factory intrinsics (these return class objects, not instances)
_MOLT_ITEMGETTER_TYPE = _require_intrinsic("molt_operator_itemgetter_type", globals())
_MOLT_ATTRGETTER_TYPE = _require_intrinsic("molt_operator_attrgetter_type", globals())
_MOLT_METHODCALLER_TYPE = _require_intrinsic(
    "molt_operator_methodcaller_type", globals()
)

itemgetter = _MOLT_ITEMGETTER_TYPE()
attrgetter = _MOLT_ATTRGETTER_TYPE()
methodcaller = _MOLT_METHODCALLER_TYPE()

inv = invert

__all__ = [
    "abs",
    "add",
    "and_",
    "attrgetter",
    "concat",
    "contains",
    "countOf",
    "delitem",
    "eq",
    "floordiv",
    "ge",
    "getitem",
    "gt",
    "iadd",
    "iand",
    "iconcat",
    "ifloordiv",
    "ilshift",
    "imatmul",
    "imod",
    "imul",
    "index",
    "inv",
    "invert",
    "ior",
    "ipow",
    "irshift",
    "is_",
    "is_not",
    "isub",
    "itemgetter",
    "itruediv",
    "ixor",
    "le",
    "length_hint",
    "lshift",
    "lt",
    "matmul",
    "methodcaller",
    "mod",
    "mul",
    "ne",
    "neg",
    "not_",
    "or_",
    "pos",
    "pow",
    "rshift",
    "setitem",
    "sub",
    "truediv",
    "truth",
    "xor",
]
