"""Purpose: differential coverage for newly lowered str method edge semantics."""


def visible(text):
    mapped = (
        text.replace("\r", "<CR>")
        .replace("\n", "<NL>")
        .replace("\t", "<TAB>")
        .replace(" ", "<SP>")
    )
    return mapped if mapped else "<EMPTY>"


def show_error(label, func):
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__)


for value, width in (
    ("", 0),
    ("", 3),
    ("42", -2),
    ("42", 1),
    ("42", 5),
    ("-42", 5),
    ("+42", 5),
    ("-0", 4),
    ("+0", 4),
    ("\u00df", 3),
    ("-\u00df", 4),
):
    try:
        out = value.zfill(width)
        print("zfill", visible(value), width, visible(out), len(out))
    except Exception as exc:
        print("zfill", visible(value), width, "ERR", type(exc).__name__)


for value, tabsize in (
    ("", 4),
    ("\t", 8),
    ("ab\tc", 4),
    ("abcd\te", 4),
    ("a\tb\t", 3),
    ("\ta", 2),
    ("a\n\tb", 4),
    ("ab\r\tc", 4),
    ("x\ty", 0),
    ("x\ty", -3),
):
    try:
        out = value.expandtabs(tabsize)
        print("expandtabs", visible(value), tabsize, visible(out), len(out))
    except Exception as exc:
        print("expandtabs", visible(value), tabsize, "ERR", type(exc).__name__)


for value, prefix in (
    ("", "x"),
    ("abc", ""),
    ("abc", "ab"),
    ("abc", "abc"),
    ("abc", "abcd"),
    ("abc", "bc"),
    ("\u00fcber", "\u00fc"),
    ("\u00fcber", "u"),
    ("aaaa", "aa"),
):
    try:
        out = value.removeprefix(prefix)
        print("removeprefix", visible(value), visible(prefix), visible(out))
    except Exception as exc:
        print("removeprefix", visible(value), visible(prefix), "ERR", type(exc).__name__)


for value, suffix in (
    ("", "x"),
    ("abc", ""),
    ("abc", "bc"),
    ("abc", "abc"),
    ("abc", "abcd"),
    ("abc", "ab"),
    ("caf\u00e9", "f\u00e9"),
    ("caf\u00e9", "fe"),
    ("aaaa", "aa"),
):
    try:
        out = value.removesuffix(suffix)
        print("removesuffix", visible(value), visible(suffix), visible(out))
    except Exception as exc:
        print("removesuffix", visible(value), visible(suffix), "ERR", type(exc).__name__)


show_error("zfill_type_error", lambda: "abc".zfill("3"))
show_error("expandtabs_type_error", lambda: "abc".expandtabs("4"))
show_error("removeprefix_type_error", lambda: "abc".removeprefix(1))
show_error("removesuffix_type_error", lambda: "abc".removesuffix(1))
