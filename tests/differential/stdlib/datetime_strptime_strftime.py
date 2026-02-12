"""Purpose: differential coverage for datetime strptime strftime."""

import datetime


stamp = datetime.datetime(2024, 3, 4, 5, 6, 7)
text = stamp.strftime("%Y-%m-%d %H:%M:%S")
parsed = datetime.datetime.strptime(text, "%Y-%m-%d %H:%M:%S")
print(text)
print(parsed == stamp)
