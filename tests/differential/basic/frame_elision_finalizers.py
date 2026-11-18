"""Purpose: a frameless callee releases an object parameter as CPython does.

CPython clears a returning frame's locals after the caller is the current
frame again, so a finalizer that the release runs sees the caller on top. A
callee whose body cannot raise and runs no Python code drops its frame record;
its one object parameter is then released at its last use, which no program
can tell apart from the return. Two object parameters keep their release
order.
"""

import sys


class Noisy:
    def __init__(self, tag):
        self.tag = tag

    def __del__(self):
        print("del", self.tag, sys._getframe(1).f_code.co_name)


def leaf(obj, n):
    return n + 1


def two(first, second, n):
    return n * 2


class Box:
    def __init__(self, k):
        self.k = k

    def bump(self, x):
        return (x + x) & 1023


def plain_leaf():
    r = leaf(Noisy("leaf"), 3)
    print("after leaf", r)


def ordered_pair():
    r = two(Noisy("first"), Noisy("second"), 4)
    print("after two", r)


def method_temporary():
    total = 0
    for i in range(3):
        total += Box(i).bump(i)
    print("method", total)


def method_in_loop():
    box = Box(7)
    total = 0
    for i in range(5):
        total = (total + box.bump(i)) & 4095
    print("loop", total, box.k)


plain_leaf()
ordered_pair()
method_temporary()
method_in_loop()
