"""Internationalization and localization support — CPython 3.12 parity for Molt.

Provides the standard GNU gettext interface including NullTranslations,
GNUTranslations, translation(), install(), textdomain(), bindtextdomain(),
gettext(), ngettext(), pgettext(), npgettext(), and the d*-family of
domain-specific variants.

Key difference from CPython: plural form evaluation is done via a pure
recursive descent evaluator instead of exec/eval (Molt restriction).
"""

from __future__ import annotations

import operator
import os
import re
import struct
import sys
from _intrinsics import require_intrinsic as _require_intrinsic

_molt_gettext_gettext = _require_intrinsic("molt_gettext_gettext")
_molt_gettext_ngettext = _require_intrinsic("molt_gettext_ngettext")

__all__ = [
    "NullTranslations",
    "GNUTranslations",
    "Catalog",
    "bindtextdomain",
    "find",
    "translation",
    "install",
    "textdomain",
    "dgettext",
    "dngettext",
    "gettext",
    "ngettext",
    "pgettext",
    "dpgettext",
    "npgettext",
    "dnpgettext",
]

_default_localedir = os.path.join(sys.base_prefix, "share", "locale")


# ---------------------------------------------------------------------------
# Plural form expression parser and evaluator
# ---------------------------------------------------------------------------
# The gettext plural form mini-language is a strict subset of C:
#   operators: || && == != < > <= >= + - * / % ? : ! ()
#   variables: n (the count)
#   literals:  decimal integers
#
# We parse into an AST (nested tuples) and evaluate directly — no exec.

_token_re = re.compile(
    r"""
    (?P<WS>[ \t]+)                          |
    (?P<NUM>[0-9]+\b)                       |
    (?P<NAME>n\b)                           |
    (?P<PAREN>[()])                         |
    (?P<OP>[-*/%+?:]|[><!]=?|==|&&|\|\|)   |
    (?P<BAD>\w+|.)
""",
    re.VERBOSE | re.DOTALL,
)


def _tokenize(plural: str):
    """Yield (kind, value) pairs, skipping whitespace; then yield ('', '')."""
    for m in _token_re.finditer(plural):
        kind = m.lastgroup
        if kind == "WS":
            continue
        value = m.group(kind)
        if kind == "BAD":
            raise ValueError("invalid token in plural form: %s" % value)
        yield kind, value
    yield "", ""


# Operator precedence levels (higher = binds tighter)
_PREC: dict[str, int] = {
    "||": 1,
    "&&": 2,
    "==": 3,
    "!=": 3,
    "<": 4,
    ">": 4,
    "<=": 4,
    ">=": 4,
    "+": 5,
    "-": 5,
    "*": 6,
    "/": 6,
    "%": 6,
}

_BIN_OPS: dict[str, object] = {
    "||": operator.or_,  # logical-or (handled specially for short-circuit)
    "&&": operator.and_,  # logical-and
    "==": operator.eq,
    "!=": operator.ne,
    "<": operator.lt,
    ">": operator.gt,
    "<=": operator.le,
    ">=": operator.ge,
    "+": operator.add,
    "-": operator.sub,
    "*": operator.mul,
    "/": operator.floordiv,
    "%": operator.mod,
}


