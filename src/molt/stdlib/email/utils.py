"""Intrinsic-backed email.utils subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_EMAIL_UTILS_MAKE_MSGID = _require_intrinsic(
    "molt_email_utils_make_msgid", globals()
)
_MOLT_EMAIL_UTILS_GETADDRESSES = _require_intrinsic(
    "molt_email_utils_getaddresses", globals()
)
_MOLT_EMAIL_UTILS_PARSEDATE_TZ = _require_intrinsic(
    "molt_email_utils_parsedate_tz", globals()
)
_MOLT_EMAIL_UTILS_FORMAT_DATETIME = _require_intrinsic(
    "molt_email_utils_format_datetime", globals()
)
_MOLT_EMAIL_UTILS_PARSEDATE_TO_DATETIME = _require_intrinsic(
    "molt_email_utils_parsedate_to_datetime", globals()
)


def make_msgid(idstring: str | None = None, domain: str | None = None) -> str:
    # CPython signature: make_msgid(idstring=None, domain=None).
    if domain is None:
        domain_arg = "localhost"
    else:
        domain_arg = str(domain)
    out = _MOLT_EMAIL_UTILS_MAKE_MSGID(domain_arg)
    if not isinstance(out, str):
        raise RuntimeError("email.utils.make_msgid intrinsic returned invalid value")
    if idstring is None:
        return out
    token = str(idstring)
    if not token:
        return out
    if out.startswith("<") and out.endswith(">") and "@" in out:
        core = out[1:-1]
        left, right = core.split("@", 1)
        return f"<{left}.{token}@{right}>"
    return out


def getaddresses(fieldvalues) -> list[tuple[str, str]]:
    out = _MOLT_EMAIL_UTILS_GETADDRESSES(fieldvalues)
    if not isinstance(out, list):
        raise RuntimeError("email.utils.getaddresses intrinsic returned invalid value")
    pairs: list[tuple[str, str]] = []
    for item in out:
        if not isinstance(item, tuple) or len(item) != 2:
            raise RuntimeError(
                "email.utils.getaddresses intrinsic returned invalid value"
            )
        pairs.append((str(item[0]), str(item[1])))
    return pairs


def parseaddr(addr: str, *, strict: bool = True) -> tuple[str, str]:
    _ = strict
    parsed = getaddresses([addr])
    if not parsed:
        return ("", "")
    return parsed[0]


def parsedate_tz(data: str):
    if not isinstance(data, str):
        raise TypeError("parsedate_tz() argument must be str")
    out = _MOLT_EMAIL_UTILS_PARSEDATE_TZ(data)
    if out is None:
        return None
    if not isinstance(out, tuple):
        raise RuntimeError("email.utils.parsedate_tz intrinsic returned invalid value")
    return out


def parsedate(data: str):
    out = parsedate_tz(data)
    if out is None:
        return None
    return out[:9]


def format_datetime(dt, usegmt: bool = False) -> str:
    out = _MOLT_EMAIL_UTILS_FORMAT_DATETIME(dt)
    if not isinstance(out, str):
        raise RuntimeError(
            "email.utils.format_datetime intrinsic returned invalid value"
        )
    if usegmt and out.endswith(" +0000"):
        return out[:-6] + " GMT"
    return out


def parsedate_to_datetime(data: str):
    out = _MOLT_EMAIL_UTILS_PARSEDATE_TO_DATETIME(data)
    return out


def localtime(dt=None):
    datetime_mod = __import__("datetime")
    datetime_cls = datetime_mod.datetime
    timezone_cls = datetime_mod.timezone
    if dt is None:
        return datetime_cls.now().astimezone()
    if dt.tzinfo is None:
        return dt.replace(tzinfo=timezone_cls.utc).astimezone()
    return dt.astimezone()


__all__ = [
    "format_datetime",
    "getaddresses",
    "localtime",
    "make_msgid",
    "parseaddr",
    "parsedate",
    "parsedate_to_datetime",
    "parsedate_tz",
]
