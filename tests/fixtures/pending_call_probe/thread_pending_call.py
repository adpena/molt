from pending_call_probe._native import arm_thread_callback, thread_callback_count


assert thread_callback_count() == 0
arm_thread_callback()
# A real taken Python backedge is the shared dispatch obligation after the
# publisher's blocking join releases/restores the main thread's GIL custody.
total = 0
for i in range(1000):
    total += i
assert thread_callback_count() == 1
arm_thread_callback()
for i in range(1000):
    total += i
assert thread_callback_count() == 2
print("foreign thread pending calls", thread_callback_count(), total)
