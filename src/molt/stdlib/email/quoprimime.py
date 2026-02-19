# Copyright (C) 2001-2006 Python Software Foundation
# Author: Ben Gertzfield
# Contact: email-sig@python.org

"""Quoted-printable content transfer encoding per RFCs 2045-2047."""

from _intrinsics import require_intrinsic as _require_intrinsic

import re
from string import ascii_letters, digits, hexdigits

__all__ = [
    "body_decode",
    "body_encode",
    "body_length",
    "decode",
    "decodestring",
    "header_decode",
    "header_encode",
    "header_length",
    "quote",
    "unquote",
]

CRLF = "\r\n"
NL = "\n"
EMPTYSTRING = ""

_MOLT_EMAIL_QUOPRIMIME_HEADER_CHECK = _require_intrinsic(
    "molt_email_quoprimime_header_check", globals()
)
_MOLT_EMAIL_QUOPRIMIME_BODY_CHECK = _require_intrinsic(
    "molt_email_quoprimime_body_check", globals()
)
_MOLT_EMAIL_QUOPRIMIME_HEADER_LENGTH = _require_intrinsic(
    "molt_email_quoprimime_header_length", globals()
)
_MOLT_EMAIL_QUOPRIMIME_BODY_LENGTH = _require_intrinsic(
    "molt_email_quoprimime_body_length", globals()
)
_MOLT_EMAIL_QUOPRIMIME_QUOTE = _require_intrinsic(
    "molt_email_quoprimime_quote", globals()
)
_MOLT_EMAIL_QUOPRIMIME_UNQUOTE = _require_intrinsic(
    "molt_email_quoprimime_unquote", globals()
)
_MOLT_EMAIL_QUOPRIMIME_HEADER_ENCODE = _require_intrinsic(
    "molt_email_quoprimime_header_encode", globals()
)
_MOLT_EMAIL_QUOPRIMIME_HEADER_DECODE = _require_intrinsic(
    "molt_email_quoprimime_header_decode", globals()
)
_MOLT_EMAIL_QUOPRIMIME_BODY_ENCODE = _require_intrinsic(
    "molt_email_quoprimime_body_encode", globals()
)
_MOLT_EMAIL_QUOPRIMIME_DECODE = _require_intrinsic(
    "molt_email_quoprimime_decode", globals()
)


def header_check(octet):
    return _MOLT_EMAIL_QUOPRIMIME_HEADER_CHECK(octet)


def body_check(octet):
    return _MOLT_EMAIL_QUOPRIMIME_BODY_CHECK(octet)


def header_length(bytearray):
    return _MOLT_EMAIL_QUOPRIMIME_HEADER_LENGTH(bytearray)


def body_length(bytearray):
    return _MOLT_EMAIL_QUOPRIMIME_BODY_LENGTH(bytearray)


def quote(c):
    return _MOLT_EMAIL_QUOPRIMIME_QUOTE(c)


def unquote(s):
    return _MOLT_EMAIL_QUOPRIMIME_UNQUOTE(s)


def header_encode(header_bytes, charset="iso-8859-1"):
    return _MOLT_EMAIL_QUOPRIMIME_HEADER_ENCODE(header_bytes, charset)


def body_encode(body, maxlinelen=76, eol=NL):
    return _MOLT_EMAIL_QUOPRIMIME_BODY_ENCODE(body, maxlinelen, eol)


def decode(encoded, eol=NL):
    return _MOLT_EMAIL_QUOPRIMIME_DECODE(encoded, eol)


body_decode = decode
decodestring = decode


def header_decode(s):
    return _MOLT_EMAIL_QUOPRIMIME_HEADER_DECODE(s)
