"""Purpose: differential coverage for selectors fileobj fileno errors."""

import selectors


class BadFile:
    def fileno(self) -> int:
        raise OSError("bad fileno")


sel = selectors.DefaultSelector()
try:
    sel.register(BadFile(), selectors.EVENT_READ)
except Exception as exc:
    print(type(exc).__name__)
