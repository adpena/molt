"""Public API surface shim for ``email.utils``."""

from __future__ import annotations

import datetime
import re
import time
import urllib

from _intrinsics import require_intrinsic as _require_intrinsic


_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

_MOLT_EMAIL_UTILS_MAKE_MSGID = _require_intrinsic(
    "molt_email_utils_make_msgid"
)
_MOLT_EMAIL_UTILS_GETADDRESSES = _require_intrinsic(
    "molt_email_utils_getaddresses"
)
_MOLT_EMAIL_UTILS_PARSEDATE_TZ = _require_intrinsic(
    "molt_email_utils_parsedate_tz"
)
_MOLT_EMAIL_UTILS_FORMAT_DATETIME = _require_intrinsic(
    "molt_email_utils_format_datetime"
)
_MOLT_EMAIL_UTILS_PARSEDATE_TO_DATETIME = _require_intrinsic(
    "molt_email_utils_parsedate_to_datetime"
)

COMMASPACE = ", "
EMPTYSTRING = ""
UEMPTYSTRING = ""
CRLF = "\r\n"
TICK = "'"
supports_strict_parsing = False

# Equivalent to the CPython character-class matcher, but expressed without a
# `[]` char class so it compiles on Molt's current regex parser.
specialsre = re.compile(r"\\(|\\)|<|>|@|,|:|;|\\\"|\\.|\\[|\\]")
escapesre = re.compile(r"[\\\\\"]")
rfc2231_continuation = re.compile(r"^([^*]+)\\*(\\d+)\\*?$")


def _has_surrogates(value) -> bool:
    if not isinstance(value, str):
        return False
    for ch in value:
        code = ord(ch)
        if 0xD800 <= code <= 0xDFFF:
            return True
    return False


def _sanitize(value):
    if not isinstance(value, str):
        return value
    if not _has_surrogates(value):
        return value
    try:
        return value.encode("ascii", "surrogateescape").decode("ascii", "replace")
    except Exception:
        return value


def formataddr(pair, charset: str = "utf-8") -> str:
    del charset
    name, addr = pair
    if name:
        return f'"{name}" <{addr}>'
    return str(addr)


def parseaddr(addr: str, *, strict: bool = True):
    del strict
    if "<" in addr and ">" in addr:
        name, rest = addr.split("<", 1)
        return (name.strip().strip('"'), rest.split(">", 1)[0].strip())
    return ("", addr.strip())


def getaddresses(fieldvalues, *, strict: bool = True):
    del strict
    return _MOLT_EMAIL_UTILS_GETADDRESSES(fieldvalues)


def formatdate(timeval=None, localtime: bool = False, usegmt: bool = False) -> str:
    del usegmt
    if timeval is None:
        timeval = time.time()
    dt = datetime.datetime.fromtimestamp(timeval)
    if not localtime:
        dt = datetime.datetime.utcfromtimestamp(timeval)
    return dt.strftime("%a, %d %b %Y %H:%M:%S +0000")


def format_datetime(dt: datetime.datetime, usegmt: bool = False) -> str:
    del usegmt
    return _MOLT_EMAIL_UTILS_FORMAT_DATETIME(dt)


def make_msgid(idstring=None, domain=None):
    return _MOLT_EMAIL_UTILS_MAKE_MSGID(domain)


def parsedate(date):
    parsed = parsedate_tz(date)
    if parsed is None:
        return None
    return parsed[:9]


def parsedate_tz(date):
    return _MOLT_EMAIL_UTILS_PARSEDATE_TZ(date)


def parsedate_to_datetime(data):
    return _MOLT_EMAIL_UTILS_PARSEDATE_TO_DATETIME(data)


def mktime_tz(data):
    if data is None:
        raise TypeError("Tuple or struct_time argument required")
    return int(time.mktime(tuple(data[:9])))


def localtime(dt=None):
    if dt is None:
        return datetime.datetime.now().astimezone()
    if dt.tzinfo is None:
        return dt.astimezone()
    return dt.astimezone()


def quote(value: str) -> str:
    return escapesre.sub(lambda mo: "\\" + mo.group(0), value)


def unquote(value: str) -> str:
    if len(value) > 1 and value[0] == value[-1] == '"':
        value = value[1:-1]
    return value.replace("\\\\", "\\").replace('\\"', '"')


def decode_rfc2231(value: str):
    parts = value.split("'", 2)
    if len(parts) == 3:
        return tuple(parts)
    return (None, None, value)


def encode_rfc2231(value: str, charset: str | None = None, language: str | None = None):
    if charset is None and language is None:
        return urllib.parse.quote(value)
    charset = charset or "us-ascii"
    language = language or ""
    return f"{charset}'{language}'{urllib.parse.quote(value)}"


def collapse_rfc2231_value(
    value, errors: str = "replace", fallback_charset: str = "us-ascii"
):
    del errors
    if not isinstance(value, tuple) or len(value) != 3:
        return str(value)
    charset, _lang, text = value
    if charset is None:
        charset = fallback_charset
    try:
        return urllib.parse.unquote(text, encoding=charset)
    except Exception:
        return urllib.parse.unquote(text)


def decode_params(params):
    return params

globals().pop("_require_intrinsic", None)
