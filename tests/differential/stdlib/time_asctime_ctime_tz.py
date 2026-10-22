"""Purpose: differential coverage for time.asctime/ctime/tzname/timezone basics."""

import time

print(isinstance(time.timezone, int))
print(isinstance(time.tzname, tuple), len(time.tzname))
print(all(isinstance(item, str) for item in time.tzname))
print(time.asctime(time.gmtime(0)))
print(len(time.ctime(0)))

# The hidden fields belong to the exact provider type and survive every public
# consumer. UTC makes the expected civil fields independent of local timezone.
import sys

base = (2024, 1, 2, 3, 4, 5, 1, 2, 0)
zone = time.struct_time(base, {"tm_zone": "TEST", "tm_gmtoff": 19800})
print("hidden", len(zone), tuple(zone) == base, zone.tm_zone, zone.tm_gmtoff)
print("zone-format", time.strftime("%Z|%z", zone))
print("asctime-hidden", time.asctime(zone) == time.asctime(base))
print("utc-hidden", time.gmtime(0).tm_zone, time.gmtime(0).tm_gmtoff)

for offset in (-12600, 0, 20700):
    item = time.struct_time(base, {"tm_zone": "OFFSET", "tm_gmtoff": offset})
    print("offset", offset, time.strftime("%Z|%z", item))

# Required index conversion precedes hidden-zone validation. A retained error
# distinguishes original callback identity from a newly synthesized exception.
visits = []
problem = RuntimeError("time conversion sentinel")


class Index:
    def __init__(self, label, value):
        self.label, self.value = label, value

    def __index__(self):
        visits.append(self.label)
        if self.value is problem:
            raise problem
        return self.value


for operation in (lambda value: time.strftime("%Y", value), time.asctime, time.mktime):
    visits.clear()
    indexed = tuple(Index(index, value) for index, value in enumerate(base))
    item = time.struct_time(
        indexed, {"tm_zone": "TEST", "tm_gmtoff": Index("offset", 0)}
    )
    operation(item)
    print("conversion-order", visits)
    visits.clear()
    broken = (Index("year", problem),) + base[1:]
    try:
        operation(
            time.struct_time(
                broken, {"tm_zone": object(), "tm_gmtoff": Index("offset", 0)}
            )
        )
    except RuntimeError as error:
        print("index-error", error is problem, visits)
    visits.clear()
    try:
        operation(
            time.struct_time(
                base, {"tm_zone": object(), "tm_gmtoff": Index("offset", 0)}
            )
        )
    except TypeError:
        print("zone-before-offset", visits)
    # The zone's conversion error is visible even to asctime/mktime, which do
    # not themselves display the zone. Windows has no native tm_zone fields.
    if sys.platform != "win32":
        try:
            operation(
                time.struct_time(
                    base, {"tm_zone": "\ud800", "tm_gmtoff": Index("offset", 0)}
                )
            )
        except UnicodeEncodeError:
            print("zone-unicode", visits)


# Exact tuple admission cannot invoke a foreign iterable or tuple-subclass len.
class PoisonTuple(tuple):
    def __len__(self):
        raise AssertionError("public len must not run")

    def __iter__(self):
        raise AssertionError("public iteration must not run")


print("physical-tuple", time.asctime(PoisonTuple(base)) == time.asctime(base))
for operation in (lambda value: time.strftime("%Y", value), time.asctime, time.mktime):
    try:
        operation(list(base))
    except TypeError:
        print("tuple-required", True)
