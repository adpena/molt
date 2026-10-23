"""Purpose: differential coverage for unicodedata basic."""

from collections import namedtuple
import unicodedata

# name / lookup round-trip
print("name_A", unicodedata.name("A"))
print("name_a", unicodedata.name("a"))
print("lookup_a", unicodedata.lookup("LATIN SMALL LETTER A"))
print("lookup_A", unicodedata.lookup("LATIN CAPITAL LETTER A"))

# name with default
print("name_default", unicodedata.name("\ufffe", "UNKNOWN"))

# Generic codecs share the table without requiring the public names API.
print("named_escape", b"\\N{LATIN CAPITAL LETTER A}".decode("unicode_escape"))
print("name_replace", "\u20ac\u0378".encode("ascii", "namereplace"))

# category
print("cat_A", unicodedata.category("A"))
print("cat_a", unicodedata.category("a"))
print("cat_9", unicodedata.category("9"))
print("cat_space", unicodedata.category(" "))

# bidirectional
print("bidi_A", unicodedata.bidirectional("A"))
print("bidi_9", unicodedata.bidirectional("9"))

# combining
print("combining_A", unicodedata.combining("A"))
print("combining_accent", unicodedata.combining("\u0301"))

# mirrored
print("mirrored_A", unicodedata.mirrored("A"))
print("mirrored_paren", unicodedata.mirrored("("))

# decomposition
print("decomp_A", repr(unicodedata.decomposition("A")))
print("decomp_e_acute", repr(unicodedata.decomposition("\u00e9")))

# decimal / digit / numeric
print("decimal_5", unicodedata.decimal("5"))
print("decimal_A_def", unicodedata.decimal("A", -1))
print("digit_5", unicodedata.digit("5"))
print("digit_A_def", unicodedata.digit("A", -1))
print("numeric_5", unicodedata.numeric("5"))
print("numeric_half", unicodedata.numeric("\u00bd"))

# east_asian_width
print("eaw_A", unicodedata.east_asian_width("A"))
for code in (0x231A, 0x301, 0xD800, 0xDFFF, 0xFF21, 0x1FAE9, 0x1FA8F, 0x10FFFF):
    print("eaw", code, unicodedata.east_asian_width(chr(code)))
print("ucd_version", unicodedata.unidata_version)

# normalize round-trip
cafe_nfd = "caf\u0065\u0301"
cafe_nfc = unicodedata.normalize("NFC", cafe_nfd)
print("nfc", repr(cafe_nfc))
cafe_back = unicodedata.normalize("NFD", cafe_nfc)
print("nfd_roundtrip_len", len(cafe_back))

# is_normalized
nfc_text = unicodedata.normalize("NFC", "caf\u00e9")
print("is_nfc", unicodedata.is_normalized("NFC", nfc_text))

# unidata_version exists and is a string
print("version_type", type(unicodedata.unidata_version).__name__)
print("version_nonempty", len(unicodedata.unidata_version) > 0)

# error cases
try:
    unicodedata.name("")
except TypeError:
    print("name_empty_err", "TypeError")

try:
    unicodedata.name("AB")
except TypeError:
    print("name_two_err", "TypeError")

try:
    unicodedata.lookup("NONEXISTENT CHARACTER NAME XXXX")
except KeyError:
    print("lookup_bad_err", "KeyError")

try:
    unicodedata.decimal("A")
except ValueError:
    print("decimal_A_err", "ValueError")


# Target-table deltas: Unicode 15.1's new printable symbol, Unicode 16's
# Cyrillic case pair, and Unicode 17's changed lowercase classification.
# These observations, rather than a version predicate, select the oracle.
for code in (0x2FFC, 0x1C89, 0x0295, 0x1FAE9):
    char = chr(code)
    print(
        "target_facts",
        hex(code),
        unicodedata.category(char),
        unicodedata.east_asian_width(char),
        char.isprintable(),
        char.isalpha(),
        char.islower(),
        char.isupper(),
        char.isidentifier(),
        repr(char.lower()),
        repr(char.upper()),
        repr(char.casefold()),
    )

# Context and expansions must survive a whole-string operation, including
# combining marks, punctuation, and a surrogate boundary.
for value in ("AΣ", "AΣ'A", "A\u0301Σ", "İßﬃ", "aΣ'ß", "ǅaΣ", "A\ud800ΣA"):
    print(
        "case_context",
        repr(value),
        repr(value.lower()),
        repr(value.upper()),
        repr(value.casefold()),
        repr(value.title()),
        repr(value.capitalize()),
        repr(value.swapcase()),
    )

ascii_text = " ab CD_09\t\n"
print(
    "ascii_control",
    repr(ascii_text.strip()),
    ascii_text.split(),
    repr(ascii_text.title()),
    repr(ascii_text.capitalize()),
    repr(ascii_text.swapcase()),
    "ASCII_09".isidentifier(),
    "09".isdecimal(),
)

# Unicode numeric whitespace is admitted for str, while bytes retain the
# ASCII grammar; Python's additional U+001C..1F string spaces are not numeric.
for label, padding, raw_padding in (
    ("ascii", " \t", b" \t"),
    ("nbsp", "\u00a0", b"\xc2\xa0"),
    ("ideographic", "\u3000", b"\xe3\x80\x80"),
    ("file_separator", "\x1c", b"\x1c"),
    ("unit_separator", "\x1f", b"\x1f"),
):
    for domain, value in (
        ("str", padding + "1.5" + padding),
        ("bytes", raw_padding + b"1.5" + raw_padding),
    ):
        try:
            result = float(value)
        except ValueError:
            print("numeric_whitespace", label, domain, "ValueError")
        else:
            print("numeric_whitespace", label, domain, result)
    try:
        result = complex(padding + "1+2j" + padding)
    except ValueError:
        print("complex_whitespace", label, "ValueError")
    else:
        print("complex_whitespace", label, result)

# XID includes Other_ID_Start/combining continuation and excludes a merely
# alphanumeric superscript. The Cyrillic field also changes with target data.
for field in ("a²", "℘", "a\u0301", "\ud800", "\u1c89"):
    try:
        point = namedtuple("UnicodePoint", [field])
    except ValueError:
        renamed = namedtuple("UnicodePoint", [field], rename=True)
        assert renamed._fields == ("_0",)
        print("xid", repr(field), field.isidentifier(), "ValueError")
    else:
        assert point._fields == (field,)
        assert point(7)[0] == 7
        print("xid", repr(field), field.isidentifier(), "accepted")
