"""Purpose: root success and scanner retry shared by every regex consumer.

Oracle: CPython v3.12.15 Modules/_sre/sre_lib.h SUCCESS checks match_all and
must_advance; Modules/_sre/sre.c scanner, findall, split and pattern_subx
retain the empty-match retry and its actual captures:
https://github.com/python/cpython/blob/v3.12.15/Modules/_sre/sre.c
"""

import re


text = "abc123"
print(bool(re.fullmatch(r"[a-z]+\d+", text)))
print([m.group(0) for m in re.finditer(r"\d", text)])


def capture(match):
    return (match.span(), match.group(), match.groups(),
            match.lastindex, match.lastgroup)


# The empty branch must not hide a consuming alternative at the same offset.
# Captures distinguish a retried path from a duplicate/stale previous match.
for kind in ("str", "bytes"):
    for expression, text in (
        (r"(?P<empty>)|(?P<run>a+)", "aa"),
        (r"a*?", "aa"),
        (r"(a|ab)", "abab"),
        (r"(?=(a))|a", "a"),
    ):
        replacement = r"[\g<1>]" if re.compile(expression).groups else "#"
        marker = "#"
        if kind == "bytes":
            expression = expression.encode("ascii")
            text = text.encode("ascii")
            replacement = replacement.encode("ascii")
            marker = marker.encode("ascii")
        compiled = re.compile(expression)
        label = (kind, expression)
        iterator = compiled.finditer(text)
        print("finditer", label, [capture(match) for match in iterator])
        print("exhausted", label, list(iterator))
        print("findall", label, compiled.findall(text))
        print("split", label, compiled.split(text))
        print("subn", label, compiled.subn(replacement, text))

        calls = []

        def replace(match):
            before = capture(match)
            nested = compiled.search(text)
            calls.append((before, nested.span(), capture(match) == before))
            return marker

        print("callback", label, compiled.sub(replace, text, count=2), calls)

    expression = b"(|a)" if kind == "bytes" else r"(|a)"
    text = b"zaaz" if kind == "bytes" else "zaaz"
    compiled = re.compile(expression)
    print("bounded", kind, [capture(match) for match in compiled.finditer(text, 1, 3)])
    print("reversed-bounds", kind, list(compiled.finditer(text, 3, 1)))
