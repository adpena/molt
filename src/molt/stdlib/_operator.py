"""Intrinsic-first stdlib module for `_operator`."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


_MOLT_ABS = _require_intrinsic("molt_operator_abs", globals())
_MOLT_ADD = _require_intrinsic("molt_operator_add", globals())
_MOLT_SUB = _require_intrinsic("molt_operator_sub", globals())
_MOLT_MUL = _require_intrinsic("molt_operator_mul", globals())
_MOLT_MATMUL = _require_intrinsic("molt_operator_matmul", globals())
_MOLT_TRUEDIV = _require_intrinsic("molt_operator_truediv", globals())
_MOLT_FLOORDIV = _require_intrinsic("molt_operator_floordiv", globals())
_MOLT_MOD = _require_intrinsic("molt_operator_mod", globals())
_MOLT_POW = _require_intrinsic("molt_operator_pow", globals())
_MOLT_LSHIFT = _require_intrinsic("molt_operator_lshift", globals())
_MOLT_RSHIFT = _require_intrinsic("molt_operator_rshift", globals())
_MOLT_AND = _require_intrinsic("molt_operator_and", globals())
_MOLT_OR = _require_intrinsic("molt_operator_or", globals())
_MOLT_XOR = _require_intrinsic("molt_operator_xor", globals())
_MOLT_NEG = _require_intrinsic("molt_operator_neg", globals())
_MOLT_POS = _require_intrinsic("molt_operator_pos", globals())
_MOLT_INVERT = _require_intrinsic("molt_operator_invert", globals())
_MOLT_NOT = _require_intrinsic("molt_operator_not", globals())
_MOLT_TRUTH = _require_intrinsic("molt_operator_truth", globals())
_MOLT_EQ = _require_intrinsic("molt_operator_eq", globals())
_MOLT_NE = _require_intrinsic("molt_operator_ne", globals())
_MOLT_LT = _require_intrinsic("molt_operator_lt", globals())
_MOLT_LE = _require_intrinsic("molt_operator_le", globals())
_MOLT_GT = _require_intrinsic("molt_operator_gt", globals())
_MOLT_GE = _require_intrinsic("molt_operator_ge", globals())
_MOLT_IS = _require_intrinsic("molt_operator_is", globals())
_MOLT_IS_NOT = _require_intrinsic("molt_operator_is_not", globals())
_MOLT_CONTAINS = _require_intrinsic("molt_operator_contains", globals())
_MOLT_GETITEM = _require_intrinsic("molt_operator_getitem", globals())
_MOLT_SETITEM = _require_intrinsic("molt_operator_setitem", globals())
_MOLT_DELITEM = _require_intrinsic("molt_operator_delitem", globals())
_MOLT_COUNTOF = _require_intrinsic("molt_operator_countof", globals())
_MOLT_LENGTH_HINT = _require_intrinsic("molt_operator_length_hint", globals())
_MOLT_CONCAT = _require_intrinsic("molt_operator_concat", globals())
_MOLT_ICONCAT = _require_intrinsic("molt_operator_iconcat", globals())
_MOLT_IADD = _require_intrinsic("molt_operator_iadd", globals())
_MOLT_ISUB = _require_intrinsic("molt_operator_isub", globals())
_MOLT_IMUL = _require_intrinsic("molt_operator_imul", globals())
_MOLT_IMATMUL = _require_intrinsic("molt_operator_imatmul", globals())
_MOLT_ITRUEDIV = _require_intrinsic("molt_operator_itruediv", globals())
_MOLT_IFLOORDIV = _require_intrinsic("molt_operator_ifloordiv", globals())
_MOLT_IMOD = _require_intrinsic("molt_operator_imod", globals())
_MOLT_IPOW = _require_intrinsic("molt_operator_ipow", globals())
_MOLT_ILSHIFT = _require_intrinsic("molt_operator_ilshift", globals())
_MOLT_IRSHIFT = _require_intrinsic("molt_operator_irshift", globals())
_MOLT_IAND = _require_intrinsic("molt_operator_iand", globals())
_MOLT_IOR = _require_intrinsic("molt_operator_ior", globals())
_MOLT_IXOR = _require_intrinsic("molt_operator_ixor", globals())
_MOLT_INDEX = _require_intrinsic("molt_operator_index", globals())
_MOLT_ITEMGETTER_TYPE = _require_intrinsic("molt_operator_itemgetter_type", globals())
_MOLT_ATTRGETTER_TYPE = _require_intrinsic("molt_operator_attrgetter_type", globals())
_MOLT_METHODCALLER_TYPE = _require_intrinsic(
    "molt_operator_methodcaller_type", globals()
)

itemgetter = _MOLT_ITEMGETTER_TYPE()
attrgetter = _MOLT_ATTRGETTER_TYPE()
methodcaller = _MOLT_METHODCALLER_TYPE()


def abs(val):
    return _MOLT_ABS(val)


def add(a, b):
    return _MOLT_ADD(a, b)


def sub(a, b):
    return _MOLT_SUB(a, b)


def mul(a, b):
    return _MOLT_MUL(a, b)


def matmul(a, b):
    return _MOLT_MATMUL(a, b)


def truediv(a, b):
    return _MOLT_TRUEDIV(a, b)


def floordiv(a, b):
    return _MOLT_FLOORDIV(a, b)


def mod(a, b):
    return _MOLT_MOD(a, b)


def pow(a, b):
    return _MOLT_POW(a, b)


def lshift(a, b):
    return _MOLT_LSHIFT(a, b)


def rshift(a, b):
    return _MOLT_RSHIFT(a, b)


def and_(a, b):
    return _MOLT_AND(a, b)


def or_(a, b):
    return _MOLT_OR(a, b)


def xor(a, b):
    return _MOLT_XOR(a, b)


def neg(val):
    return _MOLT_NEG(val)


def pos(val):
    return _MOLT_POS(val)


def invert(val):
    return _MOLT_INVERT(val)


def not_(val):
    return _MOLT_NOT(val)


def truth(val):
    return _MOLT_TRUTH(val)


def eq(a, b):
    return _MOLT_EQ(a, b)


def ne(a, b):
    return _MOLT_NE(a, b)


def lt(a, b):
    return _MOLT_LT(a, b)


def le(a, b):
    return _MOLT_LE(a, b)


def gt(a, b):
    return _MOLT_GT(a, b)


def ge(a, b):
    return _MOLT_GE(a, b)


def is_(a, b):
    return _MOLT_IS(a, b)


def is_not(a, b):
    return _MOLT_IS_NOT(a, b)


def contains(container, item):
    return _MOLT_CONTAINS(container, item)


def getitem(obj, key):
    return _MOLT_GETITEM(obj, key)


def setitem(obj, key, value):
    return _MOLT_SETITEM(obj, key, value)


def delitem(obj, key):
    return _MOLT_DELITEM(obj, key)


def countOf(container, value):
    return _MOLT_COUNTOF(container, value)


def length_hint(obj, default=0):
    return _MOLT_LENGTH_HINT(obj, default)


def concat(a, b):
    return _MOLT_CONCAT(a, b)


def iconcat(a, b):
    return _MOLT_ICONCAT(a, b)


def iadd(a, b):
    return _MOLT_IADD(a, b)


def isub(a, b):
    return _MOLT_ISUB(a, b)


def imul(a, b):
    return _MOLT_IMUL(a, b)


def imatmul(a, b):
    return _MOLT_IMATMUL(a, b)


def itruediv(a, b):
    return _MOLT_ITRUEDIV(a, b)


def ifloordiv(a, b):
    return _MOLT_IFLOORDIV(a, b)


def imod(a, b):
    return _MOLT_IMOD(a, b)


def ipow(a, b):
    return _MOLT_IPOW(a, b)


def ilshift(a, b):
    return _MOLT_ILSHIFT(a, b)


def irshift(a, b):
    return _MOLT_IRSHIFT(a, b)


def iand(a, b):
    return _MOLT_IAND(a, b)


def ior(a, b):
    return _MOLT_IOR(a, b)


def ixor(a, b):
    return _MOLT_IXOR(a, b)


def index(obj):
    return _MOLT_INDEX(obj)


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
