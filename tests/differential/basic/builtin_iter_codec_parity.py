"""Purpose: CPython parity for builtin iteration + codec-arg error messages.
next(non-iterator) is type-qualified (and rejects before a default); enumerate's
non-int start uses the integer-interpretation message; zip(strict) names the
contiguous run of preceding arguments; bytes.decode/str.encode and codecs.* drop
the spurious quotes around the offending type name. All version-stable.

NOTE: the enumerate(...) and zip(strict=True) cases are exercised at module scope
rather than inside a lambda.  An eager iterator consumer (`list(...)`) compiled
WITHOUT the function exception stack hangs when the producer raises (zip mid
-iteration, or enumerate at construction) -- a separate, pre-existing frontend
bug tracked in the iter-consume-hang baton.  The error *messages* under test here
are produced by the iterator/constructor and are identical regardless of the
consuming context, so module scope (which carries the exception stack) validates
the message fixes without tripping that orthogonal hang.
"""


def show(label, fn):
    try:
        r = fn()
        print(label, "OK", repr(r))
    except Exception as e:
        print(label, type(e).__name__, str(e))


# next(non-iterator) — type-qualified, rejects even with a default (no iteration)
show("next_int", lambda: next(5))
show("next_list", lambda: next([1, 2]))
show("next_int_default", lambda: next(5, 99))

# enumerate(start=non-int) — module scope; see the module docstring.
try:
    list(enumerate([1, 2], start="x"))
except Exception as e:
    print("enum_str", type(e).__name__, str(e))
try:
    list(enumerate([1, 2], 2.5))
except Exception as e:
    print("enum_float", type(e).__name__, str(e))

# zip(strict=True) length mismatch — preceding-arg phrasing (module scope).
try:
    list(zip([1, 2], [1, 2], [1], strict=True))
except Exception as e:
    print("zip3_short", type(e).__name__, str(e))
try:
    list(zip([1], [1], [1, 2], strict=True))
except Exception as e:
    print("zip3_long", type(e).__name__, str(e))
try:
    list(zip([1, 2], [1], strict=True))
except Exception as e:
    print("zip2_short", type(e).__name__, str(e))
try:
    list(zip([1, 2], [1, 2], [1, 2], [1], strict=True))
except Exception as e:
    print("zip4_short", type(e).__name__, str(e))

# bytes.decode / str.encode / codecs.* codec-arg type error (no quotes; no iteration)
show("decode_int", lambda: b"abc".decode(123))
show("decode_bytes", lambda: b"abc".decode(b"utf-8"))
show("decode_errors_int", lambda: b"abc".decode("utf-8", 123))
show("encode_int", lambda: "abc".encode(123))
import codecs

show("codecs_decode_int", lambda: codecs.decode(b"abc", 123))
show("codecs_encode_int", lambda: codecs.encode("abc", 123))
