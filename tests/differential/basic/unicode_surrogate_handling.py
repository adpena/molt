# MOLT_META: stderr=exact
"""Purpose: differential coverage for unicode surrogate handling."""

import codecs


s = "\ud800"
try:
    s.encode("utf-8")
except Exception as exc:
    print(type(exc).__name__)

print(s.encode("utf-8", "surrogatepass"))
decoded = codecs.decode(b"\xed\xa0\x80", "utf-8", "surrogatepass")
try:
    print(decoded)
except UnicodeEncodeError:
    print("stdout_surrogate", "UnicodeEncodeError")
# Keep the decoded value observable without ending the indexing witness at an
# uncaught stdout encoding error. Matching that early exit proved no indexing.
print(repr(decoded))
print(s.encode("utf-8", "backslashreplace"))


def indexed_summary(text):
    # The differential harness compares these observable results with CPython;
    # neither byte offsets nor the compiler's storage helpers define the oracle.
    checksum = 0
    for character in text:
        checksum = (checksum * 131 + ord(character)) % 1000000007
    return (
        len(text),
        checksum,
        text[:6].encode("utf-8", "surrogatepass"),
        text[-6:].encode("utf-8", "surrogatepass"),
    )


def indexing_matrix(label, text):
    length = len(text)
    for index in (0, 1, 3, 4, -1, -length, length, -length - 1):
        try:
            character = text[index]
            # The first read exercises string results; the fused ord read has
            # its own runtime consumer and must preserve the same code point.
            print(
                label,
                "index",
                index,
                character.encode("utf-8", "surrogatepass"),
                ord(text[index]),
            )
        except Exception as exc:
            print(label, "index", index, type(exc).__name__, str(exc))

    for start, stop, step in (
        (None, None, 1),
        (None, None, 2),
        (None, None, -1),
        (None, None, -3),
        (2, -1, 3),
        (-2, None, -2),
        (3, 2, 1),
        (2, 3, -1),
        (None, None, 2**80),
        (None, None, -(2**80)),
        (10**40, -(10**40), -1),
    ):
        print(
            label,
            "slice",
            start,
            stop,
            step,
            indexed_summary(text[slice(start, stop, step)]),
        )


# Build the text at runtime. Adjacent explicit surrogates remain two Python
# code points beside the distinct supplementary scalar and an embedded NUL.
mixed = "".join(
    chr(code) for code in (65, 233, 29483, 55357, 56832, 128512, 0, 55296, 57343)
)
indexing_matrix("short", mixed)
indexing_matrix("empty", "")
# Cross both the existing 16-KiB cache threshold and 4096-byte block boundaries.
# Keeping the first non-ASCII point late also exercises the cached ASCII probe.
indexing_matrix("long", "a" * 17000 + mixed * 1800)
