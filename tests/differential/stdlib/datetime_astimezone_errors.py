"""Purpose: differential coverage for datetime astimezone errors."""

import datetime


naive = datetime.datetime(2024, 1, 1, 0, 0, 0)
try:
    naive.astimezone(datetime.timezone.utc)
    print("ok")
except Exception as exc:
    print(type(exc).__name__)

try:
    datetime.datetime(2024, 1, 1, tzinfo="bad")
except Exception as exc:
    print(type(exc).__name__)
