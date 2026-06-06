"""Purpose: membership (`in` / `not in`) dispatch over hash + sequence containers.

Regression for the MEMBERSHIP-FAMILY cluster:

  * #43 (P0 SIGSEGV, native): a `set`/`dict`/`list`/`tuple` built by a
    constructor op (`set_new`/`list_new`/`dict_new`/`tuple_new`/`frozenset_new`)
    was lifted to a type-aliasing `OpCode::Copy` passthrough, so the
    representation plan mistyped the container as its first *element* (e.g. the
    first `str` of a `set`). The native `contains` dispatch then called the wrong
    specialized intrinsic — `molt_str_contains` on a set/dict — which read the
    container's bytes as a string and faulted. `x not in {"raise", "ignore"}`
    (the `csv.DictWriter` `extrasaction` check) was the original crash.
  * #52 (P0 wrong-result, native): the same mistyping made a `dict` `in` for an
    absent key return the wrong value instead of crashing, depending on which
    wrong intrinsic the mistype selected.
  * #53 (P1 wrong-result, LLVM): `emit_containment` passed the membership
    operands swapped — `3 in [1, 2, 3]` called `molt_contains(3, [1,2,3])` and
    raised "argument of type 'int' is not iterable".

Must be byte-identical to CPython 3.14 on BOTH the native and llvm targets.
Covers the {const/runtime str, const/runtime int} item × {set/dict/list/tuple
literal + constructed} container × {present, absent} × {in, not in} matrix, plus
the const-string-set membership in a class `__init__` (the csv shape) and
frozenset membership.
"""


# ── String item over string set (the #43 crash shape: const-str item). ──
print("raise" in {"raise", "ignore"})  # True
print("raise" not in {"raise", "ignore"})  # False
print("zzz" in {"raise", "ignore"})  # False
print("zzz" not in {"raise", "ignore"})  # True

# Module-scope const-str variable item (also crashed pre-fix).
mx = "raise"
print(mx in {"raise", "ignore"})  # True
print(mx not in {"raise", "ignore"})  # False

# Runtime (non-const) string item.
rx = "ra" + "ise"
print(rx in {"raise", "ignore"})  # True
print(rx not in {"raise", "ignore"})  # False


# ── Function-parameter string item over a set literal (the always-worked path,
#    kept as a regression guard so the fix does not perturb it). ──
def member(item):
    return item in {"raise", "ignore"}


print(member("raise"))  # True
print(member("zzz"))  # False


# ── String item over a constructed set. ──
cs = set(["raise", "ignore"])
print("raise" in cs)  # True
print("zzz" not in cs)  # True

# ── Int item over int set (regression guard — never mistyped). ──
print(1 in {1, 2})  # True
print(5 in {1, 2})  # False
print(1 not in {1, 2})  # False

# ── String / int keys over dict literals and constructed dicts (#52). ──
sd = {"a": 1, "b": 2}
print("a" in sd)  # True
print("zzz" in sd)  # False  (the absent-key #52 case)
print("zzz" not in sd)  # True
id_ = {1: 10, 2: 20}
print(1 in id_)  # True
print(99 in id_)  # False
sd2 = dict([("a", 1), ("b", 2)])
print("a" in sd2)  # True
print("zzz" in sd2)  # False

# ── List / tuple membership (the #53 LLVM-swap shape + #43 string-elem list). ──
print(3 in [1, 2, 3])  # True  (#53: was "argument of type 'int' is not iterable")
print(5 in [1, 2, 3])  # False
print("raise" in ["raise", "ignore"])  # True (string-elem list crashed pre-fix)
print("zzz" not in ["raise", "ignore"])  # True
print(2 in (1, 2, 3))  # True
print("raise" in ("raise", "ignore"))  # True
print("zzz" not in ("raise", "ignore"))  # True

# NOTE: frozenset membership is exercised on the native target by the
# `frozenset_new`-typing fix (its element-string set-probe was a #43 crash), but
# is intentionally NOT asserted in this cross-target differential: the LLVM
# backend's `frozenset(...)` builtin currently returns None (a separate,
# construction-level bug — `len(frozenset({...}))` raises "object of type
# 'NoneType' has no len()" on LLVM), which is unrelated to the membership
# dispatch fixed here. Asserting frozenset membership would gate this regression
# on that distinct LLVM bug.


# ── const-string set membership inside a class __init__ (the csv.DictWriter
#    extrasaction validation shape that triggered the original crash). ──
class Validated:
    def __init__(self, action):
        self.action = action.lower()
        if self.action not in {"raise", "ignore"}:
            raise ValueError(f"action ({action}) must be 'raise' or 'ignore'")


print(Validated("RAISE").action)  # raise
print(Validated("IGNORE").action)  # ignore
try:
    Validated("nope")
except ValueError as exc:
    print(type(exc).__name__, str(exc))  # ValueError action (nope) ...
