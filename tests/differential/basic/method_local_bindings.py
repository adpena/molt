"""Method and lambda locals keep CPython's binding semantics.

A method's local lives in its frame, as a plain function's does; only a
local that an inner scope captures needs a cell. Reads before a store, `del`,
late-bound closures, `nonlocal`, `locals()`, exception names and recursion
must behave exactly as in CPython.
"""


class Probe:
    def __init__(self, seed):
        self.seed = seed

    def unbound_read(self, flag):
        if flag:
            value = self.seed
        try:
            return value
        except UnboundLocalError as exc:
            return f"unbound: {exc}"

    def deleted(self):
        value = self.seed
        del value
        try:
            return value
        except UnboundLocalError as exc:
            return f"deleted: {exc}"

    def loop_carried(self, n):
        total = 0
        last = None
        for i in range(n):
            if i % 2:
                last = i
            total = total + i
        return total, last

    def late_closure(self):
        value = 1
        read = lambda: value
        value = 2
        first = read()
        value = 3
        return first, read()

    def nonlocal_writer(self):
        count = 0

        def bump():
            nonlocal count
            count += 10

        bump()
        bump()
        return count

    def frame_locals(self, extra):
        alpha = self.seed
        beta = alpha + extra
        return sorted(locals().items(), key=lambda item: item[0] != "self")[1:]

    def exception_name(self):
        try:
            raise ValueError("boom")
        except ValueError as err:
            message = str(err)
        try:
            err
        except NameError as exc:
            return message, type(exc).__name__
        return message, "still bound"

    def finally_sees(self, fail):
        state = "start"
        try:
            state = "body"
            if fail:
                raise KeyError("k")
            state = "done"
        except KeyError:
            state = state + "+handled"
        finally:
            state = state + "+finally"
        return state

    def recurse(self, depth):
        local = depth * 2
        if depth == 0:
            return [local]
        inner = self.recurse(depth - 1)
        return inner + [local]

    def generator_reads(self, scale):
        factor = scale
        return list(x * factor for x in range(3))

    @staticmethod
    def static_locals(a, b):
        low = min(a, b)
        high = max(a, b)
        return high - low

    @classmethod
    def class_locals(cls, text):
        name = cls.__name__
        joined = name + ":" + text
        return joined


class Child(Probe):
    def summed(self, depth):
        result = super().recurse(depth)
        tail = result[-1]
        return result, tail


probe = Probe(5)
print(probe.unbound_read(True))
print(probe.unbound_read(False))
print(probe.deleted())
print(probe.loop_carried(7))
print(probe.late_closure())
print(probe.nonlocal_writer())
print(probe.frame_locals(3))
print(probe.exception_name())
print(probe.finally_sees(False), probe.finally_sees(True))
print(probe.recurse(3))
print(probe.generator_reads(4))
print(Probe.static_locals(9, 2), Probe.class_locals("x"))
print(Child(1).summed(2))

walrus = lambda: (y := 5) + y
print(walrus())
print((lambda a, b=4: (c := a * b) - c // 2)(3))
