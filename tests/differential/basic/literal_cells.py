# MOLT_ENV: MOLT_CAPABILITIES=thread.spawn,thread.shared
"""Literals keep CPython's observable identity after their first evaluation.

A compiled str, bytes or big-int literal is built once, on its first
evaluation, and loaded from its cell afterwards (call_anatomy.md 4.9). Every
evaluation must still yield an equal object; identifier-like strings must stay
the interned singletons CPython makes them; and a literal first reached in a
cold branch, a generator, a loop or another thread must behave the same.
"""

import sys
import threading


def name_literal():
    return "attribute_name"


def spaced_literal():
    return "two words"


def bytes_literal():
    return b"\x00\xffpayload"


def big_literal():
    return 123456789012345678901234567890


# Identifier-like literals are interned: equal literals are one object, and
# sys.intern returns that same object.
print("name is name", name_literal() is name_literal())
print("intern", sys.intern(name_literal()) is name_literal())
same_name = "attribute_name"
print("cross-function", name_literal() is same_name)

# Every evaluation yields an equal object of the right type and value.
values = [spaced_literal() for _ in range(5)]
print("spaced", all(value == "two words" for value in values), type(values[0]).__name__)
print("spaced same object", all(value is values[0] for value in values))
blobs = [bytes_literal() for _ in range(3)]
print("bytes", blobs[0], all(blob == blobs[0] for blob in blobs))
bigs = [big_literal() for _ in range(3)]
print("big", bigs[0], all(big == bigs[0] for big in bigs), bigs[0] + 1)


def cold(flag):
    if flag:
        return "first reached in a cold branch"
    return None


print("cold none", cold(False))
print("cold", cold(True), cold(True) is cold(True))


def generator():
    for index in range(3):
        yield f"{index}:" + "suffix literal"


print("generator", list(generator()))

results = []


def worker():
    results.append(spaced_literal())


threads = [threading.Thread(target=worker) for _ in range(8)]
for thread in threads:
    thread.start()
for thread in threads:
    thread.join()
print("threads", len(results), all(value == "two words" for value in results))
print("threads same object", all(value is results[0] for value in results))

# Mutating a value built from a literal never changes the literal.
built = bytearray(bytes_literal())
built[0] = 7
print("literal unchanged", bytes_literal(), bytes(built)[:1])
empty = ""
print("empty", repr(empty), empty is str(), b"" == bytes_literal()[:0])
