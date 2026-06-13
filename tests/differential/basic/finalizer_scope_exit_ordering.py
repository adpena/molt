"""Purpose: #58 ordering keystone — a finalizer-sensitive local (an instance
whose class defines ``__del__``, or a container absorbing one) is released at
its Python lifetime boundary (function scope exit), NOT at its SSA last-use.

CPython holds ``bag`` in the frame until ``run`` returns, so ``__del__`` fires
AFTER the body's last statement and BEFORE the caller's next statement. The
pre-#58 molt behavior dropped the list at its SSA last-use (the assignment) and
printed DEL first. This file is the c_scope repro promoted to a durable
regression anchor (module-free isolation; see doc 50 §A and
ownership_lattice_min.rs).
"""


events = []


class A:
    def __del__(self) -> None:
        print("DEL")
        events.append("del")


def run() -> None:
    bag = [A()]  # A held by list; bag never read again (SSA last-use = here)
    print("in run")


def run_direct() -> None:
    a = A()  # direct finalizer-bearing local, no container
    print("in run_direct")


def run_used_later() -> None:
    bag = [A()]
    print("in run_used_later")
    print("len", len(bag))  # a real later use; release still at scope exit
    print("after use")


def run_events() -> None:
    bag = [A()]
    print("inside events", list(events))


run()
print("between")
run_direct()
print("between2")
run_used_later()
print("between3")
run_events()
print("after events", list(events))
print("end")
