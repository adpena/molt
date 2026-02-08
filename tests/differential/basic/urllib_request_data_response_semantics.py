"""Purpose: intrinsic parity for urllib.request data: response metadata semantics."""

import urllib.request

with urllib.request.urlopen("data:text/plain,hello%20world") as resp:
    print(resp.getcode())
    print(resp.status)
    print(resp.geturl())
    print(resp.read(5))
    print(resp.read())
