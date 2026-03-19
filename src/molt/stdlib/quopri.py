"""Conversions to/from quoted-printable transport encoding as per RFC 1521."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


__all__ = ["encode", "decode", "encodestring", "decodestring"]

ESCAPE = b"="
MAXLINESIZE = 76
HEX = b"0123456789ABCDEF"
EMPTYSTRING = b""

_MOLT_QUOPRI_ENCODE = _require_intrinsic("molt_quopri_encode")
_MOLT_QUOPRI_DECODE = _require_intrinsic("molt_quopri_decode")
_MOLT_QUOPRI_NEEDS_QUOTING = _require_intrinsic("molt_quopri_needs_quoting")
_MOLT_QUOPRI_QUOTE = _require_intrinsic("molt_quopri_quote")
_MOLT_QUOPRI_ISHEX = _require_intrinsic("molt_quopri_ishex")
_MOLT_QUOPRI_UNHEX = _require_intrinsic("molt_quopri_unhex")


def _expect_bytes(value, name: str) -> bytes:
    if isinstance(value, (bytes, bytearray)):
        return bytes(value)
    raise RuntimeError(f"{name} intrinsic returned invalid value")


def _expect_bool(value, name: str) -> bool:
    if not isinstance(value, bool):
        raise RuntimeError(f"{name} intrinsic returned invalid value")
    return value


def _expect_int(value, name: str) -> int:
    if not isinstance(value, int):
        raise RuntimeError(f"{name} intrinsic returned invalid value")
    return value


def needsquoting(c, quotetabs, header):
    """Decide whether a byte value needs quoted-printable escaping."""
    assert isinstance(c, bytes)
    return _expect_bool(
        _MOLT_QUOPRI_NEEDS_QUOTING(c, quotetabs, header),
        "quopri.needsquoting",
    )


def quote(c):
    """Quote a single character."""
    assert isinstance(c, bytes) and len(c) == 1
    return _expect_bytes(_MOLT_QUOPRI_QUOTE(c), "quopri.quote")


def encode(input, output, quotetabs, header=False):
    """Read input, apply quoted-printable encoding, and write to output."""
    data = input.read()
    encoded = _expect_bytes(
        _MOLT_QUOPRI_ENCODE(data, quotetabs, header),
        "quopri.encodestring",
    )
    output.write(encoded)


def encodestring(s, quotetabs=False, header=False):
    return _expect_bytes(
        _MOLT_QUOPRI_ENCODE(s, quotetabs, header),
        "quopri.encodestring",
    )


def decode(input, output, header=False):
    """Read input, apply quoted-printable decoding, and write to output."""
    data = input.read()
    decoded = _expect_bytes(
        _MOLT_QUOPRI_DECODE(data, header),
        "quopri.decodestring",
    )
    output.write(decoded)


def decodestring(s, header=False):
    return _expect_bytes(_MOLT_QUOPRI_DECODE(s, header), "quopri.decodestring")


def ishex(c):
    """Return true if byte 'c' is a hexadecimal digit in ASCII."""
    assert isinstance(c, bytes)
    return _expect_bool(_MOLT_QUOPRI_ISHEX(c), "quopri.ishex")


def unhex(s):
    """Get the integer value of a hexadecimal number."""
    return _expect_int(_MOLT_QUOPRI_UNHEX(s), "quopri.unhex")


def main():
    import getopt
    import sys

    try:
        opts, args = getopt.getopt(sys.argv[1:], "td")
    except getopt.error as msg:
        sys.stdout = sys.stderr
        print(msg)
        print("usage: quopri [-t | -d] [file] ...")
        print("-t: quote tabs")
        print("-d: decode; default encode")
        sys.exit(2)
    deco = False
    tabs = False
    for o, a in opts:
        if o == "-t":
            tabs = True
        if o == "-d":
            deco = True
    if tabs and deco:
        sys.stdout = sys.stderr
        print("-t and -d are mutually exclusive")
        sys.exit(2)
    if not args:
        args = ["-"]
    sts = 0
    for file in args:
        if file == "-":
            fp = sys.stdin.buffer
        else:
            try:
                fp = open(file, "rb")
            except OSError as msg:
                sys.stderr.write("%s: can't open (%s)\n" % (file, msg))
                sts = 1
                continue
        try:
            if deco:
                decode(fp, sys.stdout.buffer)
            else:
                encode(fp, sys.stdout.buffer, tabs)
        finally:
            if file != "-":
                fp.close()
    if sts:
        sys.exit(sts)


if __name__ == "__main__":
    main()
