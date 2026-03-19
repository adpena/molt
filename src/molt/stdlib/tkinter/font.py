"""Intrinsic-backed `tkinter.font` wrappers."""

import tkinter as _tkinter
from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_TK_CALL = _require_intrinsic("molt_tk_call")

NORMAL = "normal"
ROMAN = "roman"
BOLD = "bold"
ITALIC = "italic"


def _resolve_root(root):
    if root is None:
        return _tkinter._get_default_root()
    if not isinstance(root, _tkinter.Misc):
        raise TypeError("font root must be a tkinter widget or root")
    return root


def _app_handle(widget_or_root):
    app = widget_or_root._tk_app
    return getattr(app, "_handle", app)


def _normalize_option_name(name):
    return name if name.startswith("-") else f"-{name}"


def _normalize_options(options):
    normalized = []
    for key, value in options.items():
        if value is None:
            continue
        normalized.append(_normalize_option_name(str(key)))
        normalized.append(value)
    return normalized


def _font_call(root, operation, *argv):
    del operation
    tk_root = _resolve_root(root)
    return tk_root, _MOLT_TK_CALL(_app_handle(tk_root), ["font", *argv])


def families(root=None, displayof=None):
    argv = ["families"]
    if displayof is not None:
        argv.extend(["-displayof", displayof])
    tk_root, result = _font_call(root, "tkinter.font.families", *argv)
    return tuple(tk_root.splitlist(result))


def names(root=None):
    tk_root, result = _font_call(root, "tkinter.font.names", "names")
    return tuple(tk_root.splitlist(result))


def nametofont(name, root=None):
    return Font(root=root, name=name, exists=True)


class Font:
    """Thin wrapper around Tk `font` commands."""

    def __init__(self, root=None, font=None, name=None, exists=False, **options):
        self._root = _resolve_root(root)
        self.tk = self._root.tk
        if font is not None:
            self.name = str(font)
            self._exists = True
        else:
            self.name = str(name) if name is not None else f"font{abs(id(self))}"
            self._exists = bool(exists)

        if not self._exists:
            _MOLT_TK_CALL(
                _app_handle(self._root),
                ["font", "create", self.name, *_normalize_options(options)],
            )
        elif options:
            self.configure(**options)

    def __del__(self):
        if not self._exists:
            root = getattr(self, "_root", None)
            if root is not None:
                try:
                    _MOLT_TK_CALL(_app_handle(root), ["font", "delete", self.name])
                except Exception:  # noqa: BLE001
                    pass

    def _call(self, operation, *argv):
        del operation
        return _MOLT_TK_CALL(_app_handle(self._root), ["font", *argv])

    def __str__(self):
        return self.name

    def __eq__(self, other):
        if not isinstance(other, Font):
            return NotImplemented
        return self.name == other.name

    def __hash__(self):
        return hash(self.name)

    def actual(self, option=None, displayof=None):
        argv = ["actual", self.name]
        if displayof is not None:
            argv.extend(["-displayof", displayof])
        if option is not None:
            argv.append(_normalize_option_name(str(option)))
        return self._call("tkinter.font.Font.actual", *argv)

    def cget(self, option):
        return self.actual(option)

    def configure(self, **options):
        if options:
            return self._call(
                "tkinter.font.Font.configure",
                "configure",
                self.name,
                *_normalize_options(options),
            )
        return self._call("tkinter.font.Font.configure", "configure", self.name)

    def config(self, **options):
        return self.configure(**options)

    def copy(self):
        copied_name = f"font{abs(hash((self.name, id(self))))}"
        self._call("tkinter.font.Font.copy", "create", copied_name, "-copy", self.name)
        return Font(root=self._root, name=copied_name, exists=True)

    def measure(self, text, displayof=None):
        argv = ["measure", self.name]
        if displayof is not None:
            argv.extend(["-displayof", displayof])
        argv.append(text)
        return int(self._call("tkinter.font.Font.measure", *argv))

    def metrics(self, option=None, **options):
        if option is not None and options:
            raise TypeError("metrics() option cannot be combined with keyword options")
        if option is not None:
            return self._call(
                "tkinter.font.Font.metrics",
                "metrics",
                self.name,
                _normalize_option_name(str(option)),
            )
        if options:
            return self._call(
                "tkinter.font.Font.metrics",
                "configure",
                self.name,
                *_normalize_options(options),
            )
        return self._call("tkinter.font.Font.metrics", "metrics", self.name)

    def delete(self):
        return self._call("tkinter.font.Font.delete", "delete", self.name)


__all__ = [
    "BOLD",
    "Font",
    "ITALIC",
    "NORMAL",
    "ROMAN",
    "families",
    "nametofont",
    "names",
]

globals().pop("_require_intrinsic", None)
