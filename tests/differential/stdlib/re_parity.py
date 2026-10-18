"""CPython differential authority for the public re semantic family.

The functions group the public matching, capture, scanning, replacement,
flag/parser and buffer-custody contracts in one compiled program. Both str
and bytes paths retain their independent CPython observations. The engine
continuation oracle is CPython v3.12.15 Modules/_sre/sre_lib.h and sre.c:
https://github.com/python/cpython/blob/v3.12.15/Modules/_sre/sre_lib.h
https://github.com/python/cpython/blob/v3.12.15/Modules/_sre/sre.c

Buffer admission additionally follows CPython v3.12.13 Modules/_sre/sre.c:
https://github.com/python/cpython/blob/v3.12.13/Modules/_sre/sre.c
"""

import re
from array import array


class TrappingBytes(bytes):
    def __len__(self):
        raise AssertionError("subclass __len__ was called")

    def __getitem__(self, key):
        raise AssertionError("subclass __getitem__ was called")


class TrappingStr(str):
    def __len__(self):
        raise AssertionError("subclass __len__ was called")

    def __getitem__(self, key):
        raise AssertionError("subclass __getitem__ was called")


class SREBuffer:
    def __init__(self, failure=None, fail_on_call=None):
        self.data = bytearray(b"aa")
        self.failure = failure
        self.flags = []
        self.releases = 0
        self.fail_on_call = fail_on_call

    def __buffer__(self, flags):
        self.flags.append(flags)
        if self.failure is not None or len(self.flags) == self.fail_on_call:
            raise self.failure or TypeError("later capture exporter failure")
        return memoryview(self.data)

    def __release_buffer__(self, view):
        self.releases += 1
        view.release()


def observe_continuation(label, expression, text, operation):
    compiled = re.compile(expression)
    match = getattr(compiled, operation)(text)
    if match is None:
        print(label, None)
    else:
        # Every capture span is observed, including unset and rolled-back
        # groups. This is the same information as Match.regs.
        spans = tuple(match.span(group) for group in range(compiled.groups + 1))
        print(
            label,
            match.group(),
            match.groups(),
            spans,
            match.lastindex,
            match.lastgroup,
            match.groupdict(),
        )


def admission_result(label, operation):
    try:
        raise LookupError("outer admission handler")
    except LookupError as outer:
        try:
            operation()
        except Exception as exc:
            print(label, type(exc).__name__, str(exc), exc.__context__ is outer)


def observe_compile_flags(pattern, flags=0):
    try:
        compiled = re.compile(pattern, flags)
        print("ok", compiled.pattern, compiled.flags)
    except Exception as exc:
        print("err", type(exc).__name__)


def capture(match):
    return (
        match.span(),
        match.group(),
        match.groups(),
        match.lastindex,
        match.lastgroup,
    )


def window_capture(match):
    if match is None:
        return None
    return capture(match), match.pos, match.endpos


def observe_search(label, pattern, text):
    match = re.search(pattern, text)
    print(label, bool(match), None if match is None else match.group(0))


def observe_compile_error(label, pattern, expected_prefix):
    try:
        re.compile(pattern)
        print(label, "ok-unexpected")
    except Exception as exc:
        print(
            label,
            isinstance(exc, re.error),
            str(exc).startswith(expected_prefix),
        )


def test_internal_module_imports():
    # ensure CPython-style internal re modules are importable.
    import re._casefix  # noqa: F401
    import re._compiler  # noqa: F401
    import re._constants  # noqa: F401
    import re._parser  # noqa: F401

    print(bool(getattr(re, "compile", None)))
    print(bool(getattr(re._compiler, "_compile", None)))


def test_match_search_fullmatch():
    # Basic match
    m = re.match(r"hello", "hello world")
    assert m is not None
    assert m.group() == "hello"
    assert m.span() == (0, 5)

    # Match anchored at start — no match mid-string
    m = re.match(r"world", "hello world")
    assert m is None

    # Search finds mid-string
    m = re.search(r"world", "hello world")
    assert m is not None
    assert m.group() == "world"
    assert m.span() == (6, 11)

    # fullmatch — must match entire string
    m = re.fullmatch(r"hello", "hello")
    assert m is not None
    assert m.group() == "hello"

    m = re.fullmatch(r"hello", "hello world")
    assert m is None

    m = re.fullmatch(r".*", "anything goes")
    assert m is not None
    assert m.group() == "anything goes"

    # Empty pattern
    m = re.match(r"", "abc")
    assert m is not None
    assert m.group() == ""
    assert m.span() == (0, 0)

    m = re.fullmatch(r"", "")
    assert m is not None

    print("PASS: match/search/fullmatch basics")

    # differential coverage for re basic.
    text = "Hello 123 world"
    print(bool(re.search(r"\d+", text)))
    print(re.sub(r"\d+", "X", text))
    print(re.findall(r"[A-Za-z]+", text))
    print(re.split(r"\s+", text))

    m = re.match(r"(Hello)\s+(\d+)", text)
    print(m.group(1), m.group(2))


