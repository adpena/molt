"""Purpose: urllib.response handle-backed readinto lowers through runtime intrinsic."""

import urllib.request


with urllib.request.urlopen("data:text/plain,abcdef") as resp:
    out = bytearray(b"xxxx")
    print("n1", resp.readinto(out), bytes(out).decode("ascii"))
    print("n2", resp.readinto(out), bytes(out).decode("ascii"))
    print("n3", resp.readinto(out), bytes(out).decode("ascii"))

with urllib.request.urlopen("data:text/plain,abcdef") as resp:
    view = memoryview(bytearray(3))
    print("mv", resp.readinto(view), bytes(view).decode("ascii"))

with urllib.request.urlopen("data:text/plain,abcdef") as resp:
    try:
        resp.readinto(b"abcd")
    except Exception as exc:  # noqa: BLE001
        print("readonly", type(exc).__name__)
