import codecs


def show(label, func):
    try:
        print(label, func())
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


decode_table = "A" * 128 + "\u20ac" + "B" * 127
undefined_table = "\ufffe" * 256
xml_map = {}
for ch in "&#8364;":
    xml_map[ord(ch)] = ord(ch)

print("build_last", codecs.charmap_build("AA")[65])
print("decode_string_table", codecs.charmap_decode(b"\x80", "strict", decode_table))
print("decode_bytes_table", codecs.charmap_decode(b"A", "strict", b"B" * 256))
print("decode_backslash", codecs.charmap_decode(b"\x81", "backslashreplace", undefined_table))
print(
    "decode_surrogateescape",
    repr(codecs.charmap_decode(b"\x81", "surrogateescape", undefined_table)),
)
show("decode_bad_int", lambda: codecs.charmap_decode(b"A", "strict", {65: 0x110000}))
show("decode_bad_type", lambda: codecs.charmap_decode(b"A", "strict", {65: b"xy"}))
show("encode_str_key", lambda: codecs.charmap_encode("A", "strict", {"A": 66}))
show("encode_str_val", lambda: codecs.charmap_encode("A", "strict", {65: "x"}))
show("encode_bad_int", lambda: codecs.charmap_encode("A", "strict", {65: 300}))
print("encode_bytes_val", codecs.charmap_encode("A", "strict", {65: b"xy"}))
print("encode_replace", codecs.charmap_encode("\u20ac", "replace", {63: 63}))
print("encode_xml", codecs.charmap_encode("\u20ac", "xmlcharrefreplace", xml_map))
