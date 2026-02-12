"""Purpose: differential coverage for datetime timezone parsing."""

import datetime


try:
    datetime.datetime.fromisoformat("2024-01-02T03:04:05Z")
    print("ok")
except Exception as exc:
    print(type(exc).__name__)

text = "2024-01-02T03:04:05+00:00"
parsed = datetime.datetime.fromisoformat(text)
print(parsed.tzinfo is not None, parsed.utcoffset().total_seconds())
