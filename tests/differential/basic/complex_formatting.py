"""Purpose: differential coverage for complex formatting."""


def show(label, func):
    try:
        value = func()
        print(f"{label}: {value}")
    except Exception as exc:
        print(f"{label}: {type(exc).__name__} {exc}")


z = 1.2345 + 2.3456j
show("default", lambda: format(z, ""))
show("g", lambda: format(z, "g"))
show("f", lambda: format(z, ".2f"))
show("e", lambda: format(z, "e"))
show("E", lambda: format(z, "E"))
show("width", lambda: format(z, "20.2f"))
show("align_left", lambda: format(z, "<20.2f"))
show("sign_plus", lambda: format(z, "+.1f"))
show("sign_space", lambda: format(z, " .1f"))

z2 = 1000 + 2000j
show("grouping", lambda: format(z2, ",.1f"))
show("n_group_error", lambda: format(z2, ",n"))

z3 = 1j
show("imag_default", lambda: format(z3, ".2"))
show("imag_typed", lambda: format(z3, "g"))
show("imag_sign", lambda: format(z3, "+.2f"))

show("zero_pad_error", lambda: format(z, "0.2f"))
show("align_eq_error", lambda: format(z, "=10.2f"))
show("unknown_type", lambda: format(z, "d"))

z4 = 0 - 2.5j
show("zero_real_default", lambda: format(z4, ""))
show("zero_real_prec", lambda: format(z4, ".3f"))
show("zero_real_sign", lambda: format(z4, "+.1f"))

show("alt_g", lambda: format(z, "#.3g"))
show("alt_f", lambda: format(z, "#.2f"))
show("prec_g", lambda: format(z, ".3g"))
show("n_type", lambda: format(z, "n"))
show("zero_value", lambda: format(0j, ""))
show("percent_error", lambda: format(z, "%"))
