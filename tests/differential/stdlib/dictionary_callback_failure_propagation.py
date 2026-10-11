"""Dictionary callback errors survive stdlib lookup and mutation boundaries."""

from collections import ChainMap, defaultdict
import csv
from functools import lru_cache
import graphlib
import io
import statistics


failure = ValueError("dictionary callback sentinel")


class Key:
    def __init__(self, fail_hash=False, fail_equal=False):
        self.fail_hash = fail_hash
        self.fail_equal = fail_equal
        self.hash_calls = 0
        self.equal_calls = 0

    def __hash__(self):
        self.hash_calls += 1
        if self.fail_hash:
            raise failure
        return 7

    def __eq__(self, other):
        self.equal_calls += 1
        if self.fail_equal:
            raise failure
        return self is other


def observe(label, operation):
    try:
        operation()
    except ValueError as exc:
        print(label, exc is failure, str(exc))
    else:
        print(label, "lost error")


bad_hash = Key(fail_hash=True)
chain = ChainMap({}, {"later": 1})
observe("chain get hash", lambda: chain[bad_hash])
observe("chain contains hash", lambda: bad_hash in chain)
observe("chain delete hash", lambda: chain.__delitem__(bad_hash))
print("chain unchanged", list(chain.maps[0]), chain["later"])

resident = Key()
collision = Key()
first = {resident: 1}
resident.fail_equal = True
chain = ChainMap(first, {collision: 2})
observe("chain get equality", lambda: chain[collision])
observe("chain contains equality", lambda: collision in chain)
observe("chain delete equality", lambda: chain.__delitem__(collision))
print("chain resident retained", len(first), first[resident])

factory_calls = []


def factory():
    factory_calls.append("called")
    return 99


defaults = defaultdict(factory)
observe("defaultdict hash", lambda: defaults[bad_hash])
print("defaultdict no factory", factory_calls, len(defaults))

for maxsize in (None, 2):
    cache_calls = []

    @lru_cache(maxsize=maxsize)
    def cached(key):
        cache_calls.append("called")
        return "cached value"

    cache_resident = Key()
    cache_collision = Key()
    cached(cache_resident)
    cache_resident.fail_equal = True
    before = cached.cache_info()
    observe("lru hash " + str(maxsize), lambda: cached(bad_hash))
    observe("lru equality " + str(maxsize), lambda: cached(cache_collision))
    print("lru unchanged", maxsize, cache_calls, cached.cache_info() == before)
    cache_resident.fail_equal = False
    print("lru retained", cached(cache_resident), cached.cache_info().hits)
    cached.cache_clear()

observe("mode hash", lambda: statistics.mode([bad_hash]))
observe("multimode hash", lambda: statistics.multimode([bad_hash]))
observe("mode equality", lambda: statistics.mode([resident, collision]))
observe("multimode equality", lambda: statistics.multimode([resident, collision]))

sorter = graphlib.TopologicalSorter()
observe("graph hash", lambda: sorter.add(bad_hash))
sorter.add("good")
print("graph unchanged", list(sorter.static_order()))

for label, fieldnames, line, restkey in [
    ("csv field", [bad_hash, "untouched"], "1,2\n", None),
    ("csv restkey", ["first"], "1,2\n", bad_hash),
    ("csv missing field", ["first", bad_hash], "1\n", None),
]:
    reader = csv.DictReader(io.StringIO(line), fieldnames=fieldnames, restkey=restkey)
    observe(label, lambda: next(reader))
print(
    "csv success",
    list(
        csv.DictReader(
            io.StringIO("1,2,3\n"), fieldnames=["first", "second"], restkey="rest"
        )
    ),
)
