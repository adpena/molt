"""Purpose: differential coverage for HTTP upgrade header formatting."""

import email.message
import http.client

_KEY = "dGhlIHNhbXBsZSBub25jZQ=="

msg = email.message.EmailMessage()
msg["Upgrade"] = "websocket"
msg["Connection"] = "Upgrade"
msg["Sec-WebSocket-Key"] = _KEY
msg["Sec-WebSocket-Version"] = "13"

conn = http.client.HTTPConnection("example.com", 80)
conn.putrequest("GET", "/", skip_accept_encoding=True)
for name, value in msg.items():
    conn.putheader(name, value)

raw = b"".join(conn._buffer)
lines = [line for line in raw.decode("ascii", "surrogateescape").split("\r\n") if line]


def _has_line(target: str) -> bool:
    lower = target.lower()
    return any(line.lower() == lower for line in lines)


def _has_prefix(prefix: str) -> bool:
    lower = prefix.lower()
    return any(line.lower().startswith(lower) for line in lines)


print("host", _has_line("Host: example.com"))
print("upgrade", _has_line("Upgrade: websocket"))
print("connection", _has_line("Connection: Upgrade"))
print("sec_key", _has_prefix("Sec-WebSocket-Key:"))
print("sec_ver", _has_line("Sec-WebSocket-Version: 13"))
print("line_count", len(lines))
