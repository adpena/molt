"""Intrinsic-backed email.policy subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_EMAIL_POLICY_NEW = _require_intrinsic("molt_email_policy_new", globals())


class EmailPolicy:
    # TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:partial): add full policy hooks (header factories, line length controls, content manager integration).
    __slots__ = ("name", "utf8")

    def __init__(self, name: str = "default", utf8: bool = False) -> None:
        self.name = str(name)
        self.utf8 = bool(utf8)

    def clone(self, **kwargs) -> "EmailPolicy":
        name = kwargs.get("name", self.name)
        utf8 = kwargs.get("utf8", self.utf8)
        return _new_policy(str(name), bool(utf8))

    def __repr__(self) -> str:
        return f"EmailPolicy(name={self.name!r}, utf8={self.utf8!r})"

    def __eq__(self, other) -> bool:
        return (
            isinstance(other, EmailPolicy)
            and self.name == other.name
            and self.utf8 == other.utf8
        )


def _new_policy(name: str, utf8: bool) -> EmailPolicy:
    payload = _MOLT_EMAIL_POLICY_NEW(str(name), bool(utf8))
    if (
        not isinstance(payload, tuple)
        or len(payload) != 2
        or not isinstance(payload[0], str)
    ):
        raise RuntimeError("email.policy intrinsic returned invalid value")
    return EmailPolicy(name=payload[0], utf8=bool(payload[1]))


default = _new_policy("default", False)
strict = _new_policy("strict", False)
SMTP = _new_policy("SMTP", False)
SMTPUTF8 = _new_policy("SMTPUTF8", True)
HTTP = _new_policy("HTTP", False)
compat32 = _new_policy("compat32", False)


__all__ = [
    "EmailPolicy",
    "HTTP",
    "SMTP",
    "SMTPUTF8",
    "compat32",
    "default",
    "strict",
]
