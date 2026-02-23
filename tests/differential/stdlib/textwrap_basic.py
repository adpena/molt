"""Purpose: differential coverage for textwrap intrinsic-backed behavior."""

from __future__ import annotations

import textwrap

wrapper = textwrap.TextWrapper(width=5)
wrapped = wrapper.wrap("hello world from molt")
filled = wrapper.fill("hello world from molt")
print("width_wrap", wrapped)
print("width_fill", filled.replace("\n", "|"))
assert wrapped == ["hello", "world", "from", "molt"]
assert filled == "hello\nworld\nfrom\nmolt"
assert "\n".join(wrapped) == filled

indented_default = textwrap.indent("line1\n\nline2", "> ")
indented_all = textwrap.indent("line1\n\nline2", "> ", lambda _: True)
assert indented_default == "> line1\n\n> line2"
assert indented_all == "> line1\n> \n> line2"
print("indent_contract", "ok")

drop_true = textwrap.TextWrapper(width=6, drop_whitespace=True).wrap("a   b   c")
drop_false = textwrap.TextWrapper(width=6, drop_whitespace=False).wrap("a   b   c")
assert drop_true == ["a   b", "c"]
assert drop_false == ["a   b", "   c"]
print("drop_whitespace_contract", "ok")

expand_true = textwrap.TextWrapper(
    width=4,
    expand_tabs=True,
    replace_whitespace=False,
).wrap("a\tb")
expand_false = textwrap.TextWrapper(
    width=4,
    expand_tabs=False,
    replace_whitespace=False,
).wrap("a\tb")
assert expand_true == ["a", "b"]
assert expand_false == ["a\tb"]
print("expand_tabs_contract", "ok")

break_true = textwrap.TextWrapper(
    width=8,
    break_on_hyphens=True,
    break_long_words=False,
).wrap("alpha-beta-gamma")
break_false = textwrap.TextWrapper(
    width=8,
    break_on_hyphens=False,
    break_long_words=False,
).wrap("alpha-beta-gamma")
assert break_true == ["alpha-", "beta-", "gamma"]
assert break_false == ["alpha-beta-gamma"]
print("break_on_hyphens_contract", "ok")

limited_wrap = textwrap.TextWrapper(width=10, max_lines=2, placeholder=" [...]").wrap(
    "alpha beta gamma delta"
)
limited_fill = textwrap.TextWrapper(width=10, max_lines=2, placeholder=" [...]").fill(
    "alpha beta gamma delta"
)
assert limited_wrap == ["alpha beta", "[...]"]
assert limited_fill == "alpha beta\n[...]"
print("max_lines_contract", "ok")
