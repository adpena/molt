# P0 silent-integer-truncation differential for datetime's `_as_int` component
# coercion (molt_datetime_as_int), part of the `MoltObject::from_int` inline-
# window class. `_as_int` boxed a `to_i64(...)` result through `from_int`, which
# masks mod 2**47. A year of 2**47 + 2000 masks to 2000 — so the OLD code
# silently built a VALID datetime(year=2000) where CPython raises (year out of
# range): a silent wrong answer. The fix routes the value through the full-range
# `int_bits_from_i64` (and a BigInt fallback), preserving the true magnitude so
# the range check fires.
#
# Expected CPython 3.12 output (verified on .venv/Scripts/python.exe):
#   year_probe rejected
#   td_us 11574 days, 1:46:40
#   td_s 12725829 days, 0:36:16
#   td_days_big 1000000 days, 0:00:00
#   hash_dt_type int
#   hash_td_type int
from datetime import datetime, timedelta


def probe_year():
    # 2**47 + 2000 masks (mod 2**47) to 2000; the old truncation silently built
    # datetime(year=2000). CPython rejects the out-of-range year — molt must too.
    try:
        return datetime(2**47 + 2000, 6, 15).year
    except (ValueError, OverflowError):
        return "rejected"


print("year_probe", probe_year())

# timedelta large-argument normalization also flows through component coercion;
# a truncated microseconds/seconds arg would normalize to the wrong delta.
print("td_us", timedelta(microseconds=10**15))
print("td_s", timedelta(seconds=2**40))
print("td_days_big", timedelta(days=10**6))

# __hash__ must yield a proper int (full-range, never masked). Molt's datetime
# hash *value* is not CPython-parity (different algorithm), so only the type is
# asserted here; the fix's job is to not truncate the hash.
print("hash_dt_type", type(hash(datetime(2000, 1, 1))).__name__)
print("hash_td_type", type(hash(timedelta(days=10**6))).__name__)
