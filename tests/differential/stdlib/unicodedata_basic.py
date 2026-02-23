"""Purpose: differential coverage for unicodedata basic."""

import unicodedata

# name / lookup round-trip
print("name_A", unicodedata.name("A"))
print("name_a", unicodedata.name("a"))
print("lookup_a", unicodedata.lookup("LATIN SMALL LETTER A"))
print("lookup_A", unicodedata.lookup("LATIN CAPITAL LETTER A"))

# name with default
print("name_default", unicodedata.name("\ufffe", "UNKNOWN"))

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
