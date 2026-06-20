"""Differential checks for CPython stringprep table and mapping semantics."""

# MOLT_META: stdlib_profile=full

import stringprep


def show(label, thunk):
    try:
        value = thunk()
    except Exception as exc:
        print(label, exc.__class__.__name__)
    else:
        print(label, repr(value))


CASES = [
    ("a1_U0221", stringprep.in_table_a1("\u0221")),
    ("a1_A", stringprep.in_table_a1("A")),
    ("d1_arabic_hamza", stringprep.in_table_d1("\u0621")),
    ("d2_latin_A", stringprep.in_table_d2("A")),
    ("d2_arabic_hamza", stringprep.in_table_d2("\u0621")),
    ("b2_U03F9", stringprep.map_table_b2("\u03F9")),
    ("b3_U03F9", stringprep.map_table_b3("\u03F9")),
    ("b2_U1E9E", stringprep.map_table_b2("\u1E9E")),
    ("b3_U1E9E", stringprep.map_table_b3("\u1E9E")),
]


for name, value in CASES:
    print(name, repr(value))


ARG_SHAPE_CASES = [
    ("a1_empty", lambda: stringprep.in_table_a1("")),
    ("a1_multi", lambda: stringprep.in_table_a1("ab")),
    ("b1_empty", lambda: stringprep.in_table_b1("")),
    ("b1_multi", lambda: stringprep.in_table_b1("ab")),
    ("c11_empty", lambda: stringprep.in_table_c11("")),
    ("c11_multi", lambda: stringprep.in_table_c11("ab")),
    ("c11_c12_empty", lambda: stringprep.in_table_c11_c12("")),
    ("c11_c12_space", lambda: stringprep.in_table_c11_c12(" ")),
    ("c12_multi", lambda: stringprep.in_table_c12("ab")),
    ("c21_multi", lambda: stringprep.in_table_c21("ab")),
    ("c21_c22_multi", lambda: stringprep.in_table_c21_c22("ab")),
    ("c3_multi", lambda: stringprep.in_table_c3("ab")),
    ("c4_multi", lambda: stringprep.in_table_c4("ab")),
    ("c5_multi", lambda: stringprep.in_table_c5("ab")),
    ("c6_multi", lambda: stringprep.in_table_c6("ab")),
    ("c7_multi", lambda: stringprep.in_table_c7("ab")),
    ("c8_multi", lambda: stringprep.in_table_c8("ab")),
    ("c9_multi", lambda: stringprep.in_table_c9("ab")),
    ("d1_empty", lambda: stringprep.in_table_d1("")),
    ("d1_multi", lambda: stringprep.in_table_d1("ab")),
    ("d2_multi", lambda: stringprep.in_table_d2("ab")),
    ("b2_empty", lambda: stringprep.map_table_b2("")),
    ("b2_multi", lambda: stringprep.map_table_b2("ab")),
    ("b3_empty", lambda: stringprep.map_table_b3("")),
    ("b3_multi", lambda: stringprep.map_table_b3("ab")),
]

for name, thunk in ARG_SHAPE_CASES:
    show(name, thunk)
