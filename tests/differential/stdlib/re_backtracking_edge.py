"""Purpose: CPython regex continuation, capture undo and assertion semantics.

Oracle: CPython v3.12.15 Modules/_sre/sre_lib.h, particularly SUCCESS,
MARK/LASTMARK_RESTORE, BRANCH, REPEAT, ASSERT and GROUPREF_EXISTS:
https://github.com/python/cpython/blob/v3.12.15/Modules/_sre/sre_lib.h
"""

import re


pattern = re.compile(r"(a+)+$")
print(bool(pattern.match("a" * 10)))
print(bool(pattern.match("a" * 10 + "!")))


def show(label, expression, text, operation):
    compiled = re.compile(expression)
    match = getattr(compiled, operation)(text)
    if match is None:
        print(label, None)
    else:
        # Every capture span is observed, including unset and rolled-back
        # groups. This is the same information as Match.regs.
        spans = tuple(match.span(group) for group in range(compiled.groups + 1))
        print(label, match.group(), match.groups(), spans,
              match.lastindex, match.lastgroup, match.groupdict())


cases = (
    ("partial-first-branch", r"(a|ab)", "ab", "match"),
    ("fullmatch-later-branch", r"(a|ab)", "ab", "fullmatch"),
    ("flags-backtrack", r"(?i:a*)a", "Aaa", "fullmatch"),
    ("flags-restored", r"(?i:a*)a", "AA", "fullmatch"),
    ("repeat-capture-undo", r"((a){1,3}a)", "aaa", "fullmatch"),
    ("branch-capture-undo", r"((a)|(ab))c", "abc", "fullmatch"),
    ("nested-repeat-branch", r"((a|ab)+)b", "aabb", "fullmatch"),
    ("prior-iteration-capture", r"(a(b)?)+c", "abac", "fullmatch"),
    ("lazy-repeat-backref", r"((a){1,3}?)(\2)", "aaa", "fullmatch"),
    ("empty-repeat-progress", r"(a?)*", "a", "fullmatch"),
    ("closing-order", r"(?P<outer>(?P<inner>a))", "a", "fullmatch"),
    ("assertion-capture", r"(?=(a+))a*\1", "aaaa", "fullmatch"),
    ("lookahead-atomic", r"(?=(a|ab))\1c", "abc", "fullmatch"),
    ("lookahead-priority", r"(?=(ab|a))\1c", "abc", "fullmatch"),
    ("negative-assertion-undo", r"(?:(?!(a))b|(a))", "a", "fullmatch"),
    ("conditional-after-undo", r"(?:(a)c|a)(?(1)b|d)", "ad", "fullmatch"),
    ("backref-later-branch", r"(a|ab)\1", "abab", "fullmatch"),
    ("lookbehind-capture", r"(?<=(a))b", "ab", "search"),
    ("mandatory-empty-before-optional", r"(?:(|a)){1,2}c", "ac", "fullmatch"),
    ("mandatory-empty-before-lazy", r"(?:(|a)){1,2}?c", "ac", "fullmatch"),
    ("conditional-new-start-past-stale-end", r"(?:(a(?(1)b|c))x)+", "acxacx", "match"),
    ("conditional-new-start-at-stale-end", r"(a(?(1)b|c))+", "acab", "fullmatch"),
    ("lookbehind-visible-lookahead", r"(?<=a(?=(b)))b", "ab", "search"),
    ("nested-assertion-atomic", r"(?=(a(?=(b|bc))))a\2c", "abcc", "fullmatch"),
    ("nested-assertion-priority", r"(?=(a(?=(bc|b))))a\2c", "abcc", "fullmatch"),
    ("nested-negative-undo", r"(?:(?=(a(?!(b))))ac|a(b))", "ab", "fullmatch"),
    ("nested-empty-minimum", r"((a?){2}){2}a", "a", "fullmatch"),
    ("fixed-greedy-empty-captures", r"(()(a)())+a", "aaa", "fullmatch"),
    ("fixed-lazy-empty-captures", r"(()(a)())+?a", "aaa", "fullmatch"),
    ("absolute-end-newline", r"a\Z", "a\n", "match"),
    ("class-not-digit", r"[\D]", "a", "fullmatch"),
    ("class-mixed-negation", r"[\S\d]", "a", "fullmatch"),
    ("class-mixed-negative", r"[\S\d]", "\n", "fullmatch"),
    ("class-double-negation", r"[^\D]", "7", "fullmatch"),
    ("class-nonword-union", r"[\W_]+", "!?_", "fullmatch"),
)
for kind in ("str", "bytes"):
    for label, expression, text, operation in cases:
        if kind == "bytes":
            expression = expression.encode("ascii")
            text = text.encode("ascii")
        show((kind, label), expression, text, operation)
