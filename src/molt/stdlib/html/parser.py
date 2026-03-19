"""Intrinsic-backed html.parser module for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_molt_html_parser_new = _require_intrinsic("molt_html_parser_new")
_molt_html_parser_feed = _require_intrinsic("molt_html_parser_feed")
_molt_html_parser_close = _require_intrinsic("molt_html_parser_close")
_molt_html_parser_drop = _require_intrinsic("molt_html_parser_drop")


class HTMLParser:
    """Find tags and other markup and call handler functions.

    Usage:
        p = HTMLParser()
        p.feed(data)
        ...
        p.close()

    Start tags are handled by calling self.handle_starttag() or
    self.handle_startendtag(); end tags by self.handle_endtag().  The
    data between tags is passed from the parser to the derived class
    by calling self.handle_data() with the data as argument (the data
    may be split up in arbitrary chunks).  If convert_charrefs is
    True the character references are converted automatically to the
    corresponding char (and handle_data() is not called for them).
    """

    def __init__(self, *, convert_charrefs: bool = True) -> None:
        self.convert_charrefs = convert_charrefs
        self._handle = _molt_html_parser_new(bool(convert_charrefs))

    def feed(self, data: str) -> None:
        """Feed some text to the parser.

        Call this as often as you want, with as little or as much text
        as you want (can even be empty).
        """
        events = _molt_html_parser_feed(self._handle, str(data))
        self._dispatch_events(events)

    def close(self) -> None:
        """Handle any buffered data."""
        events = _molt_html_parser_close(self._handle)
        self._dispatch_events(events)

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _molt_html_parser_drop(handle)
            except Exception:
                pass

    def _dispatch_events(self, events: Any) -> None:
        if events is None:
            return
        for event in events:
            kind = event[0]
            if kind == "starttag":
                self.handle_starttag(event[1], event[2])
            elif kind == "endtag":
                self.handle_endtag(event[1])
            elif kind == "data":
                self.handle_data(event[1])
            elif kind == "comment":
                self.handle_comment(event[1])
            elif kind == "decl":
                self.handle_decl(event[1])
            elif kind == "pi":
                self.handle_pi(event[1])
            elif kind == "startendtag":
                self.handle_startendtag(event[1], event[2])
            elif kind == "entityref":
                self.handle_entityref(event[1])
            elif kind == "charref":
                self.handle_charref(event[1])

    def reset(self) -> None:
        """Reset this instance. Loses all unprocessed data."""
        if self._handle is not None:
            try:
                _molt_html_parser_drop(self._handle)
            except Exception:
                pass
        self._handle = _molt_html_parser_new(bool(self.convert_charrefs))

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        """Override to handle start tags."""

    def handle_endtag(self, tag: str) -> None:
        """Override to handle end tags."""

    def handle_startendtag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        """Override to handle start-end (self-closing) tags."""
        self.handle_starttag(tag, attrs)
        self.handle_endtag(tag)

    def handle_data(self, data: str) -> None:
        """Override to handle data between tags."""

    def handle_entityref(self, name: str) -> None:
        """Override to handle entity references (e.g. &amp;)."""

    def handle_charref(self, name: str) -> None:
        """Override to handle character references (e.g. &#62;)."""

    def handle_comment(self, data: str) -> None:
        """Override to handle comments (e.g. <!-- ... -->)."""

    def handle_decl(self, decl: str) -> None:
        """Override to handle declarations (e.g. <!DOCTYPE ...>)."""

    def handle_pi(self, data: str) -> None:
        """Override to handle processing instructions (e.g. <?...>)."""