def test_groups():
    # Capturing groups
    m = re.match(r"(\w+)\s+(\w+)", "hello world")
    assert m is not None
    assert m.group(0) == "hello world"
    assert m.group(1) == "hello"
    assert m.group(2) == "world"
    assert m.groups() == ("hello", "world")
    assert m.start(1) == 0
    assert m.end(1) == 5
    assert m.span(2) == (6, 11)

    # Multiple return from group()
    assert m.group(1, 2) == ("hello", "world")

    # Non-capturing group
    m = re.match(r"(?:abc)+", "abcabc")
    assert m is not None
    assert m.group() == "abcabc"
    assert m.groups() == ()

    # Nested groups
    m = re.match(r"((a)(b))", "ab")
    assert m is not None
    assert m.group(1) == "ab"
    assert m.group(2) == "a"
    assert m.group(3) == "b"

    # Optional group not matched
    m = re.match(r"(a)(b)?", "a")
    assert m is not None
    assert m.group(1) == "a"
    assert m.group(2) is None
    assert m.groups() == ("a", None)
    assert m.groups("DEFAULT") == ("a", "DEFAULT")

    print("PASS: groups")

    # differential coverage for re flags groups.
    text = "Line1\nLine2\nLINE3"
    pattern = re.compile(r"^line", re.IGNORECASE | re.MULTILINE)
    print(pattern.findall(text))

    m = re.search(r"(?P<word>Line)\d", text)
    print(m.group(0), m.group("word"))

    m2 = re.search(r"(Line)(\d)\n(.*)", text, re.DOTALL)
    print(m2.group(1), m2.group(2), m2.group(3))

    print(re.sub(r"(Line)(\d)", r"\1-\2", text))


def test_named_groups():
    m = re.match(r"(?P<first>\w+)\s+(?P<last>\w+)", "John Doe")
    assert m is not None
    assert m.group("first") == "John"
    assert m.group("last") == "Doe"
    assert m.groupdict() == {"first": "John", "last": "Doe"}
    assert m["first"] == "John"

    # groupdict with default
    m = re.match(r"(?P<a>\w+)(?P<b>\d+)?", "hello")
    assert m is not None
    assert m.groupdict() == {"a": "hello", "b": None}
    assert m.groupdict("N/A") == {"a": "hello", "b": "N/A"}

    print("PASS: named groups and groupdict")

    # differential coverage for re.groupdict defaults.
    match = re.match(r"(?P<a>\d+)?-(?P<b>\w+)", "-abc")
    print("groupdict", match.groupdict())
    print("groupdict_default", match.groupdict(default="X"))


def test_findall():
    # No groups — returns list of strings
    result = re.findall(r"\d+", "abc 123 def 456 ghi")
    assert result == ["123", "456"], f"got {result}"

    # One group — returns list of group matches
    result = re.findall(r"(\d+)", "abc 123 def 456")
    assert result == ["123", "456"]

    # Multiple groups — returns list of tuples
    result = re.findall(r"(\w+)=(\w+)", "a=1 b=2 c=3")
    assert result == [("a", "1"), ("b", "2"), ("c", "3")]

    # No match
    result = re.findall(r"xyz", "abc def")
    assert result == []

    # Empty pattern
    result = re.findall(r"", "abc")
    assert result == ["", "", "", ""]

    print("PASS: findall")


