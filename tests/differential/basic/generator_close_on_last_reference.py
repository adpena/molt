"""Purpose: a suspended generator closes when its last reference dies.

CPython finalizes a started, unfinished generator by calling close(), so its
finally block runs at that point. A generator that never started runs nothing,
and one that yields again from finally reports RuntimeError as unraisable.
"""

import gc
import sys


def items(log, tag):
    try:
        yield 1
        yield "x"
    finally:
        log.append("closed " + tag)


log = []
g = items(log, "plain")
next(g)
log.append("before del")
del g
log.append("after del")
print(log)

log = []
g = items(log, "sum")
try:
    sum(g)
except TypeError:
    log.append("caught")
log.append("before del")
del g
log.append("after del")
print(log)

log = []
g = items(log, "unstarted")
del g
print("unstarted", log)

log = []
g = items(log, "finished")
list(g)
log.append("exhausted")
del g
print("finished", log)


def stubborn(log):
    try:
        yield 1
    finally:
        log.append("finally")
        yield 2


unraisable = []
sys.unraisablehook = lambda report: unraisable.append(type(report.exc_value).__name__)
log = []
g = stubborn(log)
next(g)
del g
print("stubborn", log, unraisable)
sys.unraisablehook = sys.__unraisablehook__


def cyclic(log):
    holder = []
    try:
        holder.append((yield 1))
    finally:
        log.append("cycle closed")


log = []
g = cyclic(log)
next(g)
box = [g]
box.append(box)
del g, box
gc.collect()
print("cycle", log)


# A generator whose frame already finished is terminal: collecting it later,
# even after the collector clears it, must not run its body a second time.
def finishes_by_throw():
    try:
        yield 1
    finally:
        print("throw finally")


def finishes_by_close():
    try:
        yield 1
    finally:
        print("close finally")


def finished_generators():
    thrown = finishes_by_throw()
    next(thrown)
    try:
        thrown.throw(IndexError("ix"))
    except IndexError as exc:
        print("throw escaped", exc)
    closed = finishes_by_close()
    next(closed)
    closed.close()
    box = [thrown, closed]
    box.append(box)


finished_generators()
gc.collect()
print("finished generators collected")
