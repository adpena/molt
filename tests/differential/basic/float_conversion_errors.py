"""Purpose: differential coverage for float() argument handling.

Covers valid numeric strings (including inf/nan and whitespace padding), invalid
strings (ValueError whose message embeds repr(arg)), bytes/bytearray, and
non-convertible types (TypeError with the CPython 3.12 "not '<type>'" wording).

Regression for the float-primary lane clobbering the real exception: native
codegen lowered `float(x)` whose result feeds a raw-f64 lane as
`molt_float_from_obj` (which correctly raised the ValueError) followed by
`molt_float_as_f64` on the error sentinel, and that extractor raised a generic
`TypeError: float-compatible object expected` ON TOP OF the pending ValueError.
So `float("nope")` reported the wrong type/message:
    molt:   TypeError: float-compatible object expected
    CPython ValueError: could not convert string to float: 'nope'
The fix makes scalar extractors refuse to clobber a pending exception, and
aligns the ValueError repr + the wrong-type TypeError wording with CPython 3.12.

`show` returns the result of `fn()` from a lambda so the value lands in a
float-primary lane — exactly the path that exposed the bug. All messages here
are version-stable across CPython 3.12/3.13/3.14.
"""


def show(label, fn):
    try:
        print(label, "OK", repr(fn()))
    except Exception as e:
        print(label, type(e).__name__, str(e))


# --- Valid numeric strings (molt + CPython agree; underscores excluded). ---
show("v_plain", lambda: float("3.14"))
show("v_pad", lambda: float("  1.5  "))
show("v_exp", lambda: float("1e10"))
show("v_negexp", lambda: float("-2.5e-3"))
show("v_plus", lambda: float("+5"))
show("v_negzero", lambda: float("-0.0"))
show("v_leaddot", lambda: float(".5"))
show("v_traildot", lambda: float("1."))
show("v_zero", lambda: float("0"))
show("v_tabnl", lambda: float("\t2.5\n"))

# --- inf / nan: sign, case, and whitespace padding. ---
show("s_inf", lambda: float("inf"))
show("s_neginf", lambda: float("-inf"))
show("s_plusinf", lambda: float("+inf"))
show("s_infinity", lambda: float("Infinity"))
show("s_INF", lambda: float("INF"))
show("s_nan", lambda: float("nan"))
show("s_NaN", lambda: float("NaN"))
show("s_negnan", lambda: float("-nan"))
show("s_pad_inf", lambda: float("  inf  "))
show("s_pad_nan", lambda: float("  nan  "))

# --- Invalid strings -> ValueError; message embeds repr(arg). ---
show("b_word", lambda: float("nope"))
show("b_empty", lambda: float(""))
show("b_spaces", lambda: float("   "))
show("b_double_dot", lambda: float("1.2.3"))
show("b_inf_word", lambda: float("inf nope"))
show("b_doublesign", lambda: float("++1"))
show("b_exp_only", lambda: float("1e"))
show("b_hex", lambda: float("0x10"))
show("b_squote", lambda: float("it's"))
show("b_newline", lambda: float("a\nb"))

# --- bytes / bytearray: valid parse + invalid repr (b'...' / bytearray(b'...')). ---
show("by_ok", lambda: float(b"3.14"))
show("ba_ok", lambda: float(bytearray(b"3.14")))
show("by_bad", lambda: float(b"bad"))
show("ba_bad", lambda: float(bytearray(b"bad")))

# --- Non-convertible types -> TypeError "not '<type>'" (3.12 wording). ---
show("t_list", lambda: float([]))
show("t_none", lambda: float(None))
show("t_dict", lambda: float({}))
show("t_tuple", lambda: float((1, 2)))
show("t_set", lambda: float(set()))

# --- Valid non-string args still convert. ---
show("n_int", lambda: float(7))
show("n_bool", lambda: float(True))
show("n_float", lambda: float(2.5))

# --- float() in real float arithmetic (a distinct raw-f64 lane use). ---
show("a_ok", lambda: float("2.5") + 1.0)
show("a_bad", lambda: float("nope") + 1.0)

print("done")
