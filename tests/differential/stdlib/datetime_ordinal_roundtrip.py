"""Purpose: ordinal/date conversion parity for canonical epoch anchors."""

import datetime as dt


print(dt.date.fromordinal(1).isoformat())
print(dt.date.fromordinal(719163).isoformat())
print(dt.date(1970, 1, 1).toordinal())
