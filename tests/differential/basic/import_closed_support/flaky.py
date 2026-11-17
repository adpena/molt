import import_closed_support as _support

_support.attempts = getattr(_support, "attempts", 0) + 1
if _support.attempts == 1:
    raise RuntimeError("first body run fails")
READY = True
