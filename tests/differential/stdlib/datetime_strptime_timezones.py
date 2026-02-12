"""Purpose: differential coverage for datetime strptime timezones."""

import datetime


text = "2024-01-02 03:04:05 +0200"
parsed = datetime.datetime.strptime(text, "%Y-%m-%d %H:%M:%S %z")
print(parsed.utcoffset().total_seconds())

try:
    datetime.datetime.strptime("2024-01-02 03:04:05 PST", "%Y-%m-%d %H:%M:%S %Z")
    print("ok")
except Exception as exc:
    print(type(exc).__name__)
