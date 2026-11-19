"""Purpose: a native method descriptor binds the keywords its signature names.

Each call goes through getattr, so the receiver's type is not known statically
and the call takes the runtime's generic binder.
"""


def call(label, f):
    try:
        print(label, repr(f()))
    except Exception as e:
        print(label, type(e).__name__, e)


s = "a b c"
b = b"a\tb"
call("str.split", lambda: getattr(s, "split")(sep=" ", maxsplit=1))
call("str.rsplit", lambda: getattr(s, "rsplit")(maxsplit=1))
call("str.splitlines", lambda: getattr("a\nb", "splitlines")(keepends=True))
call("str.expandtabs", lambda: getattr("a\tb", "expandtabs")(tabsize=2))
call("str.encode", lambda: getattr("a", "encode")(encoding="ascii", errors="strict"))
call("bytes.translate", lambda: getattr(b, "translate")(None, delete=b"a"))
call("bytes.expandtabs", lambda: getattr(b, "expandtabs")(tabsize=2))
call("bytes.split", lambda: getattr(b"a b", "split")(maxsplit=0))
call("bytes.decode", lambda: getattr(b"a", "decode")(errors="strict"))
call("bytes.hex", lambda: getattr(b"ab", "hex")(sep=":"))
call("bytearray.translate", lambda: getattr(bytearray(b"ab"), "translate")(None, delete=b"a"))
call("bytearray.splitlines", lambda: getattr(bytearray(b"a\nb"), "splitlines")(keepends=True))
call("int.to_bytes", lambda: getattr(5, "to_bytes")(length=2, byteorder="little"))
call("list.sort", lambda: getattr([3, 1], "sort")(reverse=True))
call("str.translate-keyword", lambda: getattr("ab", "translate")(table={}))
call("bytes.translate-table-keyword", lambda: getattr(b"ab", "translate")(table=None))
call("str.split-unknown", lambda: getattr(s, "split")(bad=1))
call("str.split-duplicate", lambda: getattr(s, "split")(" ", sep=" "))
call("str.split-extra", lambda: getattr(s, "split")(" ", 1, 2))
call("str.split-keyword-only", lambda: getattr(s, "split")(maxsplit=1))
call("bytes.translate-missing", lambda: getattr(b"ab", "translate")())
call("bytes.translate-extra", lambda: getattr(b"ab", "translate")(None, b"", b""))
call("bytes.hex-bytes-per-sep", lambda: getattr(b"abcd", "hex")(sep="-", bytes_per_sep=2))
call("memoryview.hex", lambda: getattr(memoryview(b"ab"), "hex")(sep=":"))
call("bytearray.decode", lambda: getattr(bytearray(b"a"), "decode")(encoding="ascii"))
call("str.encode-unknown", lambda: getattr("a", "encode")(codec="ascii"))
call("str.splitlines-unknown", lambda: getattr("a", "splitlines")(keep=True))
call("str.expandtabs-extra", lambda: getattr("a", "expandtabs")(1, 2))
call("str.zfill-keyword", lambda: getattr("a", "zfill")(width=3))
call("list.append-keyword", lambda: getattr([], "append")(object=1))
call("explicit-descriptor", lambda: str.split("a b", sep=" "))
call("explicit-translate", lambda: bytes.translate(b"ab", None, delete=b"b"))
call("bound-subclass", lambda: getattr(type("S", (str,), {})("a\tb"), "expandtabs")(tabsize=1))
