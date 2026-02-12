"""Purpose: differential coverage for datetime basic."""

import datetime


naive = datetime.datetime(2024, 1, 2, 3, 4, 5)
aware = datetime.datetime(2024, 1, 2, 3, 4, 5, tzinfo=datetime.timezone.utc)

print(naive.isoformat())
print(aware.isoformat())
print(aware.astimezone(datetime.timezone.utc).isoformat())
print(datetime.date(2024, 1, 2).isoformat())
print(datetime.time(3, 4, 5).isoformat())
