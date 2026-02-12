"""Purpose: differential coverage for datetime timezone arithmetic."""

import datetime


tz = datetime.timezone(datetime.timedelta(hours=5, minutes=30))
local = datetime.datetime(2024, 1, 2, 3, 4, 5, tzinfo=tz)
utc = local.astimezone(datetime.timezone.utc)
print(utc.isoformat())

roundtrip = utc.astimezone(tz)
print(roundtrip.isoformat())
