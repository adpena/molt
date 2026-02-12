"""Purpose: differential coverage for datetime ISO round-trips and tz offsets."""

from datetime import datetime, date, time, timezone, timedelta


dt = datetime(2024, 1, 2, 3, 4, 5, 123456, tzinfo=timezone.utc)
text = dt.isoformat()
print("dt", text)
print("dt_roundtrip", datetime.fromisoformat(text))

offset = timezone(timedelta(hours=-5, minutes=-30))
dt2 = datetime(2024, 6, 1, 0, 0, tzinfo=offset)
print("dt2", dt2.isoformat())
print("dt2_roundtrip", datetime.fromisoformat(dt2.isoformat()))

print("date", date(2024, 1, 2).isoformat())
print("time", time(3, 4, 5, 123456).isoformat())
