"""Purpose: differential coverage for datetime edge cases."""

import datetime


try:
    datetime.datetime.fromisoformat("2024-13-01")
except Exception as exc:
    print(type(exc).__name__)

naive = datetime.datetime(2024, 1, 1, 0, 0, 0)
aware = datetime.datetime(2024, 1, 1, 0, 0, 0, tzinfo=datetime.timezone.utc)
try:
    _ = naive < aware
except Exception as exc:
    print(type(exc).__name__)

print(datetime.timedelta(days=1, seconds=1).total_seconds())

stamp = datetime.datetime(2024, 1, 1, 0, 0, 0, fold=1)
print(stamp.fold)
