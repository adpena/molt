"""Differential test: re module parity with CPython.

Verifies that Molt's intrinsic-backed re module produces identical results
to CPython for match, search, fullmatch, findall, split, sub, subn, and
flag handling.
"""

import re

# ---------------------------------------------------------------------------
# 1. re.match / re.search / re.fullmatch
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 2. Groups
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 3. Named groups and groupdict
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 4. findall — with and without groups
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 5. finditer
# ---------------------------------------------------------------------------

matches = list(re.finditer(r"\d+", "a1b22c333"))
assert len(matches) == 3
assert matches[0].group() == "1"
assert matches[1].group() == "22"
assert matches[2].group() == "333"
assert matches[0].span() == (1, 2)
assert matches[1].span() == (3, 5)
assert matches[2].span() == (6, 9)

print("PASS: finditer")

# ---------------------------------------------------------------------------
# 6. re.sub — string replacement
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 7. re.sub — callable replacement
# ---------------------------------------------------------------------------

result = re.sub(r"\d+", lambda m: str(int(m.group()) * 2), "a1 b2 c3")
assert result == "a2 b4 c6", f"got {result!r}"

result = re.sub(r"(\w+)", lambda m: m.group(1).upper(), "hello world")
assert result == "HELLO WORLD", f"got {result!r}"

print("PASS: re.sub callable replacement")

# ---------------------------------------------------------------------------
# 8. re.subn
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 9. re.split — basic and with groups
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 10. Flags — IGNORECASE
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 11. Flags — MULTILINE
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 12. Flags — DOTALL
# ---------------------------------------------------------------------------

m = re.match(r".", "\n")
assert m is None

m = re.match(r".", "\n", re.DOTALL)
assert m is not None
assert m.group() == "\n"

m = re.match(r".*", "hello\nworld", re.DOTALL)
assert m is not None
assert m.group() == "hello\nworld"

print("PASS: DOTALL flag")

# ---------------------------------------------------------------------------
# 13. Flags — VERBOSE
# ---------------------------------------------------------------------------

pattern = re.compile(r"""
    (\d+)       # integer part
    \.          # decimal point
    (\d+)       # fractional part
""", re.VERBOSE)
m = pattern.match("3.14")
assert m is not None
assert m.group(1) == "3"
assert m.group(2) == "14"

print("PASS: VERBOSE flag")

# ---------------------------------------------------------------------------
# 14. Flags — ASCII
# ---------------------------------------------------------------------------

# \w with ASCII flag matches only ASCII word chars
m = re.match(r"\w+", "cafe\u0301", re.ASCII)
assert m is not None
assert m.group() == "cafe"

# \d with ASCII flag
m = re.match(r"\d+", "123", re.ASCII)
assert m is not None
assert m.group() == "123"

print("PASS: ASCII flag")

# ---------------------------------------------------------------------------
# 15. Inline flags
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 16. Anchors
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 17. Quantifiers and backtracking
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 18. Alternation
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 19. Backreferences
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 20. Lookahead and lookbehind
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 21. Character classes
# ---------------------------------------------------------------------------

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

print("PASS: character classes")

# ---------------------------------------------------------------------------
# 22. re.escape
# ---------------------------------------------------------------------------

assert re.escape("hello") == "hello"
assert re.escape("hello.world") == r"hello\.world"
assert re.escape("a+b*c?") == r"a\+b\*c\?"
assert re.escape("(test)") == r"\(test\)"

# Escaped pattern should match literally
pattern = re.escape("foo.bar+baz")
m = re.match(pattern, "foo.bar+baz")
assert m is not None

print("PASS: re.escape")

# ---------------------------------------------------------------------------
# 23. Compiled Pattern object
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 24. Match object properties
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 25. Edge cases
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 26. Error handling
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 27. Conditional patterns
# ---------------------------------------------------------------------------

m = re.match(r"(a)?(?(1)b|c)", "ab")
assert m is not None
assert m.group() == "ab"

m = re.match(r"(a)?(?(1)b|c)", "c")
assert m is not None
assert m.group() == "c"

print("PASS: conditional patterns")

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------

print("ALL TESTS PASSED")
