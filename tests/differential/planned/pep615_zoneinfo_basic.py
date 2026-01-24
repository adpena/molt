"""Purpose: differential coverage for PEP 615 zoneinfo basics."""

import zoneinfo


tz = zoneinfo.ZoneInfo("UTC")
print(tz.key)
print(tz.utcoffset(None))
print(zoneinfo.ZoneInfo("UTC") == tz)
