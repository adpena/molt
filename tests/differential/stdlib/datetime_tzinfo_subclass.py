"""Purpose: differential coverage for datetime tzinfo subclass."""

import datetime


class FixedTZ(datetime.tzinfo):
    def __init__(self, offset_hours: int) -> None:
        self._offset = datetime.timedelta(hours=offset_hours)

    def utcoffset(self, dt):
        return self._offset

    def dst(self, dt):
        return datetime.timedelta(0)

    def tzname(self, dt):
        return f"UTC{self._offset}"


tz = FixedTZ(3)
dt = datetime.datetime(2024, 1, 1, 12, 0, tzinfo=tz)
print(dt.utcoffset().total_seconds())
print(dt.tzname())
