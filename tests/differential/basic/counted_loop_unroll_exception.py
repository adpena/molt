# Exception soundness for the unrolled counted loop (L4 producer):
#  * A `try:` block INSIDE the loop body must keep unrolling REFUSED
#    (has_exception_handlers() == true), and the program must still run and
#    catch correctly.
#  * A bare raise from a constant-trip loop body, with no handler in the loop,
#    must propagate to the function-exit path identically whether or not the
#    loop was unrolled — every unrolled clone of the body's CheckException op
#    points at the same fn-exit handler.


def try_in_body(items):
    # try/except inside the loop body — unroll must NOT fire here.
    total = 0
    for i in range(4):
        try:
            total += items[i]
        except IndexError:
            total += 100
    return total


def raising_range(n, bad):
    # No handler in the loop; the i == bad iteration raises and propagates.
    total = 0
    for i in range(n):
        if i == bad:
            raise ValueError(f"bad {i}")
        total += i
    return total


print(try_in_body([1, 2, 3, 4]))
print(try_in_body([10]))

try:
    raising_range(4, 2)
    print("no raise")
except ValueError as e:
    print("caught:", e)

print(raising_range(4, 99))


def gen_then_loop():
    # A generator consumed by a counted loop in the same function — the loop is
    # NOT a counted-range loop (iterates the generator), and the function has a
    # generator state region, so unrolling is correctly skipped.
    def squares(n):
        for k in range(n):
            yield k * k

    out = 0
    for v in squares(5):
        out += v
    return out


print(gen_then_loop())


# The guard executes once more than the body. Its final IV must not inherit
# the body's bounds/nonzero/shift-count facts, even through exception edges.
# Keep the guard calculation before a direct IV comparison so the compiler's
# counted-path analysis can see both the operation and the recurrence.
def final_guard_division():
    i = 10
    total = 0
    try:
        while True:
            probe = 10 // i
            if i <= 0:
                break
            total += probe
            i -= 1
    except ZeroDivisionError:
        return "division", i, total
    return "missed division", i, total


def final_guard_remainder():
    i = 4
    total = 0
    try:
        while True:
            probe = 10 % i
            if i <= 0:
                break
            total += probe
            i -= 1
    except ZeroDivisionError:
        return "remainder", i, total
    return "missed remainder", i, total


def final_guard_read():
    items = [2, 3, 5, 7]
    i = 0
    total = 0
    try:
        while True:
            probe = items[i]
            if i >= 4:
                break
            total += probe
            i += 1
    except IndexError:
        return "read", i, total, items
    return "missed read", i, total, items


def final_guard_write():
    items = [0, 0, 0, 0]
    i = 0
    try:
        while True:
            items[i] = i + 10
            if i >= 4:
                break
            i += 1
    except IndexError:
        return "write", i, items
    return "missed write", i, items


def final_guard_left_shift():
    i = 2
    total = 0
    try:
        while True:
            probe = 0 << i
            if i <= -1:
                break
            total += probe
            i -= 1
    except ValueError:
        return "left shift", i, total
    return "missed left shift", i, total


def final_guard_right_shift():
    i = 2
    total = 0
    try:
        while True:
            probe = 8 >> i
            if i <= -1:
                break
            total += probe
            i -= 1
    except ValueError:
        return "right shift", i, total
    return "missed right shift", i, total


def final_guard_bigint():
    i = (1 << 63) - 2
    seen = []
    while True:
        seen.append(i)
        if i >= 1 << 63:
            break
        i += 1
    return "bigint", seen, i


print(final_guard_division())
print(final_guard_remainder())
print(final_guard_read())
print(final_guard_write())
print(final_guard_left_shift())
print(final_guard_right_shift())
print(final_guard_bigint())
