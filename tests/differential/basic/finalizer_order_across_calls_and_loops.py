"""Purpose: a finalizer runs when the last reference drops inside a callee.

Guards refcount coalescing across calls (HF-214).
"""

class Noisy:
    def __del__(self):
        print("del")


def drop_in_call(holder):
    holder.pop()          # drops the only other reference


def f():
    n = Noisy()
    holder = [n]
    del n                 # the frontend's dec_ref
    print("before")
    drop_in_call(holder)
    print("after")


f()
