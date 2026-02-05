"""Purpose: differential coverage for bytes/bytearray translate + maketrans."""


def show_exc(label, fn, *args):
    try:
        fn(*args)
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, "no error")


print("bytes.maketrans is bytes().maketrans", bytes.maketrans is b"".maketrans)
print(
    "bytearray.maketrans is bytearray().maketrans",
    bytearray.maketrans is bytearray().maketrans,
)

trans = bytes.maketrans(b"abc", b"xyz")
print("trans_len", len(trans))
print(b"abc".translate(trans))
print(b"abc".translate(trans, b"b"))
print(b"abc".translate(None))
print(b"abc".translate(None, b"b"))
print(bytearray(b"abc").translate(trans))

show_exc("maketrans_len", bytes.maketrans, b"ab", b"c")
show_exc("maketrans_type", bytes.maketrans, "ab", b"cd")
show_exc("maketrans_args", bytes.maketrans, b"ab")

show_exc("translate_table_len", b"abc".translate, b"abc")
show_exc("translate_delete_type", b"abc".translate, trans, "a")
