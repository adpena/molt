"""Purpose: urllib.response stream method surface lowers via Rust intrinsics."""

import urllib.request


with urllib.request.urlopen("data:text/plain,abc%0Adef") as resp:
    print("caps", resp.readable(), resp.writable(), resp.seekable())
    print("tell0", resp.tell())
    print("read1", resp.read1(2).decode("ascii"))
    out = bytearray(2)
    print("readinto1", resp.readinto1(out), bytes(out).decode("ascii"))
    print("tell1", resp.tell())
    print("seek0", resp.seek(0))
    print("tell2", resp.tell())
    print("line", resp.readline().decode("ascii").rstrip())
    print("lines", [line.decode("ascii") for line in resp.readlines()])

closed = urllib.request.urlopen("data:text/plain,abc")
closed.close()
for name, op in (
    ("read", lambda: closed.read()),
    ("readline", lambda: closed.readline()),
    ("readlines", lambda: closed.readlines()),
    ("readinto", lambda: closed.readinto(bytearray(2))),
    ("read1", lambda: closed.read1()),
    ("readinto1", lambda: closed.readinto1(bytearray(2))),
    ("readable", lambda: closed.readable()),
    ("writable", lambda: closed.writable()),
    ("seekable", lambda: closed.seekable()),
    ("tell", lambda: closed.tell()),
    ("seek", lambda: closed.seek(0)),
):
    try:
        value = op()
        print("closed", name, "ok", value)
    except Exception as exc:  # noqa: BLE001
        print("closed", name, type(exc).__name__)