class _PluralParser:
    """Recursive descent parser for GNU plural form expressions.

    Returns an AST represented as nested tuples:
        ('n',)                   — the variable n
        ('num', int)             — integer literal
        ('not', expr)            — logical not
        ('ternary', cond, t, f)  — cond ? t : f
        ('binop', op_str, left, right)
    """

    def __init__(self, plural: str) -> None:
        self._tokens = list(_tokenize(plural))
        self._pos = 0

    def _peek(self) -> tuple[str, str]:
        return self._tokens[self._pos]

    def _consume(self) -> tuple[str, str]:
        tok = self._tokens[self._pos]
        self._pos += 1
        return tok

    def parse(self):
        expr = self._parse_expr(0)
        kind, val = self._peek()
        if kind != "":
            raise ValueError("unexpected token in plural form: %s" % val)
        return expr

    def _parse_expr(self, min_prec: int):
        """Parse an expression at the given minimum precedence level.

        Handles: unary !, atoms, binary ops (left-associative), ternary ?: .
        """
        node = self._parse_unary()

        while True:
            kind, val = self._peek()
            if kind != "OP":
                break
            if val == "?":
                if min_prec > 0:
                    break
                self._consume()  # consume '?'
                true_expr = self._parse_expr(0)
                kind2, val2 = self._peek()
                if val2 != ":":
                    raise ValueError('expected ":" in ternary, got %r' % val2)
                self._consume()  # consume ':'
                false_expr = self._parse_expr(0)
                node = ("ternary", node, true_expr, false_expr)
                # ternary is right-associative and lowest priority; done
                break
            prec = _PREC.get(val)
            if prec is None or prec <= min_prec:
                break
            self._consume()
            right = self._parse_expr(prec)  # left-associative: use same prec
            node = ("binop", val, node, right)

        return node

    def _parse_unary(self):
        kind, val = self._peek()
        if kind == "OP" and val == "!":
            self._consume()
            operand = self._parse_unary()
            return ("not", operand)
        return self._parse_atom()

    def _parse_atom(self):
        kind, val = self._consume()
        if kind == "NUM":
            return ("num", int(val))
        if kind == "NAME":
            return ("n",)
        if kind == "PAREN" and val == "(":
            inner = self._parse_expr(0)
            kind2, val2 = self._consume()
            if val2 != ")":
                raise ValueError("unbalanced parenthesis in plural form")
            return inner
        raise ValueError("unexpected token in plural form: %r" % val)


def _eval_plural_ast(node, n: int) -> int:
    """Evaluate a plural form AST with variable n."""
    tag = node[0]
    if tag == "num":
        return node[1]
    if tag == "n":
        return n
    if tag == "not":
        return int(not _eval_plural_ast(node[1], n))
    if tag == "ternary":
        cond = _eval_plural_ast(node[1], n)
        if cond:
            return _eval_plural_ast(node[2], n)
        return _eval_plural_ast(node[3], n)
    if tag == "binop":
        op_str = node[1]
        left = _eval_plural_ast(node[2], n)
        right = _eval_plural_ast(node[3], n)
        if op_str == "||":
            return int(bool(left) or bool(right))
        if op_str == "&&":
            return int(bool(left) and bool(right))
        op_fn = _BIN_OPS[op_str]
        return int(op_fn(left, right))
    raise ValueError("unknown AST node: %r" % tag)


def c2py(plural: str):
    """Parse a C plural form expression and return a Python callable.

    The callable takes an integer *n* and returns the plural form index.
    """
    if len(plural) > 1000:
        raise ValueError("plural form expression is too long")
    try:
        ast = _PluralParser(plural).parse()
    except RecursionError:
        raise ValueError("plural form expression is too complex")

    def plural_func(n: int) -> int:
        if not isinstance(n, int):
            n = _as_int(n)
        return _eval_plural_ast(ast, n)

    return plural_func


def _as_int(n) -> int:
    """Coerce n to int, warning if n is a float."""
    try:
        return operator.index(n)
    except TypeError:
        pass
    try:
        round(n)
    except TypeError:
        raise TypeError(
            "Plural value must be an integer, got %s" % type(n).__name__
        ) from None
    import warnings

    warnings.warn(
        "Plural value must be an integer, got %s" % type(n).__name__,
        DeprecationWarning,
        stacklevel=2,
    )
    return int(n)


