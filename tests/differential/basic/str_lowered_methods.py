"""Purpose: differential parity coverage for newly lowered str methods."""


def show(label, value):
    print(label, value)


def show_text(label, value):
    print(label, f"[{value}]")


def show_dots(label, value):
    print(label, value.replace(" ", "."))


def codepoints(text):
    return ",".join(str(ord(ch)) for ch in text) if text else "-"


def show_err(label, func):
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


show_text("zfill_empty_width0", "".zfill(0))
show_text("zfill_empty_width3", "".zfill(3))
show_text("zfill_neg_sign", "-42".zfill(5))
show_text("zfill_pos_sign", "+42".zfill(5))
show_text("zfill_noop_short_width", "1234".zfill(2))
show_text("zfill_unicode", "\u732b".zfill(3))

show_dots("expandtabs_default", "a\tb".expandtabs())
show_dots("expandtabs_tabsize4", "a\tb\tc".expandtabs(4))
show_dots("expandtabs_tabsize1", "ab\tc".expandtabs(1))
show_text("expandtabs_zero", "a\tb\t".expandtabs(0))
show_text("expandtabs_negative", "a\tb".expandtabs(-3))
show(
    "expandtabs_newline_reset",
    "|".join(part.replace(" ", ".") for part in "a\t\nb\tc".expandtabs(4).split("\n")),
)
show_dots("expandtabs_unicode", "\u732b\t\u72ac".expandtabs(4))

show_text("removeprefix_match", "foobar".removeprefix("foo"))
show_text("removeprefix_mismatch", "foobar".removeprefix("baz"))
show_text("removeprefix_empty_source", "".removeprefix("x"))
show_text("removeprefix_empty_prefix", "abc".removeprefix(""))
show_text("removeprefix_longer_prefix", "foo".removeprefix("foobar"))
show_text("removeprefix_unicode_match", "\u03c0\u03c0foo".removeprefix("\u03c0"))
show("removeprefix_unicode_mismatch_cps", codepoints("cafe\u0301".removeprefix("caf\u00e9")))

show_text("removesuffix_match", "foobar".removesuffix("bar"))
show_text("removesuffix_mismatch", "foobar".removesuffix("baz"))
show_text("removesuffix_empty_source", "".removesuffix("x"))
show_text("removesuffix_empty_suffix", "abc".removesuffix(""))
show_text("removesuffix_longer_suffix", "bar".removesuffix("foobar"))
show_text("removesuffix_unicode_match", "foo\u03c0\u03c0".removesuffix("\u03c0"))
show("removesuffix_unicode_mismatch_cps", codepoints("cafe\u0301".removesuffix("f\u00e9")))

show_text("title_basic", "hello world".title())
show_text("title_apostrophe", "they're bill's friends from the UK".title())
show_text("title_unicode", "\u01c5ungla".title())

show_text("format_map_basic", "{x}-{y}".format_map({"x": "A", "y": 2}))
show_text("format_map_index", "{x[1]}".format_map({"x": ["a", "b"]}))
show_err("format_map_missing_key", lambda: "{x}".format_map({}))
show_err("format_map_positional", lambda: "{}".format_map({"x": 1}))

show_text("percent_basic_s", "hello %s" % "world")
show_text("percent_basic_r", "repr:%r" % {"x": 1})
show_text("percent_basic_d", "%05d" % 42)
show_text("percent_basic_u", "%u" % 42)
show_text("percent_basic_o", "%o" % 9)
show_text("percent_basic_x", "%x" % 255)
show_text("percent_basic_X", "%X" % 255)
show_text("percent_basic_f", "%.2f" % 3.14159)
show_text("percent_basic_e", "%e" % 3.5)
show_text("percent_basic_E", "%E" % 3.5)
show_text("percent_basic_g", "%g" % 12345.678)
show_text("percent_basic_G", "%G" % 12345.678)
show_text("percent_basic_c_int", "%c" % 65)
show_text("percent_basic_c_str", "%c" % "A")
show_text("percent_basic_a", "%a" % "\u03c0")
show_text("percent_tuple", "%s-%d-%.1f" % ("x", 3, 2.5))
show_text("percent_mapping", "%(name)s:%(count)d" % {"name": "A", "count": 7})
show_text("percent_literal", "rate=100%%")
show_text("percent_star_width", "%*s" % (5, "x"))
show_text("percent_star_precision", "%.*s" % (3, "abcdef"))
show_text("percent_star_width_precision", "%*.*f" % (8, 3, 1.23456))
show_text("percent_star_neg_width", "%*s" % (-5, "x"))
show_text("percent_star_neg_precision", "%.*f" % (-1, 1.23))
show_err("percent_err_not_enough", lambda: "%s %s" % ("x",))
show_err("percent_err_too_many", lambda: "%s" % ("x", "y"))
show_err("percent_err_requires_mapping", lambda: "%(name)s" % ("x",))
show_err("percent_err_bad_conv", lambda: "%q" % 1)
show_err("percent_err_hex_type", lambda: "%x" % "s")
show_err("percent_err_octal_type", lambda: "%o" % 1.2)
show_err("percent_err_i_type", lambda: "%i" % "s")
show_err("percent_err_u_type", lambda: "%u" % "s")
show_err("percent_err_char_type_len", lambda: "%c" % "AB")
show_err("percent_err_char_type_bytes", lambda: "%c" % b"A")
show_err("percent_err_char_range_neg", lambda: "%c" % -1)
show_err("percent_err_char_range_big", lambda: "%c" % 1114112)
show_err("percent_err_star_width_type", lambda: "%*s" % ("5", "x"))
show_err("percent_err_star_precision_type", lambda: "%.*f" % ("2", 1.2))
show_err("percent_err_star_mapping_type", lambda: "%(x)*s" % {"x": "a"})

show_err("zfill_err_float", lambda: "42".zfill(2.5))
show_err("expandtabs_err_float", lambda: "a\tb".expandtabs(2.5))
show_err("removeprefix_err_bytes", lambda: "abc".removeprefix(b"a"))
show_err("removesuffix_err_int", lambda: "abc".removesuffix(1))
show_err("strip_err_int", lambda: "abc".strip(1))
show_err("lstrip_err_int", lambda: "abc".lstrip(1))
show_err("rstrip_err_int", lambda: "abc".rstrip(1))
