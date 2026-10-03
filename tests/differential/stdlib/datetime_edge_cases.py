"""Purpose: differential coverage for datetime edge cases."""

import datetime


try:
    datetime.datetime.fromisoformat("2024-13-01")
except Exception as exc:
    print(type(exc).__name__)

naive = datetime.datetime(2024, 1, 1, 0, 0, 0)
aware = datetime.datetime(2024, 1, 1, 0, 0, 0, tzinfo=datetime.timezone.utc)
try:
    _ = naive < aware
except Exception as exc:
    print(type(exc).__name__)

print(datetime.timedelta(days=1, seconds=1).total_seconds())

stamp = datetime.datetime(2024, 1, 1, 0, 0, 0, fold=1)
print(stamp.fold)


# Hash transport follows the target signed width for every datetime consumer.
import sys

hash_low = -(2 ** (sys.hash_info.width - 1))
hash_high = 2 ** (sys.hash_info.width - 1)
hash_values = [
    datetime.date(9999, 12, 31),
    datetime.time(0, 0, tzinfo=datetime.timezone(datetime.timedelta(hours=23))),
    datetime.datetime(9999, 12, 31, 23, 59, 59, 999999),
    datetime.timedelta(days=-999999999, microseconds=1),
    datetime.timedelta(days=999999999, seconds=86399, microseconds=999999),
]
print("datetime hash width:", all(hash_low <= hash(value) < hash_high and hash(value) != -1 for value in hash_values))
first = datetime.datetime(2024, 1, 2, tzinfo=datetime.timezone.utc)
second = datetime.datetime(2024, 1, 2, 1, tzinfo=datetime.timezone(datetime.timedelta(hours=1)))
print("datetime equal hash:", first == second, hash(first) == hash(second))