def test_scanner_progress_and_windows():
    matches = list(re.finditer(r"\d+", "a1b22c333"))
    assert len(matches) == 3
    assert matches[0].group() == "1"
    assert matches[1].group() == "22"
    assert matches[2].group() == "333"
    assert matches[0].span() == (1, 2)
    assert matches[1].span() == (3, 5)
    assert matches[2].span() == (6, 9)

    print("PASS: finditer")

    # root success and scanner retry shared by every regex consumer.
    text = "abc123"
    print(bool(re.fullmatch(r"[a-z]+\d+", text)))
    print([m.group(0) for m in re.finditer(r"\d", text)])

    # The empty branch must not hide a consuming alternative at the same offset.
    # Captures distinguish a retried path from a duplicate/stale previous match.
    for kind in ("str", "bytes"):
        for expression, text in (
            (r"(?P<empty>)|(?P<run>a+)", "aa"),
            (r"a*?", "aa"),
            (r"(a|ab)", "abab"),
            (r"(?=(a))|a", "a"),
            (r"\A", "aa"),
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
        print(
            "bounded", kind, [capture(match) for match in compiled.finditer(text, 1, 3)]
        )
        print("reversed-bounds", kind, list(compiled.finditer(text, 3, 1)))

    # The offsets are clamped independently to physical length. An empty match
    # may succeed at pos > endpos, without moving pos or allowing consuming paths.
    for kind in ("str", "bytes"):
        expression = b"(|a)" if kind == "bytes" else "(|a)"
        subject = b"zaaz" if kind == "bytes" else "zaaz"
        compiled = re.compile(expression)
        for pos, endpos in (
            (-3, -1),
            (-3, 2),
            (1, 2),
            (3, 1),
            (9, 2),
            (9, 9),
            (2, -9),
            (-3, 9),
            (9, None),
        ):
            direct = []
            for mode in ("match", "fullmatch", "search"):
                operation = getattr(compiled, mode)
                found = (
                    operation(subject, pos)
                    if endpos is None
                    else operation(subject, pos, endpos)
                )
                direct.append((mode, window_capture(found)))
            iterator = (
                compiled.finditer(subject, pos)
                if endpos is None
                else compiled.finditer(subject, pos, endpos)
            )
            values = (
                compiled.findall(subject, pos)
                if endpos is None
                else compiled.findall(subject, pos, endpos)
            )
            print(
                "window",
                kind,
                pos,
                endpos,
                direct,
                [window_capture(match) for match in iterator],
                values,
            )
        required = re.compile(b"(a)" if kind == "bytes" else "(a)")
        print(
            "reversed-consuming",
            kind,
            required.match(subject, 1, 0),
            required.fullmatch(subject, 1, 0),
            required.search(subject, 1, 0),
            list(required.finditer(subject, 1, 0)),
            required.findall(subject, 1, 0),
        )


def test_string_substitution():
    # Basic replacement
    result = re.sub(r"\d+", "NUM", "abc 123 def 456")
    assert result == "abc NUM def NUM", f"got {result!r}"

    # With count
    result = re.sub(r"\d+", "NUM", "abc 123 def 456 ghi 789", count=2)
    assert result == "abc NUM def NUM ghi 789", f"got {result!r}"

    # Backreference in replacement
    result = re.sub(r"(\w+)\s+(\w+)", r"\2 \1", "hello world")
    assert result == "world hello", f"got {result!r}"

    # Named group reference in replacement
    result = re.sub(r"(?P<word>\w+)", r"[\g<word>]", "hello world")
    assert result == "[hello] [world]", f"got {result!r}"

    # \g<0> — whole match reference
    result = re.sub(r"\w+", r"(\g<0>)", "hello world")
    assert result == "(hello) (world)", f"got {result!r}"

    # No match — string unchanged
    result = re.sub(r"xyz", "ABC", "hello world")
    assert result == "hello world"

    # Empty replacement
    result = re.sub(r"\s+", "", "hello world")
    assert result == "helloworld"

    # Escape sequences in replacement
    result = re.sub(r"x", r"\n", "axb")
    assert result == "a\nb"

    print("PASS: re.sub string replacement")


def test_callable_substitution():
    result = re.sub(r"\d+", lambda m: str(int(m.group()) * 2), "a1 b2 c3")
    assert result == "a2 b4 c6", f"got {result!r}"

    result = re.sub(r"(\w+)", lambda m: m.group(1).upper(), "hello world")
    assert result == "HELLO WORLD", f"got {result!r}"

    print("PASS: re.sub callable replacement")


def test_subn():
    result, count = re.subn(r"\d+", "NUM", "abc 123 def 456")
    assert result == "abc NUM def NUM"
    assert count == 2

    result, count = re.subn(r"\d+", "NUM", "abc 123 def 456", count=1)
    assert result == "abc NUM def 456"
    assert count == 1

    result, count = re.subn(r"xyz", "ABC", "hello world")
    assert result == "hello world"
    assert count == 0

    print("PASS: re.subn")

    # differential coverage for re.subn count + flags.
    source = "AaA"
    print("subn", re.subn("a", "b", source, flags=re.I))
    print("subn_count", re.subn("a", "b", source, count=1, flags=re.I))

    # differential coverage for re subn escape.
    text = "a1b2"
    print(re.subn(r"\d", "X", text))
    print(re.escape("a.b+c?"))


def test_split():
    # Basic split
    result = re.split(r"\s+", "foo bar  baz")
    assert result == ["foo", "bar", "baz"], f"got {result}"

    # Split with maxsplit
    result = re.split(r"\s+", "foo bar baz qux", maxsplit=2)
    assert result == ["foo", "bar", "baz qux"], f"got {result}"

    # Split with capturing group — group text included in result
    result = re.split(r"(\s+)", "foo bar baz")
    assert result == ["foo", " ", "bar", " ", "baz"], f"got {result}"

    # Split with multiple groups
    result = re.split(r"(\s*)(-)\s*", "a - b - c")
    assert result == ["a", " ", "-", "b", " ", "-", "c"], f"got {result}"

    # No match — return original string in list
    result = re.split(r"x", "abc")
    assert result == ["abc"]

    # Split on every character
    result = re.split(r",", "a,b,c,d")
    assert result == ["a", "b", "c", "d"]

    # Split with pattern at start/end
    result = re.split(r",", ",a,b,")
    assert result == ["", "a", "b", ""]

    print("PASS: re.split")

    # differential coverage for re split maxsplit.
    text = "a1b2c3"
    print(re.split(r"\d", text))
    print(re.split(r"\d", text, maxsplit=2))


def test_ignorecase():
    m = re.match(r"hello", "HELLO", re.IGNORECASE)
    assert m is not None
    assert m.group() == "HELLO"

    m = re.search(r"[a-z]+", "HELLO123", re.IGNORECASE)
    assert m is not None
    assert m.group() == "HELLO"

    result = re.findall(r"[a-z]+", "Hello World 123", re.IGNORECASE)
    assert result == ["Hello", "World"]

    result = re.sub(r"hello", "GOODBYE", "Hello hello HELLO", flags=re.IGNORECASE)
    assert result == "GOODBYE GOODBYE GOODBYE", f"got {result!r}"

    print("PASS: IGNORECASE flag")


def test_multiline():
    text = "line1\nline2\nline3"

    # ^ matches start of each line with MULTILINE
    result = re.findall(r"^\w+", text, re.MULTILINE)
    assert result == ["line1", "line2", "line3"], f"got {result}"

    # Without MULTILINE, ^ only matches start of string
    result = re.findall(r"^\w+", text)
    assert result == ["line1"]

    # $ matches end of each line with MULTILINE
    result = re.findall(r"\w+$", text, re.MULTILINE)
    assert result == ["line1", "line2", "line3"], f"got {result}"

    print("PASS: MULTILINE flag")


def test_dotall():
    m = re.match(r".", "\n")
    assert m is None

    m = re.match(r".", "\n", re.DOTALL)
    assert m is not None
    assert m.group() == "\n"

    m = re.match(r".*", "hello\nworld", re.DOTALL)
    assert m is not None
    assert m.group() == "hello\nworld"

    print("PASS: DOTALL flag")


def test_verbose():
    pattern = re.compile(
        r"""
    (\d+)       # integer part
    \.          # decimal point
    (\d+)       # fractional part
""",
        re.VERBOSE,
    )
    m = pattern.match("3.14")
    assert m is not None
    assert m.group(1) == "3"
    assert m.group(2) == "14"

    print("PASS: VERBOSE flag")

    # differential coverage for re.VERBOSE / re.X flag.
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
    print("float_pattern", m.group() if m else None)
    m2 = p.match("abc")
    print("float_pattern_nomatch", m2)

    # Inline (?x) flag enables verbose mode
    p2 = re.compile(r"(?x) \d+ # digits")
    m3 = p2.match("42rest")
    print("inline_x_flag", m3.group() if m3 else None)

    # Whitespace inside character class is still literal
    p3 = re.compile(r"[ \t]+", re.VERBOSE)
    m4 = p3.match("  hello")
    print("whitespace_in_class", repr(m4.group()) if m4 else None)

    # '#' inside character class is still literal
    p4 = re.compile(r"[#a]+", re.VERBOSE)
    m5 = p4.match("#a#")
    print("hash_in_class", m5.group() if m5 else None)

    # Escaped space is a literal space in verbose mode
    p5 = re.compile(r"hello\ world", re.VERBOSE)
    m6 = p5.match("hello world")
    print("escaped_space", m6.group() if m6 else None)

    # Multiline verbose pattern with alternation
    p6 = re.compile(
        r"""
    foo   # match foo
    |
    bar   # or match bar
""",
        re.VERBOSE,
    )
    print("alt_foo", p6.match("foo").group() if p6.match("foo") else None)
    print("alt_bar", p6.match("bar").group() if p6.match("bar") else None)
    print("alt_none", p6.match("baz"))

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
    print("groups_verbose", (m7.group(1), m7.group(2)) if m7 else None)

    # re.X alias
    p8 = re.compile(r"\d+ # digits", re.X)
    m8 = p8.match("99")
    print("re_X_alias", m8.group() if m8 else None)

    # Comment at end of pattern (no trailing newline) does not cause parse error
    p9 = re.compile(r"\w+ # word chars", re.VERBOSE)
    m9 = p9.match("hello")
    print("trailing_comment_no_newline", m9.group() if m9 else None)


def test_ascii():
    # \w with ASCII flag matches only ASCII word chars
    m = re.match(r"\w+", "cafe\u0301", re.ASCII)
    assert m is not None
    assert m.group() == "cafe"

    # \d with ASCII flag
    m = re.match(r"\d+", "123", re.ASCII)
    assert m is not None
    assert m.group() == "123"

    print("PASS: ASCII flag")

    # differential coverage for re ascii locale flags.
    text = "A_"
    print(re.findall(r"\w+", text))
    print(re.findall(r"\w+", text, re.ASCII))

    try:
        re.compile(r"\w+", re.LOCALE)
        print("locale_ok")
    except Exception as exc:
        print(type(exc).__name__)


def test_inline_flags():
    m = re.match(r"(?i)hello", "HELLO")
    assert m is not None
    assert m.group() == "HELLO"

    # Scoped flags
    m = re.match(r"(?i:hello) world", "HELLO world")
    assert m is not None
    assert m.group() == "HELLO world"

    # Scoped flags don't leak
    m = re.match(r"(?i:hello) WORLD", "HELLO world")
    assert m is None

    print("PASS: inline flags")

    # differential coverage for re backrefs and inline flags.
    pattern = re.compile(r"(?i)(ab)(cd)\\1")
    print("match", bool(pattern.search("ABCDab")))

    try:
        re.compile(r"(?i:ab)(?i:cd)")
        print("inline", "ok")
    except Exception as exc:
        print("inline", type(exc).__name__)

    # differential coverage for re flags + non-greedy quantifiers.
    m = re.search("FOO", "foo", re.IGNORECASE)
    print("ignorecase", m.group(0) if m else None)

    m = re.search("a.b", "a\nb")
    print("dotall_default", m.group(0) if m else None)

    m = re.search("a.b", "a\nb", re.DOTALL)
    print("dotall", m.group(0) if m else None)

    m = re.search(r"^b", "a\nb")
    print("multiline_default", m.group(0) if m else None)

    m = re.search(r"^b", "a\nb", re.MULTILINE)
    print("multiline", m.group(0) if m else None)

    m = re.search(r"a+?", "aaaa")
    print("nongreedy", m.group(0) if m else None)


def test_anchors():
    # \A — start of string (not affected by MULTILINE)
    m = re.search(r"\Ahello", "hello\nworld", re.MULTILINE)
    assert m is not None
    m = re.search(r"\Aworld", "hello\nworld", re.MULTILINE)
    assert m is None

    # \Z — end of string
    m = re.search(r"world\Z", "hello\nworld")
    assert m is not None

    # \b — word boundary
    m = re.search(r"\bhello\b", "say hello there")
    assert m is not None
    assert m.group() == "hello"
    assert m.span() == (4, 9)

    m = re.search(r"\bhello\b", "sayhellothere")
    assert m is None

    print("PASS: anchors")


def test_continuation_and_capture_restoration():
    # Greedy
    m = re.match(r"a.*b", "aXXXb")
    assert m is not None
    assert m.group() == "aXXXb"

    # Lazy
    m = re.match(r"a.*?b", "aXbXb")
    assert m is not None
    assert m.group() == "aXb"

    # Greedy backtracking with groups
    m = re.match(r".*(\\d+)", "abc123")
    # Greedy .* eats everything, then backtracks for \d+
    m = re.match(r".*(\d+)", "abc123")
    assert m is not None
    assert m.group(1) == "3"  # greedy .* eats "abc12", \d+ gets "3"

    m = re.match(r".*?(\d+)", "abc123")
    assert m is not None
    assert m.group(1) == "123"  # lazy .*? yields "abc", \d+ gets "123"

    # Exact repeat
    m = re.match(r"a{3}", "aaaa")
    assert m is not None
    assert m.group() == "aaa"

    m = re.match(r"a{3}", "aa")
    assert m is None

    # Range repeat
    m = re.match(r"a{2,4}", "aaaaa")
    assert m is not None
    assert m.group() == "aaaa"

    print("PASS: quantifiers and backtracking")

    # CPython regex continuation, capture undo and assertion semantics.
    pattern = re.compile(r"(a+)+$")
    print(bool(pattern.match("a" * 10)))
    print(bool(pattern.match("a" * 10 + "!")))

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
        (
            "conditional-new-start-past-stale-end",
            r"(?:(a(?(1)b|c))x)+",
            "acxacx",
            "match",
        ),
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
            observe_continuation((kind, label), expression, text, operation)


def test_alternation():
    m = re.match(r"cat|dog", "dog")
    assert m is not None
    assert m.group() == "dog"

    m = re.match(r"cat|dog", "cat")
    assert m is not None
    assert m.group() == "cat"

    m = re.match(r"cat|dog", "fish")
    assert m is None

    # Alternation with groups
    m = re.match(r"(cat)|(dog)", "dog")
    assert m is not None
    assert m.group(1) is None
    assert m.group(2) == "dog"

    print("PASS: alternation")


def test_backreferences():
    m = re.match(r"(\w+) \1", "abc abc")
    assert m is not None
    assert m.group() == "abc abc"

    m = re.match(r"(\w+) \1", "abc def")
    assert m is None

    # Named backreference
    m = re.match(r"(?P<word>\w+) (?P=word)", "abc abc")
    assert m is not None
    assert m.group() == "abc abc"
    assert m.group("word") == "abc"

    print("PASS: backreferences")


def test_assertions_and_named_references():
    # Positive lookahead
    m = re.match(r"foo(?=bar)", "foobar")
    assert m is not None
    assert m.group() == "foo"  # lookahead doesn't consume

    m = re.match(r"foo(?=bar)", "foobaz")
    assert m is None

    # Negative lookahead
    m = re.match(r"foo(?!bar)", "foobaz")
    assert m is not None
    assert m.group() == "foo"

    m = re.match(r"foo(?!bar)", "foobar")
    assert m is None

    # Positive lookbehind
    m = re.search(r"(?<=foo)bar", "foobar")
    assert m is not None
    assert m.group() == "bar"

    # Negative lookbehind
    m = re.search(r"(?<!foo)bar", "bazbar")
    assert m is not None
    assert m.group() == "bar"

    m = re.search(r"(?<!foo)bar", "foobar")
    assert m is None

    print("PASS: lookahead and lookbehind")

    # differential coverage for re lookaround backref.
    text = "ababa"
    print(re.findall(r"(?=(aba))", text))
    print(re.search(r"(ab)\1", "abab").group(0))

    m = re.search(r"(?<=a)b", "ab")
    print(m.group(0))

    try:
        re.compile(r"(?<=ab)c")
        print("ok")
    except Exception as exc:
        print(type(exc).__name__)

    # differential coverage for re lookarounds and inline flags.
    print("lookahead", bool(re.search(r"a(?=b)", "ab")))
    print("lookbehind", bool(re.search(r"(?<=a)b", "ab")))

    try:
        re.compile(r"(?<=ab)c")
        print("lookbehind_fixed", "ok")
    except Exception as exc:
        print("lookbehind_fixed", type(exc).__name__)

    try:
        re.compile(r"(?i)ab(?-i:CD)")
        print("inline", "ok")
    except Exception as exc:
        print("inline", type(exc).__name__)

    # differential coverage for variable-length lookbehind errors.
    try:
        re.compile(r"(?<=a+)b")
        print("ok", "unexpected")
    except Exception as exc:
        print("err", type(exc).__name__)

    # differential coverage for re named backrefs + lookaround semantics.
    observe_search("named_backref_hit", r"(?P<word>ab)(?P=word)", "zzabab")
    observe_search("named_backref_miss", r"(?P<word>ab)(?P=word)", "zzabxy")

    observe_search("named_backref_lookahead_hit", r"(?P<word>ab)(?=(?P=word))", "abab")
    observe_search("named_backref_lookahead_miss", r"(?P<word>ab)(?=(?P=word))", "abxy")

    observe_search(
        "named_backref_lookbehind_hit", r"(?P<word>ab)(?<=(?P=word))c", "abc"
    )
    observe_search(
        "named_backref_lookbehind_miss",
        r"(?P<word>ab)(?<=(?P=word))c",
        "axc",
    )
    observe_compile_error(
        "named_backref_lookbehind_nonfixed",
        r"(?P<word>a+)(?<=(?P=word))c",
        "look-behind requires fixed-width pattern",
    )

    observe_compile_error(
        "named_backref_unknown", r"(?P=missing)", "unknown group name"
    )
    observe_compile_error(
        "named_backref_open_group",
        r"(?P<a>(?P=a)x)",
        "cannot refer to an open group",
    )
    observe_compile_error("named_backref_missing_name", r"(?P=)", "missing group name")

    # differential coverage for runtime-backed negative lookaround paths.
    observe_search("neg_la_lit_hit", r"ab(?!c)", "abd")
    observe_search("neg_la_lit_block", r"ab(?!c)", "abc")
    observe_search("neg_la_cat_hit", r"a(?!\d)", "ab")
    observe_search("neg_la_cat_block", r"a(?!\d)", "a5")

    observe_search("neg_lb_lit_hit", r"(?<!a)b", "cb")
    observe_search("neg_lb_lit_block", r"(?<!a)b", "ab")
    observe_search("neg_lb_cat_hit", r"(?<!\d)b", "xb")
    observe_search("neg_lb_cat_block", r"(?<!\d)b", "5b")

    observe_search("neg_la_complex_fallback_hit", r"a(?!b|c)", "ad")
    observe_search("neg_la_complex_fallback_block", r"a(?!b|c)", "ab")
    observe_search("neg_lb_complex_fallback_hit", r"(?<!ab|cd)e", "xye")
    observe_search("neg_lb_complex_fallback_block", r"(?<!ab|cd)e", "abe")

    try:
        re.compile(r"(?<!a+)b")
        print("neg_lb_var", "ok-unexpected")
    except Exception as exc:
        print(
            "neg_lb_var",
            type(exc).__name__,
            str(exc).startswith("look-behind requires fixed-width pattern"),
        )


def test_character_classes():
    m = re.match(r"[abc]", "b")
    assert m is not None and m.group() == "b"

    m = re.match(r"[^abc]", "d")
    assert m is not None and m.group() == "d"

    m = re.match(r"[^abc]", "a")
    assert m is None

    m = re.match(r"[a-z]", "m")
    assert m is not None

    m = re.match(r"[a-z]", "5")
    assert m is None

    # Shorthand classes
    m = re.match(r"\d", "5")
    assert m is not None
    m = re.match(r"\D", "a")
    assert m is not None
    m = re.match(r"\w", "a")
    assert m is not None
    m = re.match(r"\W", " ")
    assert m is not None
    m = re.match(r"\s", " ")
    assert m is not None
    m = re.match(r"\S", "a")
    assert m is not None

    # Escaped and literal endpoints share one range grammar. Include leading
    # ']'/'-', trailing '-', and chained ranges to detect recursive endpoint scans.
    for pattern in (
        r"[\101-\132]",
        r"[\101-Z]",
        r"[A-\132]",
        r"[\t-\r]",
        r"[\--/]",
        r"[\(-\)]",
        r"[]-a]",
        r"[--0]",
        r"[a-b-c]",
        r"[\141-\142-\143]",
        r"[a-b-c-e]",
        r"[a-]",
        r"[\d-]",
        r"[^\101-\132]",
    ):
        print(
            "class-range",
            pattern,
            repr("".join(re.findall(pattern, "".join(chr(i) for i in range(128))))),
        )

    header_pattern = re.compile(r"^(From |[\041-\071\073-\176]*:|[\t ])")
    for line in (
        "Subject: Hello\n",
        "Content-Type: text/plain\n",
        "From sender\n",
        "\tcontinued\n",
        " continued\n",
        "\n",
        "Body\n",
        "Bad Name: value\n",
    ):
        match = header_pattern.match(line)
        print("header-range", repr(line), None if match is None else match.span())

    print("PASS: character classes")

    # differential coverage for re posix class.
    text = "abc123_"  # includes underscore
    pattern = re.compile(r"[[:alnum:]]+")
    print(pattern.findall(text))

    # differential coverage for re unicode categories.
    text = "\x11\x03\na_1"
    print(re.findall(r"\w+", text))
    print(re.findall(r"\W+", text))
    print(bool(re.search(r"\d+", text)))

    # differential coverage for re unicode flags.
    text = "\u0391\u0392\u0393abc"  # Greek/Latin mix
    pattern = re.compile(r"[A-Za-z]+", re.IGNORECASE)
    print(pattern.findall(text))

    pattern_u = re.compile(r"\w+", re.UNICODE)
    print(pattern_u.findall(text))


def test_escape():
    assert re.escape("hello") == "hello"
    assert re.escape("hello.world") == r"hello\.world"
    assert re.escape("a+b*c?") == r"a\+b\*c\?"
    assert re.escape("(test)") == r"\(test\)"

    # Escaped pattern should match literally
    pattern = re.escape("foo.bar+baz")
    m = re.match(pattern, "foo.bar+baz")
    assert m is not None

    print("PASS: re.escape")


def test_pattern_objects():
    pat = re.compile(r"(\w+)\s+(\w+)")
    assert pat.pattern == r"(\w+)\s+(\w+)"
    assert pat.groups == 2

    m = pat.match("hello world")
    assert m is not None
    assert m.group(1) == "hello"

    results = pat.findall("hello world foo bar")
    assert results == [("hello", "world"), ("foo", "bar")]

    # Pattern reuse / caching
    pat1 = re.compile(r"\d+")
    pat2 = re.compile(r"\d+")
    # Both should work identically
    assert pat1.findall("a1b2") == ["1", "2"]
    assert pat2.findall("a1b2") == ["1", "2"]

    print("PASS: compiled Pattern object")


def test_match_metadata():
    m = re.search(r"(\w+)", "  hello  ")
    assert m is not None
    assert m.re is not None
    assert m.string == "  hello  "
    assert m.pos == 0
    assert m.endpos == 9
    assert m.lastindex == 1

    m = re.match(r"(?P<name>\w+)", "hello")
    assert m.lastgroup == "name"

    # __getitem__ protocol
    assert m[0] == "hello"
    assert m["name"] == "hello"

    # bool
    assert bool(m) is True

    # repr
    r = repr(m)
    assert "re.Match" in r

    print("PASS: Match object properties")

    # Match metadata observations.
    m = re.search(r"(\w+)", "  hello  ")
    assert m is not None
    print("pos", m.pos)
    print("endpos", m.endpos)
    print("lastindex", m.lastindex)

    m2 = re.match(r"(?P<name>\w+)", "hello")
    assert m2 is not None
    print("lastgroup", m2.lastgroup)


def test_empty_boundaries():
    # Empty string edge cases
    assert re.findall(r"", "") == [""]
    assert re.split(r",", "") == [""]
    assert re.sub(r"", "-", "") == "-"

    # Pattern at boundaries
    assert re.split(r",", ",") == ["", ""]
    assert re.split(r",", ",,") == ["", "", ""]

    # sub with empty match
    result = re.sub(r"x*", "-", "abc")
    assert result == "-a-b-c-", f"got {result!r}"

    print("PASS: edge cases")


def test_compile_errors():
    # Pattern compilation errors
    try:
        re.compile(r"(")
        assert False, "should have raised"
    except (re.error, ValueError):
        pass

    # Invalid flag combination
    try:
        re.compile(r"test", re.ASCII | re.UNICODE)
        assert False, "should have raised"
    except ValueError:
        pass

    # LOCALE with str pattern
    try:
        re.compile(r"test", re.LOCALE)
        assert False, "should have raised"
    except ValueError:
        pass

    print("PASS: error handling")

    # differential coverage for re compile flags errors.
    print(bool(re.compile(r"a", re.IGNORECASE | re.MULTILINE)))

    try:
        re.compile(r"a", 1 << 30)
        print("ok")
    except Exception as exc:
        print(type(exc).__name__)

    try:
        re.compile(r"(")
    except Exception as exc:
        print(type(exc).__name__)

    # differential coverage for re flags, groups, and error cases.
    observe_compile_flags(r"(?P<name>ab)c")
    observe_compile_flags(r"[a-z]+", re.IGNORECASE | re.MULTILINE)
    observe_compile_flags(r"(?s)dot.*")
    observe_compile_flags(r"(?x)a # comment\n b")
    observe_compile_flags(r"(")


def test_conditionals():
    m = re.match(r"(a)?(?(1)b|c)", "ab")
    assert m is not None
    assert m.group() == "ab"

    m = re.match(r"(a)?(?(1)b|c)", "c")
    assert m is not None
    assert m.group() == "c"

    print("PASS: conditional patterns")

    # differential coverage for re conditional groups.
    pattern = re.compile(r"(a)?b(?(1)c|d)")
    print("match1", bool(pattern.fullmatch("abc")))
    print("match2", bool(pattern.fullmatch("bd")))


def test_subject_admission_and_export_custody():
    # bytes-pattern admission through the public re subject family.
    pattern = re.compile(b"(?P<word>\\w+)")
    print(pattern.search(b"\xffA9!").group(0))
    print(pattern.findall(memoryview(b"A1,FF")))
    print(re.search(b"[^0-9A-F]", array("B", b"AF1!")).group())

    for byte_pattern in (b"\\u0041", b"\\U00000041", b"\\N{LATIN CAPITAL LETTER A}"):
        try:
            re.compile(byte_pattern)
        except Exception as exc:
            print(type(exc).__name__, str(exc))
    print(re.search(b"[A-Z]", b"\xe4", re.IGNORECASE))
    print(re.sub(b"(a)", b"\\141\\1", b"a"))
    print(re.sub(b"(a)", b"\\n", b"a"))
    for replacement in (b"\\z", b"\\g<missing>", b"\\9"):
        try:
            re.sub(b"(a)", replacement, b"a")
        except Exception as exc:
            print(type(exc).__name__, str(exc))

    subject = bytearray(b"xA1!")
    match = re.search(b"(?P<hex>[0-9A-F]+)", subject)
    print(match.string is subject, match.group("hex"))
    subject[1:3] = b"B2"
    print(match.group(0), match.groups(), match.groupdict())

    # Saved spans remain valid after the mutable subject shrinks. Capture reads
    # clamp both endpoints to the current storage, including replacement expansion.
    shrinking = bytearray(b"ab")
    shrunk_match = re.search(b"(?P<first>a)(?P<second>b)", shrinking)
    shrinking.clear()
    print(
        "cleared captures",
        shrunk_match.span(),
        shrunk_match.group(),
        shrunk_match.groups(),
        shrunk_match.groupdict(),
        shrunk_match.expand(b"<\\1|\\2>"),
    )
    shrinking.extend(b"x")
    print(
        "short captures",
        shrunk_match.group(),
        shrunk_match.groups(),
        shrunk_match.groupdict(),
        shrunk_match.expand(b"<\\1|\\2>"),
    )

    surrogate_match = re.search("(?P<surrogate>.)", "\ud800A")
    print(
        "surrogate capture",
        repr(surrogate_match.group()),
        repr(surrogate_match.groups()),
        repr(surrogate_match.groupdict()),
        repr(surrogate_match.expand("<\\g<surrogate>>")),
    )

    for label, value in (
        ("high", "\ud800"),
        ("low", "\udfff"),
        ("adjacent-pair", "\ud83d\ude00"),
        ("scalar", "\U0001f600"),
    ):
        match = re.fullmatch("(?P<value>.+)", value)
        print("expand codepoints", label, repr(match.expand("<\\g<value>>")))
        print("expand literal", label, repr(match.expand("\ud800\\1")))

    print("sub literal surrogate", repr(re.sub("a", "\ud800", "a")))
    print(
        "sub captured group with surrogate literal",
        repr(re.sub("(a)", "<\\1>\ud800", "a")),
    )
    unmatched_text = re.match("(?P<missing>x)?a", "a")
    print("expand unmatched text", repr(unmatched_text.expand("[\\g<missing>|\\1]")))
    try:
        re.match("a", "a").expand("\\g<unknown\ud800>")
    except Exception as exc:
        print("malformed surrogate group name", type(exc).__name__, str(exc))

    print(re.search(b"a", TrappingBytes(b"ba")).span())
    print(re.search("a", TrappingStr("ba")).span())
    text_match = re.search("(?P<letter>a)", TrappingStr("ba"))
    print(
        text_match.group(),
        text_match.groups(),
        text_match.groupdict(),
        text_match.expand("[\\g<letter>]"),
    )
    print([match.span() for match in re.finditer(b"|a", b"a")])
    print([match.span() for match in re.finditer("|a", "a")])
    print([match.span() for match in re.finditer(b"", b"a")])

    live_subject = bytearray(b"ab")
    live_matches = re.finditer(b"a", live_subject)
    print(next(live_matches).span())
    live_subject[1] = ord("a")
    print(next(live_matches).span())
    try:
        live_subject.append(0)
    except Exception as exc:
        print(type(exc).__name__, str(exc))

    for operation in (
        lambda: re.search(b"a", "a"),
        lambda: re.search("a", b"a"),
        lambda: re.compile(b"a", re.UNICODE),
    ):
        try:
            operation()
        except Exception as exc:
            print(type(exc).__name__, str(exc))

    for value in ("a b", b"a b", bytearray(b"a b"), "a\x00b", b"a\x00b"):
        print("escape", re.escape(value))
    for expression, value in (("(a)?b", "b"), (b"(a)?b", b"b")):
        print("unmatched findall", re.findall(expression, value))
    for limit in (-1, 0, 1):
        for expression, replacement, value in (("a", "X", "aa"), (b"a", b"X", b"aa")):
            print("limits", limit, re.split(expression, value, maxsplit=limit))
            print(re.subn(expression, replacement, value, count=limit))
            print(re.subn(expression, lambda match: replacement, value, count=limit))

    # All SRE subjects enter the same PyBUF_SIMPLE role, including text-pattern
    # mismatches. Exporter errors are replaced; the active handled error remains
    # the resulting TypeError's context rather than the discarded exporter error.

    for expression in (b"a", "a"):
        compiled = re.compile(expression)
        for value in (None, 7, []):
            for name in ("search", "finditer", "split", "sub"):
                if name == "sub":
                    replacement = b"X" if isinstance(expression, bytes) else "X"
                    operation = lambda: compiled.sub(replacement, value)
                else:
                    operation = lambda: getattr(compiled, name)(value)
                admission_result(
                    ("nonbuffer", expression, name, type(value).__name__), operation
                )

    for failure_type in (TypeError, ValueError, BufferError, MemoryError):
        for expression in (b"a", "a"):
            compiled = re.compile(expression)
            for name in ("search", "finditer"):
                source = SREBuffer(failure_type("exporter failure"))
                admission_result(
                    ("exporter", failure_type.__name__, expression, name),
                    lambda: getattr(compiled, name)(source),
                )
                print("failed export custody", source.flags, source.releases)
        source = SREBuffer()
        match = re.search(b"a", source)
        source.failure = failure_type("capture exporter failure")
        admission_result(("capture", failure_type.__name__), lambda: match.group())
        print("capture export custody", source.flags, source.releases)

    source = SREBuffer()
    match = re.search(b"(?P<first>a)(?P<second>a)", source)
    source.fail_on_call = len(source.flags) + 2
    admission_result("later capture exporter", lambda: match.groups())
    print("later capture export custody", source.flags, source.releases)

    for expression in (b"a", "a"):
        for name in ("search", "finditer"):
            strided = memoryview(b"axax")[::2]
            admission_result(
                ("strided", expression, name),
                lambda: getattr(re.compile(expression), name)(strided),
            )
            strided.release()

    source = SREBuffer()
    iterator = re.finditer(b"a", source)
    print("scanner admission flags", source.flags, source.releases)
    try:
        source.data.append(0)
    except BufferError:
        print("scanner holds export", True)
    print("scanner spans", [match.span() for match in iterator])
    print("scanner exhaustion custody", source.flags, source.releases)
    source.data.append(0)
    print("exhausted iterator stays live without export", bytes(source.data))


def main():
    test_internal_module_imports()
    test_match_search_fullmatch()
    test_groups()
    test_named_groups()
    test_findall()
    test_scanner_progress_and_windows()
    test_string_substitution()
    test_callable_substitution()
    test_subn()
    test_split()
    test_ignorecase()
    test_multiline()
    test_dotall()
    test_verbose()
    test_ascii()
    test_inline_flags()
    test_anchors()
    test_continuation_and_capture_restoration()
    test_alternation()
    test_backreferences()
    test_assertions_and_named_references()
    test_character_classes()
    test_escape()
    test_pattern_objects()
    test_match_metadata()
    test_empty_boundaries()
    test_compile_errors()
    test_conditionals()
    test_subject_admission_and_export_custody()
    print("ALL TESTS PASSED")


if __name__ == "__main__":
    main()
