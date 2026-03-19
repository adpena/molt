# Shim churn audit: 38 intrinsic-direct / 41 total exports
"""Intrinsic-first stdlib module for `_operator`.

All pure-forwarding shims have been eliminated (MOL-215). Functions are wired
directly to their Rust intrinsic callables.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


# --- Direct intrinsic bindings (no Python wrapper overhead) ---

abs = _require_intrinsic("molt_operator_abs")
add = _require_intrinsic("molt_operator_add")
sub = _require_intrinsic("molt_operator_sub")
mul = _require_intrinsic("molt_operator_mul")
matmul = _require_intrinsic("molt_operator_matmul")
truediv = _require_intrinsic("molt_operator_truediv")
floordiv = _require_intrinsic("molt_operator_floordiv")
mod = _require_intrinsic("molt_operator_mod")
pow = _require_intrinsic("molt_operator_pow")
lshift = _require_intrinsic("molt_operator_lshift")
rshift = _require_intrinsic("molt_operator_rshift")
and_ = _require_intrinsic("molt_operator_and")
or_ = _require_intrinsic("molt_operator_or")
xor = _require_intrinsic("molt_operator_xor")
neg = _require_intrinsic("molt_operator_neg")
pos = _require_intrinsic("molt_operator_pos")
invert = _require_intrinsic("molt_operator_invert")
not_ = _require_intrinsic("molt_operator_not")
truth = _require_intrinsic("molt_operator_truth")
eq = _require_intrinsic("molt_operator_eq")
ne = _require_intrinsic("molt_operator_ne")
lt = _require_intrinsic("molt_operator_lt")
le = _require_intrinsic("molt_operator_le")
gt = _require_intrinsic("molt_operator_gt")
ge = _require_intrinsic("molt_operator_ge")
is_ = _require_intrinsic("molt_operator_is")
is_not = _require_intrinsic("molt_operator_is_not")
contains = _require_intrinsic("molt_operator_contains")
getitem = _require_intrinsic("molt_operator_getitem")
setitem = _require_intrinsic("molt_operator_setitem")
delitem = _require_intrinsic("molt_operator_delitem")
countOf = _require_intrinsic("molt_operator_countof")
length_hint = _require_intrinsic("molt_operator_length_hint")
concat = _require_intrinsic("molt_operator_concat")
iconcat = _require_intrinsic("molt_operator_iconcat")
iadd = _require_intrinsic("molt_operator_iadd")
isub = _require_intrinsic("molt_operator_isub")
imul = _require_intrinsic("molt_operator_imul")
imatmul = _require_intrinsic("molt_operator_imatmul")
itruediv = _require_intrinsic("molt_operator_itruediv")
ifloordiv = _require_intrinsic("molt_operator_ifloordiv")
imod = _require_intrinsic("molt_operator_imod")
ipow = _require_intrinsic("molt_operator_ipow")
ilshift = _require_intrinsic("molt_operator_ilshift")
irshift = _require_intrinsic("molt_operator_irshift")
iand = _require_intrinsic("molt_operator_iand")
ior = _require_intrinsic("molt_operator_ior")
ixor = _require_intrinsic("molt_operator_ixor")
index = _require_intrinsic("molt_operator_index")

# Type-factory intrinsics (these return class objects, not instances)
_MOLT_ITEMGETTER_TYPE = _require_intrinsic("molt_operator_itemgetter_type")
_MOLT_ATTRGETTER_TYPE = _require_intrinsic("molt_operator_attrgetter_type")
_MOLT_METHODCALLER_TYPE = _require_intrinsic("molt_operator_methodcaller_type")

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
