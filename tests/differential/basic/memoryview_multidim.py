import traceback


def show_error(fn):
    try:
        fn()
    except Exception as exc:  # noqa: BLE001 - intentional for parity checks
        print(traceback.format_exception_only(type(exc), exc)[0].strip())


ba = bytearray(range(12))
mv = memoryview(ba)
mv2 = mv.cast("B", shape=[3, 4])
print(mv2.shape)
print(mv2.strides)
print(mv2[1, 2])
print(mv2[-1, -1])
show_error(lambda: mv2[1])
show_error(lambda: mv2[1, 2, 3])
show_error(lambda: mv2[:, 1])
show_error(lambda: mv2[:, :])

mv0 = memoryview(bytearray(b"a")).cast("B", shape=[])
show_error(lambda: len(mv0))
print(mv0[()])
show_error(lambda: mv0[0])

show_error(lambda: mv.cast(">B"))
show_error(lambda: mv.cast("B", shape=[2, 2]))
show_error(lambda: mv.cast("B", shape=1))
show_error(lambda: mv.cast("B", shape=[1, "a"]))
show_error(lambda: mv.cast("B", shape=[0]))

mvh = memoryview(bytearray([0, 0, 0, 0])).cast("H")
mvh[0] = 500
print(mvh[0])

mvc = memoryview(bytearray(b"abc")).cast("c")
print(mvc[0])
mvc[0] = b"z"
print(mvc[0])


def assign_c(val):
    mvc[0] = val


show_error(lambda: assign_c(b"zz"))
show_error(lambda: assign_c(120))
