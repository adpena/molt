"""Discarded operator results must preserve callbacks, exceptions and loop order.

This is the shared differential capsule for dynamic operator effect barriers.
Returning the operator result would keep it live and fail to test dead-code
elimination. Each action below discards it and returns an unrelated sentinel.
"""


class ArithmeticEffects:
    def __init__(self, raising):
        self.raising = raising

    def effect(self, name):
        if self.raising:
            raise ValueError("boom-" + name)
        print("effect:" + name)
        return 7

    def __add__(self, other):
        return self.effect("add")

    def __sub__(self, other):
        return self.effect("sub")

    def __mul__(self, other):
        return self.effect("mul")

    def __truediv__(self, other):
        return self.effect("truediv")

    def __floordiv__(self, other):
        return self.effect("floordiv")

    def __mod__(self, other):
        return self.effect("mod")

    def __pow__(self, other):
        return self.effect("pow")

    def __and__(self, other):
        return self.effect("and")

    def __or__(self, other):
        return self.effect("or")

    def __xor__(self, other):
        return self.effect("xor")

    def __lshift__(self, other):
        return self.effect("lshift")

    def __rshift__(self, other):
        return self.effect("rshift")

    def __eq__(self, other):
        return self.effect("eq")

    def __lt__(self, other):
        return self.effect("lt")

    def __neg__(self):
        return self.effect("neg")

    def __pos__(self):
        return self.effect("pos")

    def __invert__(self):
        return self.effect("invert")

    def __abs__(self):
        return self.effect("abs")

    def __iadd__(self, other):
        return self.effect("iadd")

    def __isub__(self, other):
        return self.effect("isub")

    def __imul__(self, other):
        return self.effect("imul")

    def __matmul__(self, other):
        return self.effect("matmul")

    def __ne__(self, other):
        return self.effect("ne")

    def __le__(self, other):
        return self.effect("le")

    def __gt__(self, other):
        return self.effect("gt")

    def __ge__(self, other):
        return self.effect("ge")

    def __itruediv__(self, other):
        return self.effect("itruediv")

    def __ifloordiv__(self, other):
        return self.effect("ifloordiv")

    def __imod__(self, other):
        return self.effect("imod")

    def __ipow__(self, other):
        return self.effect("ipow")

    def __iand__(self, other):
        return self.effect("iand")

    def __ior__(self, other):
        return self.effect("ior")

    def __ixor__(self, other):
        return self.effect("ixor")

    def __ilshift__(self, other):
        return self.effect("ilshift")

    def __irshift__(self, other):
        return self.effect("irshift")

    def __imatmul__(self, other):
        return self.effect("imatmul")

    def __radd__(self, other):
        return self.effect("radd")

    def __rsub__(self, other):
        return self.effect("rsub")

    def __rmul__(self, other):
        return self.effect("rmul")

    def __rtruediv__(self, other):
        return self.effect("rtruediv")

    def __rfloordiv__(self, other):
        return self.effect("rfloordiv")

    def __rmod__(self, other):
        return self.effect("rmod")

    def __rpow__(self, other):
        return self.effect("rpow")

    def __rand__(self, other):
        return self.effect("rand")

    def __ror__(self, other):
        return self.effect("ror")

    def __rxor__(self, other):
        return self.effect("rxor")

    def __rlshift__(self, other):
        return self.effect("rlshift")

    def __rrshift__(self, other):
        return self.effect("rrshift")

    def __rmatmul__(self, other):
        return self.effect("rmatmul")


def dead_add(value):
    value + 1
    return "survived"


def dead_sub(value):
    value - 1
    return "survived"


def dead_mul(value):
    value * 2
    return "survived"


def dead_truediv(value):
    value / 2
    return "survived"


def dead_floordiv(value):
    value // 2
    return "survived"


def dead_mod(value):
    value % 2
    return "survived"


def dead_pow(value):
    value**2
    return "survived"


def dead_and(value):
    value & 1
    return "survived"


def dead_or(value):
    value | 1
    return "survived"


def dead_xor(value):
    value ^ 1
    return "survived"


def dead_lshift(value):
    value << 1
    return "survived"


def dead_rshift(value):
    value >> 1
    return "survived"


def dead_eq(value):
    value == 1
    return "survived"


def dead_lt(value):
    value < 1
    return "survived"


def dead_neg(value):
    -value
    return "survived"


def dead_pos(value):
    +value
    return "survived"


def dead_invert(value):
    ~value
    return "survived"


