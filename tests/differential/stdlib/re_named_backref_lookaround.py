"""Purpose: differential coverage for re named backrefs + lookaround semantics."""

import re


def show_match(label, pattern, text):
    match = re.search(pattern, text)
    print(label, bool(match), None if match is None else match.group(0))


def show_compile_error(label, pattern, expected_prefix):
    try:
        re.compile(pattern)
        print(label, "ok-unexpected")
    except Exception as exc:
        print(
            label,
            isinstance(exc, re.error),
            str(exc).startswith(expected_prefix),
        )


show_match("named_backref_hit", r"(?P<word>ab)(?P=word)", "zzabab")
show_match("named_backref_miss", r"(?P<word>ab)(?P=word)", "zzabxy")

show_match("named_backref_lookahead_hit", r"(?P<word>ab)(?=(?P=word))", "abab")
show_match("named_backref_lookahead_miss", r"(?P<word>ab)(?=(?P=word))", "abxy")

show_match("named_backref_lookbehind_hit", r"(?P<word>ab)(?<=(?P=word))c", "abc")
show_match(
    "named_backref_lookbehind_miss",
    r"(?P<word>ab)(?<=(?P=word))c",
    "axc",
)
show_compile_error(
    "named_backref_lookbehind_nonfixed",
    r"(?P<word>a+)(?<=(?P=word))c",
    "look-behind requires fixed-width pattern",
)

show_compile_error("named_backref_unknown", r"(?P=missing)", "unknown group name")
show_compile_error(
    "named_backref_open_group",
    r"(?P<a>(?P=a)x)",
    "cannot refer to an open group",
)
show_compile_error("named_backref_missing_name", r"(?P=)", "missing group name")