def _expand_lang(loc: str) -> list[str]:
    import locale as _locale

    loc = _locale.normalize(loc)
    COMPONENT_CODESET = 1 << 0
    COMPONENT_TERRITORY = 1 << 1
    COMPONENT_MODIFIER = 1 << 2
    mask = 0
    pos = loc.find("@")
    if pos >= 0:
        modifier = loc[pos:]
        loc = loc[:pos]
        mask |= COMPONENT_MODIFIER
    else:
        modifier = ""
    pos = loc.find(".")
    if pos >= 0:
        codeset = loc[pos:]
        loc = loc[:pos]
        mask |= COMPONENT_CODESET
    else:
        codeset = ""
    pos = loc.find("_")
    if pos >= 0:
        territory = loc[pos:]
        loc = loc[:pos]
        mask |= COMPONENT_TERRITORY
    else:
        territory = ""
    language = loc
    ret = []
    for i in range(mask + 1):
        if not (i & ~mask):
            val = language
            if i & COMPONENT_TERRITORY:
                val += territory
            if i & COMPONENT_CODESET:
                val += codeset
            if i & COMPONENT_MODIFIER:
                val += modifier
            ret.append(val)
    ret.reverse()
    return ret


# ---------------------------------------------------------------------------
# Translation classes
# ---------------------------------------------------------------------------


class NullTranslations:
    """A translations object that simply returns the original message."""

    def __init__(self, fp=None) -> None:
        self._info: dict[str, str] = {}
        self._charset: str | None = None
        self._fallback: NullTranslations | None = None
        if fp is not None:
            self._parse(fp)

    def _parse(self, fp) -> None:
        pass

    def add_fallback(self, fallback: "NullTranslations") -> None:
        if self._fallback:
            self._fallback.add_fallback(fallback)
        else:
            self._fallback = fallback

    def gettext(self, message: str) -> str:
        if self._fallback:
            return self._fallback.gettext(message)
        return _molt_gettext_gettext(message)

    def ngettext(self, msgid1: str, msgid2: str, n: int) -> str:
        if self._fallback:
            return self._fallback.ngettext(msgid1, msgid2, n)
        return _molt_gettext_ngettext(msgid1, msgid2, n)

    def pgettext(self, context: str, message: str) -> str:
        if self._fallback:
            return self._fallback.pgettext(context, message)
        return message

    def npgettext(self, context: str, msgid1: str, msgid2: str, n: int) -> str:
        if self._fallback:
            return self._fallback.npgettext(context, msgid1, msgid2, n)
        n = _as_int(n)
        if n == 1:
            return msgid1
        return msgid2

    def info(self) -> dict[str, str]:
        return self._info

    def charset(self) -> str | None:
        return self._charset

    def install(self, names=None) -> None:
        import builtins

        builtins.__dict__["_"] = self.gettext
        if names is not None:
            allowed = {"gettext", "ngettext", "npgettext", "pgettext"}
            for name in allowed & set(names):
                builtins.__dict__[name] = getattr(self, name)


