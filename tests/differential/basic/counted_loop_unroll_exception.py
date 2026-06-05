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
