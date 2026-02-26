"""Purpose: differential coverage for runtime-backed negative lookaround paths."""

import re


def show(label, pattern, text):
    match = re.search(pattern, text)
    print(label, bool(match), None if match is None else match.group(0))


show("neg_la_lit_hit", r"ab(?!c)", "abd")
show("neg_la_lit_block", r"ab(?!c)", "abc")
show("neg_la_cat_hit", r"a(?!\d)", "ab")
show("neg_la_cat_block", r"a(?!\d)", "a5")

show("neg_lb_lit_hit", r"(?<!a)b", "cb")
show("neg_lb_lit_block", r"(?<!a)b", "ab")
show("neg_lb_cat_hit", r"(?<!\d)b", "xb")
show("neg_lb_cat_block", r"(?<!\d)b", "5b")

show("neg_la_complex_fallback_hit", r"a(?!b|c)", "ad")
show("neg_la_complex_fallback_block", r"a(?!b|c)", "ab")
show("neg_lb_complex_fallback_hit", r"(?<!ab|cd)e", "xye")
show("neg_lb_complex_fallback_block", r"(?<!ab|cd)e", "abe")

try:
    re.compile(r"(?<!a+)b")
    print("neg_lb_var", "ok-unexpected")
except Exception as exc:
    print(
        "neg_lb_var",
        type(exc).__name__,
        str(exc).startswith("look-behind requires fixed-width pattern"),
    )
