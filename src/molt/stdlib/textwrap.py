"""Intrinsic-backed textwrap wrappers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["TextWrapper", "wrap", "fill", "indent"]

_MOLT_TEXTWRAP_WRAP = _require_intrinsic("molt_textwrap_wrap", globals())
_MOLT_TEXTWRAP_FILL = _require_intrinsic("molt_textwrap_fill", globals())
_MOLT_TEXTWRAP_INDENT = _require_intrinsic("molt_textwrap_indent", globals())


_TEXTWRAPPER_OPTION_DEFAULTS = {
    "initial_indent": "",
    "subsequent_indent": "",
    "expand_tabs": True,
    "replace_whitespace": True,
    "fix_sentence_endings": False,
    "break_long_words": True,
    "drop_whitespace": True,
    "break_on_hyphens": True,
    "tabsize": 8,
    "max_lines": None,
    "placeholder": " [...]",
}


def _unsupported_option_names(wrapper: "TextWrapper") -> list[str]:
    unsupported: list[str] = []
    for name, default in _TEXTWRAPPER_OPTION_DEFAULTS.items():
        if getattr(wrapper, name) != default:
            unsupported.append(name)
    return unsupported


def _require_supported_wrap_options(wrapper: "TextWrapper", api: str) -> None:
    unsupported = _unsupported_option_names(wrapper)
    if not unsupported:
        return
    quoted = ", ".join(repr(name) for name in unsupported)
    label = "option" if len(unsupported) == 1 else "options"
    raise RuntimeError(
        f"textwrap.{api} {label} {quoted} require runtime intrinsic support; "
        "this build currently supports width-only wrapping via "
        "molt_textwrap_wrap/molt_textwrap_fill"
    )


class TextWrapper:
    def __init__(
        self,
        width: int = 70,
        initial_indent: str = "",
        subsequent_indent: str = "",
        expand_tabs: bool = True,
        replace_whitespace: bool = True,
        fix_sentence_endings: bool = False,
        break_long_words: bool = True,
        drop_whitespace: bool = True,
        break_on_hyphens: bool = True,
        tabsize: int = 8,
        *,
        max_lines: int | None = None,
        placeholder: str = " [...]",
    ) -> None:
        self.width = width
        self.initial_indent = initial_indent
        self.subsequent_indent = subsequent_indent
        self.expand_tabs = expand_tabs
        self.replace_whitespace = replace_whitespace
        self.fix_sentence_endings = fix_sentence_endings
        self.break_long_words = break_long_words
        self.drop_whitespace = drop_whitespace
        self.break_on_hyphens = break_on_hyphens
        self.tabsize = tabsize
        self.max_lines = max_lines
        self.placeholder = placeholder

    def wrap(self, text: str) -> list[str]:
        _require_supported_wrap_options(self, "wrap")
        out = _MOLT_TEXTWRAP_WRAP(text, self.width)
        if not isinstance(out, list) or not all(isinstance(item, str) for item in out):
            raise RuntimeError("textwrap.wrap intrinsic returned invalid value")
        return list(out)

    def fill(self, text: str) -> str:
        _require_supported_wrap_options(self, "fill")
        out = _MOLT_TEXTWRAP_FILL(text, self.width)
        if not isinstance(out, str):
            raise RuntimeError("textwrap.fill intrinsic returned invalid value")
        return out


def wrap(text: str, width: int = 70, **kwargs) -> list[str]:
    wrapper = TextWrapper(width=width, **kwargs)
    return wrapper.wrap(text)


def fill(text: str, width: int = 70, **kwargs) -> str:
    wrapper = TextWrapper(width=width, **kwargs)
    return wrapper.fill(text)


def indent(text: str, prefix: str, predicate=None) -> str:
    if predicate is not None:
        raise RuntimeError(
            "textwrap.indent(predicate=...) requires runtime intrinsic support; "
            "this build only supports predicate=None"
        )
    out = _MOLT_TEXTWRAP_INDENT(text, prefix)
    if not isinstance(out, str):
        raise RuntimeError("textwrap.indent intrinsic returned invalid value")
    return out