class GNUTranslations(NullTranslations):
    """Translations backed by a GNU .mo binary catalog file."""

    # Magic number of .mo files
    LE_MAGIC = 0x950412DE
    BE_MAGIC = 0xDE120495

    # msgctxt separator: msgctxt + "\x04" + msgid
    CONTEXT = "%s\x04%s"

    # Acceptable .mo format major versions
    VERSIONS = (0, 1)

    def _get_versions(self, version: int) -> tuple[int, int]:
        """Return (major_version, minor_version) from a packed version int."""
        return (version >> 16, version & 0xFFFF)

    def _parse(self, fp) -> None:
        """Parse a GNU .mo binary catalog from the file-like object *fp*."""
        self._catalog: dict = {}
        self.plural = lambda n: int(n != 1)  # germanic plural by default

        buf = fp.read()
        buflen = len(buf)
        filename = getattr(fp, "name", "")

        if len(buf) < 4:
            raise OSError(0, "Bad magic number", filename)

        magic = struct.unpack("<I", buf[:4])[0]
        if magic == self.LE_MAGIC:
            version, msgcount, masteridx, transidx = struct.unpack("<4I", buf[4:20])
            ii = "<II"
        elif magic == self.BE_MAGIC:
            version, msgcount, masteridx, transidx = struct.unpack(">4I", buf[4:20])
            ii = ">II"
        else:
            raise OSError(0, "Bad magic number", filename)

        major_version, minor_version = self._get_versions(version)
        if major_version not in self.VERSIONS:
            raise OSError(0, "Bad version number " + str(major_version), filename)

        catalog = self._catalog
        for i in range(msgcount):
            mlen, moff = struct.unpack(ii, buf[masteridx : masteridx + 8])
            mend = moff + mlen
            tlen, toff = struct.unpack(ii, buf[transidx : transidx + 8])
            tend = toff + tlen
            if mend < buflen and tend < buflen:
                msg = buf[moff:mend]
                tmsg = buf[toff:tend]
            else:
                raise OSError(0, "File is corrupt", filename)

            if mlen == 0:
                # Catalog metadata
                lastk = None
                for b_item in tmsg.split(b"\n"):
                    item = b_item.decode().strip()
                    if not item:
                        continue
                    if item.startswith("#-#-#-#-#") and item.endswith("#-#-#-#-#"):
                        continue
                    k = v = None
                    if ":" in item:
                        k, v = item.split(":", 1)
                        k = k.strip().lower()
                        v = v.strip()
                        self._info[k] = v
                        lastk = k
                    elif lastk:
                        self._info[lastk] += "\n" + item
                    if k == "content-type":
                        self._charset = v.split("charset=")[1]
                    elif k == "plural-forms":
                        v_parts = v.split(";")
                        plural_str = v_parts[1].split("plural=")[1]
                        self.plural = c2py(plural_str)

            charset = self._charset or "ascii"
            if b"\x00" in msg:
                # Plural forms
                msgid1, msgid2 = msg.split(b"\x00")
                tmsg_parts = tmsg.split(b"\x00")
                msgid1_str = str(msgid1, charset)
                for idx, x in enumerate(tmsg_parts):
                    catalog[(msgid1_str, idx)] = str(x, charset)
            else:
                catalog[str(msg, charset)] = str(tmsg, charset)

            masteridx += 8
            transidx += 8

    def gettext(self, message: str) -> str:
        missing = object()
        tmsg = self._catalog.get(message, missing)
        if tmsg is missing:
            tmsg = self._catalog.get((message, self.plural(1)), missing)
        if tmsg is not missing:
            return tmsg
        if self._fallback:
            return self._fallback.gettext(message)
        return message

    def ngettext(self, msgid1: str, msgid2: str, n: int) -> str:
        try:
            tmsg = self._catalog[(msgid1, self.plural(n))]
        except KeyError:
            if self._fallback:
                return self._fallback.ngettext(msgid1, msgid2, n)
            if n == 1:
                tmsg = msgid1
            else:
                tmsg = msgid2
        return tmsg

    def pgettext(self, context: str, message: str) -> str:
        ctxt_msg_id = self.CONTEXT % (context, message)
        missing = object()
        tmsg = self._catalog.get(ctxt_msg_id, missing)
        if tmsg is missing:
            tmsg = self._catalog.get((ctxt_msg_id, self.plural(1)), missing)
        if tmsg is not missing:
            return tmsg
        if self._fallback:
            return self._fallback.pgettext(context, message)
        return message

    def npgettext(self, context: str, msgid1: str, msgid2: str, n: int) -> str:
        ctxt_msg_id = self.CONTEXT % (context, msgid1)
        try:
            tmsg = self._catalog[ctxt_msg_id, self.plural(n)]
        except KeyError:
            if self._fallback:
                return self._fallback.npgettext(context, msgid1, msgid2, n)
            if n == 1:
                tmsg = msgid1
            else:
                tmsg = msgid2
        return tmsg


# ---------------------------------------------------------------------------
# Module-level API
# ---------------------------------------------------------------------------


