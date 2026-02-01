"""Purpose: differential coverage for complex formatting.

Behavior: complex __format__ matches CPython for default/precision formats.
Why: formatted complex numbers surface in f-strings and logging output.
Pitfalls: formatting drops parentheses when a format spec is provided.
"""

value = 1 + 2j
value_neg = 1 - 2j

print(str(0 + 0j))
print(str(value))
print(format(value))
print(format(value, ".2f"))
print(f"{value:.2f}")
print(format(value_neg, ".1f"))
print(f"{value_neg:.1f}")
