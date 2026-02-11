"""Minimal smtplib support for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import socket
from typing import Any

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)
_MOLT_IMPORT_SMOKE_RUNTIME_READY()

# TODO(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial):
# lower SMTP client transport and protocol handling into Rust intrinsics and add
# STARTTLS/auth/LMTP parity.

__all__ = [
    "SMTP",
    "SMTPException",
    "SMTPServerDisconnected",
    "SMTPResponseException",
]


class SMTPException(Exception):
    pass


class SMTPServerDisconnected(SMTPException):
    pass


class SMTPResponseException(SMTPException):
    def __init__(self, smtp_code: int, smtp_error: bytes | str):
        super().__init__(smtp_code, smtp_error)
        self.smtp_code = smtp_code
        self.smtp_error = smtp_error


def _line_to_code(line: bytes) -> tuple[int, bytes]:
    if len(line) < 3:
        raise SMTPServerDisconnected("short SMTP reply")
    try:
        code = int(line[:3].decode("ascii", "strict"))
    except Exception as exc:  # noqa: BLE001
        raise SMTPServerDisconnected("invalid SMTP reply code") from exc
    return code, line[4:] if len(line) > 4 else b""


class SMTP:
    def __init__(
        self,
        host: str = "",
        port: int = 0,
        local_hostname: str | None = None,
        timeout: float | None = None,
        source_address: tuple[str, int] | None = None,
    ) -> None:
        self.timeout = timeout
        self.local_hostname = local_hostname or "localhost"
        self.source_address = source_address
        self.sock: Any | None = None
        self.file: Any | None = None
        if host:
            self.connect(host, port)

    def connect(self, host: str = "localhost", port: int = 0):
        if port == 0:
            port = 25
        self.close()
        self.sock = socket.create_connection(
            (host, port), self.timeout, source_address=self.source_address
        )
        self.file = self.sock.makefile("rb")
        return self._getreply()

    def close(self) -> None:
        if self.file is not None:
            try:
                self.file.close()
            finally:
                self.file = None
        if self.sock is not None:
            try:
                self.sock.close()
            finally:
                self.sock = None

    def _check_connected(self) -> None:
        if self.sock is None or self.file is None:
            raise SMTPServerDisconnected("please run connect() first")

    def _getreply(self) -> tuple[int, bytes]:
        self._check_connected()
        assert self.file is not None
        code = -1
        parts: list[bytes] = []
        while True:
            line = self.file.readline()
            if not line:
                raise SMTPServerDisconnected("connection unexpectedly closed")
            line = line.rstrip(b"\r\n")
            code, text = _line_to_code(line)
            parts.append(text)
            if len(line) < 4 or line[3:4] != b"-":
                break
        return code, b"\n".join(parts)

    def _putcmd(self, cmd: str, args: str | None = None) -> None:
        self._check_connected()
        assert self.sock is not None
        line = cmd if args is None else f"{cmd} {args}"
        self.sock.sendall(line.encode("ascii", "surrogateescape") + b"\r\n")

    def helo(self, name: str | None = None) -> tuple[int, bytes]:
        self._putcmd("HELO", name or self.local_hostname)
        return self._getreply()

    def ehlo(self, name: str | None = None) -> tuple[int, bytes]:
        self._putcmd("EHLO", name or self.local_hostname)
        return self._getreply()

    def mail(self, sender: str) -> tuple[int, bytes]:
        self._putcmd("MAIL FROM:", f"<{sender}>")
        return self._getreply()

    def rcpt(self, recip: str) -> tuple[int, bytes]:
        self._putcmd("RCPT TO:", f"<{recip}>")
        return self._getreply()

    def data(self, msg: str | bytes) -> tuple[int, bytes]:
        self._putcmd("DATA")
        code, resp = self._getreply()
        if code != 354:
            return code, resp
        self._check_connected()
        assert self.sock is not None

        if isinstance(msg, str):
            payload = msg.encode("utf-8")
        else:
            payload = bytes(msg)
        payload = payload.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
        lines = payload.split(b"\n")
        stuffed = b"\r\n".join(
            (b"." + line) if line.startswith(b".") else line for line in lines
        )
        self.sock.sendall(stuffed + b"\r\n.\r\n")
        return self._getreply()

    def noop(self) -> tuple[int, bytes]:
        self._putcmd("NOOP")
        return self._getreply()

    def quit(self) -> tuple[int, bytes]:
        try:
            self._putcmd("QUIT")
            return self._getreply()
        finally:
            self.close()

    def __enter__(self) -> "SMTP":
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        try:
            self.quit()
        except Exception:  # noqa: BLE001
            self.close()