def dead_abs(value):
    abs(value)
    return "survived"


def dead_iadd(value):
    value += 1
    return "survived"


def dead_isub(value):
    value -= 1
    return "survived"


def dead_imul(value):
    value *= 2
    return "survived"


def dead_matmul(value):
    value @ 2
    return "survived"


def dead_ne(value):
    value != 1
    return "survived"


def dead_le(value):
    value <= 1
    return "survived"


def dead_gt(value):
    value > 1
    return "survived"


def dead_ge(value):
    value >= 1
    return "survived"


def dead_itruediv(value):
    value /= 2
    return "survived"


def dead_ifloordiv(value):
    value //= 2
    return "survived"


def dead_imod(value):
    value %= 2
    return "survived"


def dead_ipow(value):
    value **= 2
    return "survived"


def dead_iand(value):
    value &= 1
    return "survived"


def dead_ior(value):
    value |= 1
    return "survived"


def dead_ixor(value):
    value ^= 1
    return "survived"


def dead_ilshift(value):
    value <<= 1
    return "survived"


def dead_irshift(value):
    value >>= 1
    return "survived"


def dead_imatmul(value):
    value @= 2
    return "survived"


def dead_radd(value):
    1 + value
    return "survived"


def dead_rsub(value):
    1 - value
    return "survived"


def dead_rmul(value):
    2 * value
    return "survived"


def dead_rtruediv(value):
    2 / value
    return "survived"


def dead_rfloordiv(value):
    2 // value
    return "survived"


def dead_rmod(value):
    2 % value
    return "survived"


def dead_rpow(value):
    2**value
    return "survived"


def dead_rand(value):
    1 & value
    return "survived"


def dead_ror(value):
    1 | value
    return "survived"


def dead_rxor(value):
    1 ^ value
    return "survived"


def dead_rlshift(value):
    1 << value
    return "survived"


def dead_rrshift(value):
    1 >> value
    return "survived"


def dead_rmatmul(value):
    2 @ value
    return "survived"


def show_exc(label, action, value):
    try:
        result = action(value)
    except BaseException as exc:
        print(label, type(exc).__name__, "|", str(exc))
        return
    print(label, "DID-NOT-RAISE", repr(result))


def primitive_truediv_integer_loop(trips):
    huge = 1 << 2048
    for _ in range(trips):
        huge / 1
    return "survived"


def primitive_truediv_mixed_loop(trips):
    huge = 1 << 2048
    for _ in range(trips):
        huge / 1.0
    return "survived"


def primitive_floordiv_mixed_loop(trips):
    huge = 1 << 2048
    for _ in range(trips):
        huge // 1.0
    return "survived"


def primitive_mod_mixed_loop(trips):
    huge = 1 << 2048
    for _ in range(trips):
        huge % 1.0
    return "survived"


class HeapMutation(ArithmeticEffects):
    def __init__(self, values):
        self.values = values

    def effect(self, name):
        self.values.append(name)
        return 7


def read_across_callbacks(value, values):
    # These reads bracket operators directly, without an intervening function
    # call that could hide a missing operator mutation barrier from CSE.
    before = len(values)
    value + 1
    after_add = len(values)
    value @ 2
    after_matmul = len(values)
    abs(value)
    after_abs = len(values)
    abs(value)
    after_second_abs = len(values)
    value &= 1
    after_inplace = len(values)
    print(
        "heap-reads",
        before,
        after_add,
        after_matmul,
        after_abs,
        after_second_abs,
        after_inplace,
    )


class MutateContainers:
    def __init__(self, sequence, mapping, members):
        self.sequence = sequence
        self.mapping = mapping
        self.members = members

    def __abs__(self):
        self.sequence.append(2)
        self.mapping["b"] = 2
        self.members.add(2)
        return 7


def mutable_constant_aliases():
    values = [1]
    alias = values
    nested = (values,)
    values += [2]
    print("inplace-aliases", len(values), len(alias), len(nested[0]))

    sequence = [1]
    mapping = {"a": 1}
    members = {1}
    captured = (sequence, mapping, members)
    abs(MutateContainers(sequence, mapping, members))
    print("callback-containers", len(sequence), len(mapping), len(members))
    print("nested-containers", len(captured[0]), len(captured[1]), len(captured[2]))
    print("immutable-control", len((1, 2)), len("abc"))
    print("empty-tuple-repeat", len(() * 2147483647))


