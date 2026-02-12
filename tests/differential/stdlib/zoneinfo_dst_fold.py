"""Purpose: differential coverage for PEP 615 DST fold handling."""

from datetime import datetime
import zoneinfo


try:
    tz = zoneinfo.ZoneInfo("America/New_York")
except zoneinfo.ZoneInfoNotFoundError as exc:
    print(type(exc).__name__, exc)
else:
    first = datetime(2023, 11, 5, 1, 30, tzinfo=tz, fold=0)
    second = datetime(2023, 11, 5, 1, 30, tzinfo=tz, fold=1)
    print(first.utcoffset(), second.utcoffset(), first.fold, second.fold)
