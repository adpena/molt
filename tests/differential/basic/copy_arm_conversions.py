# LLVM Copy-arm lowering regression — direct exercise of the value-producing
# `Copy[_original_kind=...]` kinds the LLVM backend previously passed through as
# operand 0 (a wrong-result miscompile) instead of lowering to a fresh owned
# value via `lower_preserved_simpleir_op`.
#
# Each construct below lifts (via ssa.rs's `_ => OpCode::Copy` fallback) to a
# `Copy` carrying one of the 14 new explicitly-lowered kinds: int_from_obj /
# float_from_obj / repr_from_obj / ascii_from_obj / string_format / slice /
# slice_new / dict_keys / dict_values / dict_items / enumerate / dict_from_obj /
# complex_from_obj / object_new. On the buggy LLVM backend `int("42")` returned
# the STRING "42", `s[-5:]` returned the whole source string, etc. Must be
# byte-identical to CPython 3.14 on BOTH llvm and native targets.
#
# (The 15th classified kind, `contains`, is lowered through the established
# membership path on every backend rather than a new arm here; see the note in
# the membership section for why it is not exercised in this differential.)


# ── int(x[, base]) → int_from_obj : a fresh int, not the source string. ──
print(int("42") + 1)            # 43, not "42"
print(int("ff", 16) + 1)        # 256
print(int("  -7  ") * 2)        # -14
print(int(3.9))                 # 3 (truncation)

# ── float(x) → float_from_obj : a fresh float. ──
print(float("1.5") * 2)         # 3.0
print(float("3") + 0.25)        # 3.25
print(float("-2.5"))            # -2.5

# ── repr(x) → repr_from_obj : a fresh str, not the operand. ──
print(repr("hi"))               # 'hi'  (quoted)
print(repr([1, 2, 3]))          # [1, 2, 3]
print(repr(("a", 1)))           # ('a', 1)

# ── ascii(x) → ascii_from_obj : a fresh str. ──
print(ascii("café"))            # 'caf\xe9'
print(ascii("naive"))           # 'naive'

# ── format(...) / f-string field → string_format : a fresh str. ──
# NOTE: only FLOAT- and STR-valued format inputs are exercised here. An
# INT-valued format (`format(255, "x")`, `f"{n:03d}"`) hits a PRE-EXISTING LLVM
# miscompile — the int constant is boxed/decoded as a float, so the runtime
# raises `ValueError: Unknown format code 'x' for object of type 'float'`. That
# reproduces on clean origin/main LLVM (no relation to this Copy-arm split) and
# is reported as a separate finding; do not add int-format cases until it is
# fixed, or this differential gates on an unrelated bug.
print(format(3.14159, ".2f"))   # 3.14
print(format(1.5, "g"))         # 1.5
print(format("hi", ">5"))       # str format
print(format("ok", ""))         # str format, empty spec
print(f"{3.5:>6.1f}")           #    3.5
name = "molt"
print(f"<{name}>")              # <molt>

# ── (item in container) → contains : a fresh bool. ──
# NOT exercised here. `contains` IS classified `FreshValue` by this change's
# alias classifier, but on every backend it is lowered through the established
# membership path (LLVM `emit_containment`; native `molt_*_contains`), NOT
# through one of the new fresh-value `Copy` arms this split adds. Those existing
# paths carry PRE-EXISTING cross-target bugs that reproduce on clean
# origin/main (LLVM swaps the `Copy`-carried `contains` operands → `3 in
# [1,2,3]` raises "argument of type 'int' is not iterable"; native dict `in`
# for an absent key returns True). Both are unrelated to this Copy-arm split and
# are reported as separate findings; covering `contains` belongs with their fix.

# ── obj[start:end] → slice (the subscript) : a fresh object, not the source. ──
s = "abcdefghij"
print(s[-5:])                   # fghij
print(s[:3])                    # abc
print(s[2:5])                   # cde
print(s[::2])                   # acegi
print([10, 20, 30, 40][1:3])    # [20, 30]
print((1, 2, 3, 4, 5)[1:])      # (2, 3, 4, 5)

# ── slice(start, stop, step) → slice_new : a fresh slice object. ──
sl = slice(1, 8, 2)
print(sl.start, sl.stop, sl.step)   # 1 8 2
print("abcdefghij"[sl])             # bdfh
print([0, 1, 2, 3, 4, 5][slice(2, 5)])  # [2, 3, 4]

# ── dict.keys()/values()/items() → dict_keys / dict_values / dict_items. ──
d = {"a": 1, "b": 2, "c": 3}
print(sorted(d.keys()))         # ['a', 'b', 'c']
print(sorted(d.values()))       # [1, 2, 3]
print(sorted(d.items()))        # [('a', 1), ('b', 2), ('c', 3)]
print(len(d.keys()))            # 3
print(len(d.values()))          # 3
print(len(d.items()))           # 3
for k in sorted(d.keys()):
    print("key", k)
for v in sorted(d.values()):
    print("val", v)

# ── enumerate(iterable[, start]) → enumerate : a fresh enumerate object. ──
for i, ch in enumerate("xyz"):
    print(i, ch)
for i, ch in enumerate("xyz", start=10):
    print(i, ch)
print(list(enumerate([5, 6, 7])))   # [(0, 5), (1, 6), (2, 7)]

# ── dict(x) → dict_from_obj : a fresh dict copy. ──
src = {"p": 10, "q": 20}
copy = dict(src)
print(sorted(copy.items()))     # [('p', 10), ('q', 20)]
copy["p"] = 99                  # mutate the copy
print(src["p"])                 # 10 — original untouched (proves a fresh dict)
print(copy["p"])                # 99

# ── complex(real[, imag]) → complex_from_obj : a fresh complex. ──
c = complex("1+2j")
print(c.real, c.imag)           # 1.0 2.0
print(complex(3, 4))            # (3+4j)
print(complex(1, 2) + complex(2, 3))  # (3+5j)

# ── object() → object_new : a fresh bare object (identity-distinct). ──
o1 = object()
o2 = object()
print(o1 is o2)                 # False — two distinct fresh objects
print(o1 is o1)                 # True
o3 = o1
print(o1 is o3)                 # True
print(type(o1).__name__)        # object
