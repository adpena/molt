"""Purpose: differential coverage for datetime parse arithmetic."""

import datetime


iso = "2024-01-02T03:04:05+02:00"
dt = datetime.datetime.fromisoformat(iso)
print(dt.tzinfo is not None, dt.utcoffset().total_seconds())

utc = dt.astimezone(datetime.timezone.utc)
print(utc.isoformat())

start = datetime.datetime(2024, 1, 1, 0, 0, 0)
end = datetime.datetime(2024, 1, 2, 1, 2, 3)

delta = end - start
print(delta.days, delta.seconds)

stamp = datetime.datetime(2024, 1, 2, 3, 4, 5, 123456)
print(stamp.isoformat(timespec="milliseconds"))
