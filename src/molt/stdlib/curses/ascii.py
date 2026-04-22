"""Public API surface shim for ``curses.ascii``."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")

NUL = 0x00
SOH = 0x01
STX = 0x02
ETX = 0x03
EOT = 0x04
ENQ = 0x05
ACK = 0x06
BEL = 0x07
BS = 0x08
HT = 0x09
TAB = HT
LF = 0x0A
NL = LF
VT = 0x0B
FF = 0x0C
CR = 0x0D
SO = 0x0E
SI = 0x0F
DLE = 0x10
DC1 = 0x11
DC2 = 0x12
DC3 = 0x13
DC4 = 0x14
NAK = 0x15
SYN = 0x16
ETB = 0x17
CAN = 0x18
EM = 0x19
SUB = 0x1A
ESC = 0x1B
FS = 0x1C
GS = 0x1D
RS = 0x1E
US = 0x1F
SP = 0x20
DEL = 0x7F

controlnames = [
    "^@",
    "^A",
    "^B",
    "^C",
    "^D",
    "^E",
    "^F",
    "^G",
    "^H",
    "^I",
    "^J",
    "^K",
    "^L",
    "^M",
    "^N",
    "^O",
    "^P",
    "^Q",
    "^R",
    "^S",
    "^T",
    "^U",
    "^V",
    "^W",
    "^X",
    "^Y",
    "^Z",
    "^[",
    "^\\",
    "^]",
    "^^",
    "^_",
    " ",
]


def _ctoi(c) -> int:
    if isinstance(c, str):
        if not c:
            return 0
        return ord(c[0])
    return int(c)


def ascii(c) -> int:
    return _ctoi(c) & 0x7F


def ctrl(c) -> int:
    return ascii(c) & 0x1F


def alt(c) -> int:
    return _ctoi(c) | 0x80


def isascii(c) -> bool:
    return 0 <= _ctoi(c) <= 0x7F


def isctrl(c) -> bool:
    v = _ctoi(c)
    return v < SP or v == DEL


def iscntrl(c) -> bool:
    return isctrl(c)


def isblank(c) -> bool:
    return _ctoi(c) in (SP, HT)


def isspace(c) -> bool:
    return _ctoi(c) in (SP, HT, LF, VT, FF, CR)


def isdigit(c) -> bool:
    v = _ctoi(c)
    return ord("0") <= v <= ord("9")


def islower(c) -> bool:
    v = _ctoi(c)
    return ord("a") <= v <= ord("z")


def isupper(c) -> bool:
    v = _ctoi(c)
    return ord("A") <= v <= ord("Z")


def isalpha(c) -> bool:
    return islower(c) or isupper(c)


def isalnum(c) -> bool:
    return isalpha(c) or isdigit(c)


def isxdigit(c) -> bool:
    v = _ctoi(c)
    return isdigit(v) or ord("a") <= v <= ord("f") or ord("A") <= v <= ord("F")


def ispunct(c) -> bool:
    v = _ctoi(c)
    return isgraph(v) and not isalnum(v)


def isgraph(c) -> bool:
    v = _ctoi(c)
    return SP < v < DEL


def isprint(c) -> bool:
    v = _ctoi(c)
    return SP <= v < DEL


def ismeta(c) -> bool:
    return _ctoi(c) > 0x7F


def unctrl(c) -> str:
    v = _ctoi(c)
    if isascii(v):
        if isprint(v):
            return chr(v)
        if v == DEL:
            return "^?"
        return controlnames[v]
    return f"!{unctrl(v & 0x7F)}"


globals().pop("_require_intrinsic", None)
