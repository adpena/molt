"""Purpose: differential coverage for datetime.timestamp parity."""

import datetime


aware = datetime.datetime(2024, 1, 2, 3, 4, 5, 678901, tzinfo=datetime.timezone.utc)
naive = datetime.datetime(2024, 1, 2, 3, 4, 5, 678901)

print(round(aware.timestamp(), 6))
print(isinstance(naive.timestamp(), float))
