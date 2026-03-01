"""Purpose: differential coverage for re.VERBOSE / re.X flag."""

import re


def show(label, value):
    print(label, value)


# Basic: whitespace is ignored in verbose mode
p = re.compile(
    r"""
    \d+   # one or more digits
    \.    # literal dot
    \d*   # zero or more digits after dot
""",
    re.VERBOSE,
)
m = p.match("3.14")
show("float_pattern", m.group() if m else None)
m2 = p.match("abc")
show("float_pattern_nomatch", m2)

# Inline (?x) flag enables verbose mode
p2 = re.compile(r"(?x) \d+ # digits")
m3 = p2.match("42rest")
show("inline_x_flag", m3.group() if m3 else None)

# Whitespace inside character class is still literal
p3 = re.compile(r"[ \t]+", re.VERBOSE)
m4 = p3.match("  hello")
show("whitespace_in_class", repr(m4.group()) if m4 else None)

# '#' inside character class is still literal
p4 = re.compile(r"[#a]+", re.VERBOSE)
m5 = p4.match("#a#")
show("hash_in_class", m5.group() if m5 else None)

# Escaped space is a literal space in verbose mode
p5 = re.compile(r"hello\ world", re.VERBOSE)
m6 = p5.match("hello world")
show("escaped_space", m6.group() if m6 else None)

# Multiline verbose pattern with alternation
p6 = re.compile(
    r"""
    foo   # match foo
    |
    bar   # or match bar
""",
    re.VERBOSE,
)
show("alt_foo", p6.match("foo").group() if p6.match("foo") else None)
show("alt_bar", p6.match("bar").group() if p6.match("bar") else None)
show("alt_none", p6.match("baz"))

# Groups work correctly in verbose mode
p7 = re.compile(
    r"""
    (\d+)   # capture digits
    \s*     # optional whitespace
    ([a-z]+)# capture letters
""",
    re.VERBOSE,
)
m7 = p7.match("42 abc")
show("groups_verbose", (m7.group(1), m7.group(2)) if m7 else None)

# re.X alias
p8 = re.compile(r"\d+ # digits", re.X)
m8 = p8.match("99")
show("re_X_alias", m8.group() if m8 else None)

# Comment at end of pattern (no trailing newline) does not cause parse error
p9 = re.compile(r"\w+ # word chars", re.VERBOSE)
m9 = p9.match("hello")
show("trailing_comment_no_newline", m9.group() if m9 else None)
