"""Public API surface shim for ``email.contentmanager``."""

from __future__ import annotations


from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


class ContentManager:
    def get_content(self, msg, *args, **kw):
        del args, kw
        return getattr(msg, "get_payload", lambda: None)()

    def set_content(self, msg, obj, *args, **kw):
        del args, kw
        setter = getattr(msg, "set_payload", None)
        if callable(setter):
            setter(obj)
        return None


def get_non_text_content(msg):
    return getattr(msg, "get_payload", lambda: None)()


def get_text_content(msg):
    payload = getattr(msg, "get_payload", lambda: "")()
    if isinstance(payload, str):
        return payload
    if payload is None:
        return ""
    return str(payload)


def get_message_content(msg):
    return getattr(msg, "get_payload", lambda: None)()


def get_and_fixup_unknown_message_content(msg):
    return get_message_content(msg)


def set_bytes_content(msg, data, *args, **kw):
    del args, kw
    setter = getattr(msg, "set_payload", None)
    if callable(setter):
        setter(data)
    return None


def set_text_content(msg, text, *args, **kw):
    del args, kw
    setter = getattr(msg, "set_payload", None)
    if callable(setter):
        setter(text)
    return None


def set_message_content(msg, message, *args, **kw):
    del args, kw
    setter = getattr(msg, "set_payload", None)
    if callable(setter):
        setter(message)
    return None


raw_data_manager = ContentManager()
