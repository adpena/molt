"""Intrinsic-backed textwrap wrappers for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["TextWrapper", "wrap", "fill", "indent", "dedent", "shorten"]

_MOLT_TEXTWRAP_WRAP_EX = _require_intrinsic("molt_textwrap_wrap_ex", globals())
_MOLT_TEXTWRAP_FILL_EX = _require_intrinsic("molt_textwrap_fill_ex", globals())
_MOLT_TEXTWRAP_INDENT_EX = _require_intrinsic("molt_textwrap_indent_ex", globals())
_MOLT_TEXTWRAP_DEDENT = _require_intrinsic("molt_textwrap_dedent", globals())
_MOLT_TEXTWRAP_SHORTEN = _require_intrinsic("molt_textwrap_shorten", globals())


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
        max_lines_placeholder = (self.max_lines, self.placeholder)
        out = _MOLT_TEXTWRAP_WRAP_EX(
            text,
            self.width,
            self.initial_indent,
            self.subsequent_indent,
            self.expand_tabs,
            self.replace_whitespace,
            self.fix_sentence_endings,
            self.break_long_words,
            self.drop_whitespace,
            self.break_on_hyphens,
            self.tabsize,
            max_lines_placeholder,
        )
        if not isinstance(out, list) or not all(isinstance(item, str) for item in out):
            raise RuntimeError("textwrap.wrap intrinsic returned invalid value")
        return list(out)

    def fill(self, text: str) -> str:
        max_lines_placeholder = (self.max_lines, self.placeholder)
        out = _MOLT_TEXTWRAP_FILL_EX(
            text,
            self.width,
            self.initial_indent,
            self.subsequent_indent,
            self.expand_tabs,
            self.replace_whitespace,
            self.fix_sentence_endings,
            self.break_long_words,
            self.drop_whitespace,
            self.break_on_hyphens,
            self.tabsize,
            max_lines_placeholder,
        )
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
    out = _MOLT_TEXTWRAP_INDENT_EX(text, prefix, predicate)
    if not isinstance(out, str):
        raise RuntimeError("textwrap.indent intrinsic returned invalid value")
    return out


def dedent(text: str) -> str:
    """Remove any common leading whitespace from all lines in text."""
    out = _MOLT_TEXTWRAP_DEDENT(text)
    if not isinstance(out, str):
        raise RuntimeError("textwrap.dedent intrinsic returned invalid value")
    return out


def shorten(text: str, width: int, **kwargs) -> str:
    """Collapse and truncate the given text to fit in the given width."""
    placeholder = kwargs.get("placeholder", " [...]")
    out = _MOLT_TEXTWRAP_SHORTEN(text, width, placeholder)
    if not isinstance(out, str):
        raise RuntimeError("textwrap.shorten intrinsic returned invalid value")
    return out
