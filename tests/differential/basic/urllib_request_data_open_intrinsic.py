"""Purpose: differential coverage for intrinsic-backed urllib.request opener core."""

import urllib.error
import urllib.request

with urllib.request.urlopen("data:,hello%20world") as resp:
    print(resp.read())

opener = urllib.request.build_opener()
with opener.open(urllib.request.Request("data:text/plain;base64,aGk=")) as resp:
    print(resp.read())

try:
    urllib.request.urlopen("custom-scheme://example")
except urllib.error.URLError as exc:
    print(type(exc).__name__)
    print(str(exc))
