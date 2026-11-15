"""Purpose: NaN value semantics stay exact while float identity is the implementation's.

The verified subset contract ("Identity of immutable numbers") leaves `is` on
floats to the implementation, and Molt keeps NaNs in the value word, so equal
NaNs are identical there. This test pins only what both implementations owe:
NaN compares unequal to everything, itself included, and a container finds the
very NaN object the program stored.
"""

nan = float("nan")
other = float("nan")
print(nan == nan, nan != nan, nan < nan, nan == other, nan != other)
print(nan in [nan], [nan].count(nan), [nan].index(nan), (nan,) == (nan,))
print({nan: "stored"}[nan], nan in {nan}, nan in {nan: 1})

values = [1.0, nan, 2.0]
values.remove(nan)
print(len(values), values)

inf = float("inf")
computed = inf - inf
print(computed != computed, computed == computed, computed > 0, computed < 0)
print(max(1.0, nan) != max(1.0, nan), sorted([3.0, 1.0, 2.0]))
print([x for x in (nan, 1.0, nan) if x == x])
