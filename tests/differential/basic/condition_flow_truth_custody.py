"""Purpose: value/condition boundaries, identity, and truth callback custody."""

events = []


class Probe:
    def __init__(self, name, *truths):
        self.name = name
        self.truths = truths
        self.calls = 0

    def __bool__(self):
        events.append(self.name)
        if self.calls >= len(self.truths):
            raise ValueError("retested:" + self.name)
        result = self.truths[self.calls]
        self.calls += 1
        return result


class Ordered:
    def __init__(self, name, result):
        self.name = name
        self.result = result

    def __lt__(self, other):
        events.append(self.name + "<" + other.name)
        return self.result


def report(name, result):
    print(name, result.name if isinstance(result, Probe) else result, events)
    events.clear()


a = Probe("a", False, True)
report("and-value", a and Probe("b", True) and Probe("c", True))
a = Probe("a", True, False)
report("or-value", a or Probe("b", False) or Probe("c", False))
a = Probe("a", False)
if a and Probe("b", True) and Probe("c", True):
    report("and-condition", True)
else:
    report("and-condition", False)
a = Probe("a", True)
if a or Probe("b", False) or Probe("c", False):
    report("or-condition", True)
else:
    report("or-condition", False)

a = Probe("a", False, True)
report("nested-value", (a and Probe("b", True)) or Probe("c", True))
a = Probe("a", False, True)
if (a and Probe("b", True)) or Probe("c", True):
    report("nested-condition", True)
else:
    report("nested-condition", False)
a = Probe("a", True, False)
report("opposite-nested-value", (a or Probe("b", True)) and Probe("c", True))
a = Probe("a", False, True)
report("not-condition", not (a and Probe("b", True)))

comparison = Probe("comparison", False, True, False)
first = Ordered("first", comparison)
second = Ordered("second", Probe("second-result", True))
third = Ordered("third", Probe("third-result", True))
last = Ordered("last", Probe("last-result", True))
report("chain-value", first < second < third < last)
print("chain-identity", comparison.calls == 1)
comparison.calls = 0
if first < second < third < last:
    report("chain-condition", True)
else:
    report("chain-condition", False)
comparison.calls = 0
report("nested-chain-value", (first < second < third) and Probe("tail", True))
comparison.calls = 0
if (first < second < third) and Probe("tail", True):
    report("nested-chain-condition", True)
else:
    report("nested-chain-condition", False)

a = Probe("ifexp", False)
report("ifexp-value", 1 if a and Probe("unreached", True) else 2)
a = Probe("ifexp-nested", False, True)
flag = True
report(
    "ifexp-value-boundary",
    (a and Probe("unreached", True) if flag else False) or Probe("tail", False),
)
a = Probe("not-nested", False, True)
report(
    "not-value-boundary", (not (a and Probe("unreached", True))) or Probe("tail", False)
)
a = Probe("ifexp-result", False)
if (a and Probe("unreached", True)) if True else False:
    report("ifexp-condition", True)
else:
    report("ifexp-condition", False)

a = Probe("while", False)
while a and Probe("unreached", True):
    print("unreachable")
report("while-condition", 0)
a = Probe("assert", False)
try:
    assert a and Probe("unreached", True)
except AssertionError:
    report("assert-condition", "raised")
a = Probe("filter", False)
report("filter-condition", [x for x in (1,) if a and Probe("unreached", True)])
a = Probe("sum-filter", False)
report("sum-filter-condition", sum(x for x in (1,) if a and Probe("unreached", True)))
a = Probe("any-filter", False)
report("any-filter-condition", any(x for x in (1,) if a and Probe("unreached", True)))
a = Probe("guard", False)
match 1:
    case _ if a and Probe("unreached", True):
        report("match-condition", True)
    case _:
        report("match-condition", False)

# A builtin call is a real value boundary, not syntactic conditional context.
a = Probe("bool-call", False, True)
report("bool-value-boundary", bool(a and Probe("unreached", True)))
a = Probe("any-elt", False, True)
report("any-value-boundary", any(a and Probe("unreached", True) for x in (1,)))

a = Probe("first-raises")
try:
    if a and Probe("unreached", True):
        print("unreachable")
except ValueError as error:
    report("truth-exception", str(error))


def suspended_and(left):
    result = left and (yield "and-rhs") and (yield "and-tail")
    yield result


def suspended_or(left):
    result = left or (yield "or-rhs") or (yield "or-tail")
    yield result


generator = suspended_and(Probe("async-and-left", True))
report("suspend-and", next(generator))
report("resume-and", generator.send(Probe("async-and-right", False)))
generator.close()


class Finalized:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        events.append("finalize:" + self.name)

    def __lt__(self, other):
        return False


def suspended_merge_lifetime():
    value = None or (yield "merge-input")
    del value
    yield "merge-released"


def suspended_comparison_lifetime():
    result = Finalized("comparison-left") < (yield "comparison-input")
    yield "comparison-released"


def suspended_truthy_operand_lifetime():
    result = Finalized("truthy-operand") and (yield "operand-released")
    yield "operand-resumed"


class FinalizedComparison:
    def __lt__(self, other):
        return Finalized("truthy-comparison")


def suspended_truthy_comparison_lifetime():
    result = FinalizedComparison() < 1 < (yield "truthy-comparison-released")
    yield "truthy-comparison-resumed"


generator = suspended_merge_lifetime()
report("lifetime-merge-start", next(generator))
report("lifetime-merge-end", generator.send(Finalized("merge-result")))
generator.close()
generator = suspended_comparison_lifetime()
report("lifetime-comparison-start", next(generator))
report("lifetime-comparison-end", generator.send(None))
generator.close()
generator = suspended_truthy_operand_lifetime()
report("lifetime-operand-start", next(generator))
report("lifetime-operand-end", generator.send(None))
generator.close()
generator = suspended_truthy_comparison_lifetime()
report("lifetime-truthy-comparison-start", next(generator))
report("lifetime-truthy-comparison-end", generator.send(2))
generator.close()


def conditional_unbound(flag):
    flag and (bound := 1)
    return bound


def alternate_unbound(flag):
    return (bound := 1) if flag else bound


for operation in (conditional_unbound, alternate_unbound):
    for flag in (False, True):
        try:
            report("conditional-binding", operation(flag))
        except UnboundLocalError:
            report("conditional-binding", "unbound")
generator = suspended_or(Probe("async-or-left", False))
report("suspend-or", next(generator))
report("resume-or", generator.send(Probe("async-or-right", True)))
generator.close()


def suspended_comparison(left):
    result = left < (yield "compare-rhs") < (yield "compare-tail")
    yield result


generator = suspended_comparison(Ordered("async-left", Probe("async-cmp", False)))
report("suspend-compare", next(generator))
report("resume-compare", generator.send(Ordered("async-right", None)))
generator.close()
