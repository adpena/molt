"""Purpose: differential coverage for intrinsic-backed threading.stack_size."""

import threading


print("initial_is_int", isinstance(threading.stack_size(), int))

try:
    threading.stack_size(1.25)  # type: ignore[arg-type]
except Exception as exc:
    print("float_exc", type(exc).__name__)

try:
    threading.stack_size(-1)
except Exception as exc:
    print("neg_exc", type(exc).__name__)

target = 1 << 20
prev = threading.stack_size(target)
print("set_prev_is_int", isinstance(prev, int))
print("set_now_eq", threading.stack_size() == target)

threading.stack_size(0)
print("reset_zero", threading.stack_size())
