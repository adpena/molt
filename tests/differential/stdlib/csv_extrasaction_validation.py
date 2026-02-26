"""Purpose: verify DictWriter extrasaction validation mirrors CPython."""

import csv
import io


print(csv.DictWriter(io.StringIO(), ["a"], extrasaction="RAISE").extrasaction)
print(csv.DictWriter(io.StringIO(), ["a"], extrasaction="IGNORE").extrasaction)

try:
    csv.DictWriter(io.StringIO(), ["a"], extrasaction="bad")
except Exception as exc:  # noqa: BLE001
    print(type(exc).__name__)
    print(str(exc))

try:
    csv.DictWriter(io.StringIO(), ["a"], extrasaction=1)
except Exception as exc:  # noqa: BLE001
    print(type(exc).__name__)
