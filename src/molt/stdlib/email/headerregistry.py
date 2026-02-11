"""Intrinsic-backed email.headerregistry subset for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())
_MOLT_EMAIL_ADDRESS_ADDR_SPEC = _require_intrinsic(
    "molt_email_address_addr_spec", globals()
)
_MOLT_EMAIL_ADDRESS_FORMAT = _require_intrinsic("molt_email_address_format", globals())


class Address:
    def __init__(
        self,
        display_name: str = "",
        username: str = "",
        domain: str = "",
        addr_spec: str | None = None,
    ) -> None:
        self.display_name = str(display_name)
        if addr_spec is not None:
            spec = str(addr_spec)
            if "@" in spec:
                user, host = spec.split("@", 1)
                self.username = user
                self.domain = host
            else:
                self.username = spec
                self.domain = ""
        else:
            self.username = str(username)
            self.domain = str(domain)

    @property
    def addr_spec(self) -> str:
        value = _MOLT_EMAIL_ADDRESS_ADDR_SPEC(self.username, self.domain)
        if not isinstance(value, str):
            raise RuntimeError(
                "email.headerregistry addr_spec intrinsic returned invalid"
            )
        return value

    def __str__(self) -> str:
        value = _MOLT_EMAIL_ADDRESS_FORMAT(
            self.display_name, self.username, self.domain
        )
        if not isinstance(value, str):
            raise RuntimeError("email.headerregistry format intrinsic returned invalid")
        return value

    def __repr__(self) -> str:
        return (
            "Address("
            f"display_name={self.display_name!r}, "
            f"username={self.username!r}, "
            f"domain={self.domain!r})"
        )


__all__ = ["Address"]