def main():
    actions = (
        ("add", dead_add),
        ("sub", dead_sub),
        ("mul", dead_mul),
        ("truediv", dead_truediv),
        ("floordiv", dead_floordiv),
        ("mod", dead_mod),
        ("pow", dead_pow),
        ("and", dead_and),
        ("or", dead_or),
        ("xor", dead_xor),
        ("lshift", dead_lshift),
        ("rshift", dead_rshift),
        ("eq", dead_eq),
        ("lt", dead_lt),
        ("neg", dead_neg),
        ("pos", dead_pos),
        ("invert", dead_invert),
        ("abs", dead_abs),
        ("iadd", dead_iadd),
        ("isub", dead_isub),
        ("imul", dead_imul),
        ("matmul", dead_matmul),
        ("ne", dead_ne),
        ("le", dead_le),
        ("gt", dead_gt),
        ("ge", dead_ge),
        ("itruediv", dead_itruediv),
        ("ifloordiv", dead_ifloordiv),
        ("imod", dead_imod),
        ("ipow", dead_ipow),
        ("iand", dead_iand),
        ("ior", dead_ior),
        ("ixor", dead_ixor),
        ("ilshift", dead_ilshift),
        ("irshift", dead_irshift),
        ("imatmul", dead_imatmul),
        ("radd", dead_radd),
        ("rsub", dead_rsub),
        ("rmul", dead_rmul),
        ("rtruediv", dead_rtruediv),
        ("rfloordiv", dead_rfloordiv),
        ("rmod", dead_rmod),
        ("rpow", dead_rpow),
        ("rand", dead_rand),
        ("ror", dead_ror),
        ("rxor", dead_rxor),
        ("rlshift", dead_rlshift),
        ("rrshift", dead_rrshift),
        ("rmatmul", dead_rmatmul),
    )
    raising = ArithmeticEffects(True)
    recording = ArithmeticEffects(False)
    for name, action in actions:
        show_exc("dead:" + name, action, raising)
        # A callback may return normally and still have observable effects.
        action(recording)

    for name, action in (
        ("and", dead_and),
        ("or", dead_or),
        ("xor", dead_xor),
        ("lshift", dead_lshift),
        ("rshift", dead_rshift),
        ("invert", dead_invert),
    ):
        try:
            action("not-an-integer")
        except TypeError:
            print("unsupported:" + name, "TypeError")
        else:
            print("unsupported:" + name, "DID-NOT-RAISE")

    try:
        raising & 1
    except ValueError as exc:
        print("handled:and", str(exc))
    try:
        raising * 3
    except ValueError as exc:
        print("handled:mul", str(exc))
    try:
        abs(raising)
    except ValueError as exc:
        print("handled:abs", str(exc))

    for _ in range(0):
        raising & 1
    print("zero_trip", "no-spurious-raise")
    for _ in range(2):
        recording & 1
    print("two_trip", "two-callbacks")

    values = []
    read_across_callbacks(HeapMutation(values), values)
    print("heap-callbacks", values)
    mutable_constant_aliases()

    print("zero-trip:truediv_integer", primitive_truediv_integer_loop(0))
    show_exc("one-trip:truediv_integer", primitive_truediv_integer_loop, 1)
    print("zero-trip:truediv_mixed", primitive_truediv_mixed_loop(0))
    show_exc("one-trip:truediv_mixed", primitive_truediv_mixed_loop, 1)
    print("zero-trip:floordiv_mixed", primitive_floordiv_mixed_loop(0))
    show_exc("one-trip:floordiv_mixed", primitive_floordiv_mixed_loop, 1)
    print("zero-trip:mod_mixed", primitive_mod_mixed_loop(0))
    show_exc("one-trip:mod_mixed", primitive_mod_mixed_loop, 1)

    print("safe:add", 7 + 3)
    print("safe:sub", 10 - 4)
    print("safe:mul", 6 * 7)
    print("safe:eq", 7 == 3)
    print("safe:lt", 5 < 9)
    print("safe:neg", -5)
    print("safe:bitand", 6 & 3)
    print("safe:bitor", 4 | 1)


def typed_literal_join(flag):
    import math

    value = 1 if flag else True
    floating = 1 if flag else 1.0
    zero = 0.0 if flag else -0.0
    print(
        "literal-join",
        flag,
        type(value).__name__,
        value,
        type(floating).__name__,
        floating,
        math.copysign(1.0, zero),
    )


if __name__ == "__main__":
    main()
    for literal_flag in (True, False):
        typed_literal_join(literal_flag)
