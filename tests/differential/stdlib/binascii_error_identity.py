"""Purpose: every binascii and base64 decode failure raises the one binascii.Error.

``binascii.Error`` subclasses ``ValueError`` and ``binascii.Incomplete`` does
not. Each failure must be that class, caught by ``except ValueError``, with
CPython's message: strict and lenient ``a2b_base64``, the base64 wrappers,
base32, base16, hex and uu decoding.
"""

import base64
import binascii


def check(label, fn):
    try:
        print(label, "ok", fn())
    except ValueError as exc:
        print(
            label,
            type(exc).__module__,
            type(exc).__qualname__,
            type(exc) is binascii.Error,
            exc,
        )


print([cls.__qualname__ for cls in binascii.Error.__mro__])
print([cls.__qualname__ for cls in binascii.Incomplete.__mro__])
print(binascii.Error.__module__, binascii.Incomplete.__module__)

cases = [
    ("validate-junk", lambda: base64.b64decode("@@@@notbase64@@@@", validate=True)),
    ("validate-space", lambda: base64.b64decode("YWJj ZGVm", validate=True)),
    ("validate-leading-pad", lambda: base64.b64decode("=YWJj", validate=True)),
    ("validate-excess-pad", lambda: base64.b64decode("YWJj=", validate=True)),
    ("validate-after-pad", lambda: base64.b64decode("YQ==YQ==", validate=True)),
    ("validate-discontinuous", lambda: base64.b64decode("YQ=a", validate=True)),
    ("validate-one-extra", lambda: base64.b64decode("YWJjZ", validate=True)),
    ("validate-ok", lambda: base64.b64decode("YWJj", validate=True)),
    ("lenient-junk", lambda: base64.b64decode("@@@@notbase64@@@@")),
    ("lenient-skip", lambda: base64.b64decode("YW!Jj")),
    ("lenient-stop-at-pad", lambda: base64.b64decode("YQ==YWJj")),
    ("lenient-bad-pad", lambda: base64.b64decode("YWJjZA")),
    ("altchars", lambda: base64.b64decode(b"-_-_", altchars=b"-_")),
    ("urlsafe", lambda: base64.urlsafe_b64decode("YWJjZA")),
    ("standard", lambda: base64.standard_b64decode("YWJjZA")),
    ("decodebytes", lambda: base64.decodebytes(b"YWJjZA")),
    ("b32-pad", lambda: base64.b32decode("MFRGG")),
    ("b32-digit-before-pad", lambda: base64.b32decode("MFRGG!==")),
    ("b32-lower", lambda: base64.b32decode("mfrgg===")),
    ("b32hex-pad", lambda: base64.b32hexdecode("C5H66")),
    ("b16-odd", lambda: base64.b16decode("ABC")),
    ("b16-digit", lambda: base64.b16decode("ZZ")),
    ("b16-lower", lambda: base64.b16decode("ab")),
    ("b16-casefold", lambda: base64.b16decode("ab", casefold=True)),
    ("a2b-strict", lambda: binascii.a2b_base64(b"YW!Jj", strict_mode=True)),
    ("a2b-lenient", lambda: binascii.a2b_base64(b"YW!Jj")),
    ("a2b-str", lambda: binascii.a2b_base64("YWJj")),
    ("a2b-non-ascii-str", lambda: binascii.a2b_base64("YWJjé")),
    ("unhexlify-odd", lambda: binascii.unhexlify(b"abc")),
    ("unhexlify-digit", lambda: binascii.unhexlify(b"zz")),
    ("a2b-hex-odd", lambda: binascii.a2b_hex("abc")),
    ("a2b-uu-trailing", lambda: binascii.a2b_uu(b"!!!!")),
    ("a2b-uu-illegal", lambda: binascii.a2b_uu(b"#8\x016)C")),
    ("a2b-uu-short", lambda: binascii.a2b_uu(b"#86")),
    ("b2a-uu-long", lambda: binascii.b2a_uu(b"x" * 46)),
    ("a85", lambda: base64.a85decode(b"\xff")),
]
for label, fn in cases:
    check(label, fn)

try:
    base64.b64decode("@@@@notbase64@@@@", validate=True)
except (binascii.Error, ValueError) as exc:
    print("caught", type(exc).__name__, exc)