def find(
    domain: str,
    localedir: str | None = None,
    languages: list[str] | None = None,
    all: bool = False,
):
    """Locate a .mo file using the standard gettext locale search strategy."""
    if localedir is None:
        localedir = _default_localedir
    if languages is None:
        languages = []
        for envar in ("LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"):
            val = os.environ.get(envar)
            if val:
                languages = val.split(":")
                break
        if "C" not in languages:
            languages.append("C")
    # Normalize and expand language codes
    nelangs: list[str] = []
    for lang in languages:
        for nelang in _expand_lang(lang):
            if nelang not in nelangs:
                nelangs.append(nelang)
    if all:
        result: list[str] = []
    else:
        result = None  # type: ignore[assignment]
    for lang in nelangs:
        if lang == "C":
            break
        mofile = os.path.join(localedir, lang, "LC_MESSAGES", "%s.mo" % domain)
        if os.path.exists(mofile):
            if all:
                result.append(mofile)  # type: ignore[union-attr]
            else:
                return mofile
    return result


# Cache: maps (class, abspath) -> Translation instance
_translations: dict[tuple, NullTranslations] = {}


def translation(
    domain: str,
    localedir: str | None = None,
    languages: list[str] | None = None,
    class_=None,
    fallback: bool = False,
) -> NullTranslations:
    """Return a NullTranslations (or subclass) instance for the given domain."""
    if class_ is None:
        class_ = GNUTranslations
    mofiles = find(domain, localedir, languages, all=True)
    if not mofiles:
        if fallback:
            return NullTranslations()
        from errno import ENOENT

        raise FileNotFoundError(ENOENT, "No translation file found for domain", domain)
    import copy

    result: NullTranslations | None = None
    for mofile in mofiles:
        key = (class_, os.path.abspath(mofile))
        t = _translations.get(key)
        if t is None:
            with open(mofile, "rb") as fp:
                t = _translations.setdefault(key, class_(fp))
        t = copy.copy(t)
        if result is None:
            result = t
        else:
            result.add_fallback(t)
    return result  # type: ignore[return-value]


def install(domain: str, localedir: str | None = None, *, names=None) -> None:
    """Install _() in Python's builtins namespace."""
    t = translation(domain, localedir, fallback=True)
    t.install(names)


# Per-domain localedir registry and current domain
_localedirs: dict[str, str] = {}
_current_domain = "messages"


def textdomain(domain: str | None = None) -> str:
    global _current_domain
    if domain is not None:
        _current_domain = domain
    return _current_domain


def bindtextdomain(domain: str, localedir: str | None = None) -> str:
    if localedir is not None:
        _localedirs[domain] = localedir
    return _localedirs.get(domain, _default_localedir)


def dgettext(domain: str, message: str) -> str:
    try:
        t = translation(domain, _localedirs.get(domain))
    except OSError:
        return _molt_gettext_gettext(message)
    return t.gettext(message)


def dngettext(domain: str, msgid1: str, msgid2: str, n: int) -> str:
    try:
        t = translation(domain, _localedirs.get(domain))
    except OSError:
        return _molt_gettext_ngettext(msgid1, msgid2, n)
    return t.ngettext(msgid1, msgid2, n)


def dpgettext(domain: str, context: str, message: str) -> str:
    try:
        t = translation(domain, _localedirs.get(domain))
    except OSError:
        return message
    return t.pgettext(context, message)


def dnpgettext(domain: str, context: str, msgid1: str, msgid2: str, n: int) -> str:
    try:
        t = translation(domain, _localedirs.get(domain))
    except OSError:
        if n == 1:
            return msgid1
        return msgid2
    return t.npgettext(context, msgid1, msgid2, n)


def gettext(message: str) -> str:
    return dgettext(_current_domain, message)


def ngettext(msgid1: str, msgid2: str, n: int) -> str:
    return dngettext(_current_domain, msgid1, msgid2, n)


def pgettext(context: str, message: str) -> str:
    return dpgettext(_current_domain, context, message)


def npgettext(context: str, msgid1: str, msgid2: str, n: int) -> str:
    return dnpgettext(_current_domain, context, msgid1, msgid2, n)


# James Henstridge's Catalog alias
Catalog = translation
