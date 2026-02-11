"""Intrinsic-backed subset of email.message for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

_MOLT_EMAIL_MESSAGE_NEW = _require_intrinsic("molt_email_message_new", globals())
_MOLT_EMAIL_MESSAGE_SET = _require_intrinsic("molt_email_message_set", globals())
_MOLT_EMAIL_MESSAGE_GET = _require_intrinsic("molt_email_message_get", globals())
_MOLT_EMAIL_MESSAGE_SET_CONTENT = _require_intrinsic(
    "molt_email_message_set_content", globals()
)
_MOLT_EMAIL_MESSAGE_ADD_ALTERNATIVE = _require_intrinsic(
    "molt_email_message_add_alternative", globals()
)
_MOLT_EMAIL_MESSAGE_ADD_ATTACHMENT = _require_intrinsic(
    "molt_email_message_add_attachment", globals()
)
_MOLT_EMAIL_MESSAGE_IS_MULTIPART = _require_intrinsic(
    "molt_email_message_is_multipart", globals()
)
_MOLT_EMAIL_MESSAGE_PAYLOAD = _require_intrinsic(
    "molt_email_message_payload", globals()
)
_MOLT_EMAIL_MESSAGE_CONTENT = _require_intrinsic(
    "molt_email_message_content", globals()
)
_MOLT_EMAIL_MESSAGE_CONTENT_TYPE = _require_intrinsic(
    "molt_email_message_content_type", globals()
)
_MOLT_EMAIL_MESSAGE_FILENAME = _require_intrinsic(
    "molt_email_message_filename", globals()
)
_MOLT_EMAIL_MESSAGE_AS_STRING = _require_intrinsic(
    "molt_email_message_as_string", globals()
)
_MOLT_EMAIL_MESSAGE_ITEMS = _require_intrinsic("molt_email_message_items", globals())
_MOLT_EMAIL_MESSAGE_DROP = _require_intrinsic("molt_email_message_drop", globals())


def _header_value_to_text(value) -> str:
    if isinstance(value, str):
        return value
    encoder = getattr(value, "encode", None)
    if callable(encoder):
        encoded = encoder()
        if isinstance(encoded, str):
            return encoded
    return str(value)


class EmailMessage:
    # TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): implement full EmailMessage policy-aware tree walking, charset transfer-encoding knobs, and structured header object model parity.
    def __init__(self, *, policy=None) -> None:
        self.policy = policy
        self._handle = _MOLT_EMAIL_MESSAGE_NEW()
        self._header_objects: dict[str, list[object]] = {}

    @classmethod
    def _from_handle(cls, handle, *, policy=None):
        self = cls.__new__(cls)
        self.policy = policy
        self._handle = handle
        self._header_objects = {}
        return self

    def __setitem__(self, name: str, value) -> None:
        value_type = type(value)
        if getattr(value_type, "__name__", "") == "Header":
            # Match CPython's default policy path where Header isn't accepted.
            raise TypeError("'Header' object is not subscriptable")
        name_text = str(name)
        text_value = _header_value_to_text(value)
        _MOLT_EMAIL_MESSAGE_SET(self._handle, name_text, text_value)
        stored_value = value
        lower = name_text.lower()
        if lower in {"from", "to", "cc", "bcc", "reply-to"} and hasattr(
            value, "addr_spec"
        ):
            stored_value = _UniqueAddressHeader(text_value)
        self._header_objects.setdefault(lower, []).append(stored_value)

    def __getitem__(self, name: str):
        name_text = str(name)
        values = self._header_objects.get(name_text.lower())
        if values:
            return values[-1]
        value = _MOLT_EMAIL_MESSAGE_GET(self._handle, name_text)
        if value is None:
            raise KeyError(name_text)
        return value

    def get(self, name: str, default=None):
        try:
            return self[name]
        except KeyError:
            return default

    def items(self) -> list[tuple[str, str]]:
        raw = _MOLT_EMAIL_MESSAGE_ITEMS(self._handle)
        if not isinstance(raw, list):
            raise RuntimeError("email.message items intrinsic returned invalid value")
        out: list[tuple[str, str]] = []
        for item in raw:
            if not isinstance(item, tuple) or len(item) != 2:
                raise RuntimeError(
                    "email.message items intrinsic returned invalid value"
                )
            out.append((str(item[0]), str(item[1])))
        return out

    def keys(self) -> list[str]:
        return [name for name, _ in self.items()]

    def values(self) -> list[str]:
        return [value for _, value in self.items()]

    def set_content(self, content: str, *args, **kwargs) -> None:
        if args:
            raise RuntimeError(
                "email.message.set_content positional extras unsupported"
            )
        if kwargs:
            unsupported = ", ".join(sorted(str(k) for k in kwargs))
            raise RuntimeError(
                f"email.message.set_content unsupported keyword arguments: {unsupported}"
            )
        _MOLT_EMAIL_MESSAGE_SET_CONTENT(self._handle, str(content))

    def add_alternative(
        self,
        content: str,
        subtype: str = "plain",
        *args,
        **kwargs,
    ) -> None:
        if args:
            raise RuntimeError(
                "email.message.add_alternative positional extras unsupported"
            )
        if kwargs:
            unsupported = ", ".join(sorted(str(k) for k in kwargs))
            raise RuntimeError(
                "email.message.add_alternative unsupported keyword arguments: "
                f"{unsupported}"
            )
        _MOLT_EMAIL_MESSAGE_ADD_ALTERNATIVE(self._handle, str(content), str(subtype))

    def add_attachment(
        self,
        data,
        *,
        maintype: str,
        subtype: str,
        filename: str | None = None,
        **kwargs,
    ) -> None:
        if kwargs:
            unsupported = ", ".join(sorted(str(k) for k in kwargs))
            raise RuntimeError(
                "email.message.add_attachment unsupported keyword arguments: "
                f"{unsupported}"
            )
        _MOLT_EMAIL_MESSAGE_ADD_ATTACHMENT(
            self._handle,
            data,
            str(maintype),
            str(subtype),
            None if filename is None else str(filename),
        )

    def is_multipart(self) -> bool:
        return bool(_MOLT_EMAIL_MESSAGE_IS_MULTIPART(self._handle))

    def get_payload(self, i: int | None = None, decode: bool = False):
        payload = _MOLT_EMAIL_MESSAGE_PAYLOAD(self._handle)
        if isinstance(payload, list):
            parts = [
                EmailMessage._from_handle(part_handle, policy=self.policy)
                for part_handle in payload
            ]
            if i is None:
                return parts
            return parts[i]
        if i is not None:
            raise TypeError("Expected multipart payload for indexed access")
        if decode:
            return str(payload).encode("utf-8", "surrogateescape")
        return payload

    def get_content(self) -> str:
        return str(_MOLT_EMAIL_MESSAGE_CONTENT(self._handle))

    def get_body(self, preferencelist: tuple[str, ...] | None = None):
        if not self.is_multipart():
            return self
        parts = self.get_payload()
        if not isinstance(parts, list) or not parts:
            return None
        if preferencelist:
            for pref in preferencelist:
                for part in parts:
                    ctype = part.get_content_type()
                    if ctype == pref or ctype.endswith(f"/{pref}"):
                        return part
        for part in parts:
            if part.get_content_type().endswith("/plain"):
                return part
        return parts[0]

    def get_content_type(self) -> str:
        return str(_MOLT_EMAIL_MESSAGE_CONTENT_TYPE(self._handle))

    def get_filename(self) -> str | None:
        value = _MOLT_EMAIL_MESSAGE_FILENAME(self._handle)
        if value is None:
            return None
        return str(value)

    def as_string(self, *args, **kwargs) -> str:
        if args or kwargs:
            raise RuntimeError("email.message.as_string policy controls unsupported")
        return str(_MOLT_EMAIL_MESSAGE_AS_STRING(self._handle))

    def as_bytes(self, *args, **kwargs) -> bytes:
        return self.as_string(*args, **kwargs).encode("utf-8", "surrogateescape")

    def __bytes__(self) -> bytes:
        return self.as_bytes()

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is None:
            return
        try:
            _MOLT_EMAIL_MESSAGE_DROP(handle)
        except Exception:
            pass
        self._handle = None


class _UniqueAddressHeader:
    __slots__ = ("_text",)

    def __init__(self, text: str) -> None:
        self._text = str(text)

    def __str__(self) -> str:
        return self._text

    def __repr__(self) -> str:
        return f"_UniqueAddressHeader({self._text!r})"


__all__ = ["EmailMessage"]
