# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,select.select
"""Purpose: differential coverage for select timeout edge cases."""

import select

ready = select.select([], [], [], 0)
print([len(item) for item in ready])

try:
    select.select([], [], [], -1)
except Exception as exc:
    print(type(exc).__name__)

# Non-sequence iterables must use the generic collector even without OS watches.
assert select.select(iter(()), iter(()), iter(()), 0) == ([], [], [])

import gc
import weakref


class CollectionFailure(RuntimeError):
    pass


failure = CollectionFailure("select collection failed")
references = []


class Watch:
    def fileno(self):
        raise AssertionError("watch conversion preceded input collection")


class Watches:
    def __init__(self, fail):
        self.fail = fail
        self.position = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.position:
            if self.fail:
                raise failure
            raise StopIteration
        self.position += 1
        value = Watch()
        references.append(weakref.ref(value))
        return value


try:
    select.select(Watches(True), [], [], 0)
except CollectionFailure as caught:
    assert caught is failure
else:
    raise AssertionError("select swallowed iterable failure")
gc.collect()
assert len(references) == 1 and all(ref() is None for ref in references)
print("generic collection", len(references), all(ref() is None for ref in references))
