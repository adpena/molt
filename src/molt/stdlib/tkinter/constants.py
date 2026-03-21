"""Intrinsic-backed constants for `tkinter`."""

import _tkinter as _tkimpl
from _intrinsics import require_intrinsic as _require_intrinsic
from . import _support as _tkcompat

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")
_MOLT_TK_AVAILABLE = _require_intrinsic("molt_tk_available")

NO = 0
FALSE = 0
OFF = 0
YES = 1
TRUE = 1
ON = 1

N = "n"
S = "s"
E = "e"
W = "w"
NW = "nw"
NE = "ne"
SW = "sw"
SE = "se"
CENTER = "center"
NS = "ns"
EW = "ew"
NSEW = "nsew"
ANCHOR = "anchor"

TOP = "top"
BOTTOM = "bottom"
LEFT = "left"
RIGHT = "right"
BOTH = "both"
X = "x"
Y = "y"

ACTIVE = "active"
DISABLED = "disabled"
NORMAL = "normal"
HIDDEN = "hidden"
END = "end"
INSERT = "insert"
CURRENT = "current"
SEL = "sel"
SEL_FIRST = "sel.first"
SEL_LAST = "sel.last"
FIRST = "first"
LAST = "last"

HORIZONTAL = "horizontal"
VERTICAL = "vertical"
RAISED = "raised"
SUNKEN = "sunken"
FLAT = "flat"
RIDGE = "ridge"
GROOVE = "groove"
SOLID = "solid"
ROUND = "round"
PROJECTING = "projecting"
BUTT = "butt"
BEVEL = "bevel"
MITER = "miter"
BASELINE = "baseline"
INSIDE = "inside"
OUTSIDE = "outside"

ALL = "all"
NONE = "none"
UNITS = "units"
PAGES = "pages"
CHAR = "char"
WORD = "word"
NUMERIC = "numeric"
MOVETO = "moveto"
SCROLL = "scroll"

CASCADE = "cascade"
CHECKBUTTON = "checkbutton"
COMMAND = "command"
RADIOBUTTON = "radiobutton"
SEPARATOR = "separator"

SINGLE = "single"
BROWSE = "browse"
MULTIPLE = "multiple"
EXTENDED = "extended"
DOTBOX = "dotbox"
UNDERLINE = "underline"

PIESLICE = "pieslice"
CHORD = "chord"
ARC = "arc"

TK_VERSION = _tkimpl.TK_VERSION
TCL_VERSION = _tkimpl.TCL_VERSION
READABLE = _tkimpl.READABLE
WRITABLE = _tkimpl.WRITABLE
EXCEPTION = _tkimpl.EXCEPTION
DONT_WAIT = _tkimpl.DONT_WAIT
ALL_EVENTS = _tkimpl.ALL_EVENTS
FILE_EVENTS = _tkimpl.FILE_EVENTS
TIMER_EVENTS = _tkimpl.TIMER_EVENTS
IDLE_EVENTS = _tkimpl.IDLE_EVENTS
WINDOW_EVENTS = _tkimpl.WINDOW_EVENTS

TK_AVAILABLE = _tkcompat._tk_available()
HAS_GUI_CAPABILITY = _tkcompat._has_gui_capability()

__all__ = [name for name in globals() if name.isupper() and not name.startswith("_")]

globals().pop("_require_intrinsic", None)
