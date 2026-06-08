"""Purpose: __del__ finalizer DISPATCH contract matrix (CPython >= 3.12 parity).

Pins that `__del__` actually RUNS, exactly once, with the correct resurrection
and exception-swallow semantics, across every way the last reference is dropped:
del statement, scope exit, reassignment, container release, and across multiple
independent instances. Each section reports a deterministic, order-stable summary
(collected events are sorted or counted where the finalizer fires relative to a
later side-effecting statement, which is governed by drop *placement* rather than
dispatch — see the finalizer-ordering baton) so the check targets dispatch
fidelity and is byte-identical to CPython.
"""

try:
    import gc
except Exception as exc:
    print(type(exc).__name__, exc)
else:
    # ── 1. del statement triggers __del__ (observed after gc.collect) ──
    seen1 = []

    class DelStmt:
        def __init__(self, tag: int) -> None:
            self.tag = tag

        def __del__(self) -> None:
            seen1.append(self.tag)

    def run_del_statement() -> None:
        obj = DelStmt(11)
        del obj
        gc.collect()

    run_del_statement()
    print("del_statement", seen1)

    # ── 2. scope exit triggers __del__ for a plain local ──
    seen2 = []

    class ScopeExit:
        def __init__(self, tag: int) -> None:
            self.tag = tag

        def __del__(self) -> None:
            seen2.append(self.tag)

    def make_scope_exit() -> None:
        ScopeExit(22)  # never bound past this statement

    make_scope_exit()
    gc.collect()
    print("scope_exit", seen2)

    # ── 3. reassignment drops the prior object ──
    seen3 = []

    class Reassign:
        def __init__(self, tag: int) -> None:
            self.tag = tag

        def __del__(self) -> None:
            seen3.append(self.tag)

    def run_reassign() -> None:
        slot = Reassign(31)
        slot = Reassign(32)  # drops 31
        del slot  # drops 32
        gc.collect()

    run_reassign()
    print("reassignment", sorted(seen3))

    # ── 4. resurrection runs __del__ exactly once ──
    seen4 = []
    keeper = []

    class ResurrectOnce:
        def __del__(self) -> None:
            seen4.append("del")
            if not keeper:
                keeper.append(self)

    def run_resurrect() -> None:
        obj = ResurrectOnce()
        del obj
        gc.collect()
        print("after_first", seen4, len(keeper))
        keeper.clear()
        gc.collect()
        print("after_second", seen4, len(keeper))

    run_resurrect()

    # ── 5. exception raised in __del__ is swallowed (unraisable) ──
    seen5 = []

    class RaiseInDel:
        def __del__(self) -> None:
            seen5.append("del-enter")
            raise ValueError("boom in finalizer")

    def run_raise_in_del() -> None:
        obj = RaiseInDel()
        del obj
        gc.collect()

    run_raise_in_del()
    print("raise_in_del", seen5, "survived")

    # ── 6. container release defers __del__ to the container drop ──
    seen6 = []

    class Held:
        def __init__(self, tag: int) -> None:
            self.tag = tag

        def __del__(self) -> None:
            seen6.append(("del", self.tag))

    def run_container_hold() -> None:
        box = [Held(71)]
        seen6.append(("boxed", box[0].tag))
        box.clear()  # last reference drops here
        gc.collect()
        seen6.append("after-clear")

    run_container_hold()
    print("container_hold", seen6)

    # ── 7. multiple independent instances each finalize ──
    seen7 = []

    class Many:
        def __init__(self, tag: int) -> None:
            self.tag = tag

        def __del__(self) -> None:
            seen7.append(self.tag)

    def run_many() -> None:
        a = Many(1)
        b = Many(2)
        c = Many(3)
        del a
        del b
        del c
        gc.collect()

    run_many()
    print("many", sorted(seen7))

    # ── 8. finalizer observes live instance __dict__ and class attrs ──
    seen8 = []

    class WithState:
        kind = "WS"

        def __init__(self, tag: int) -> None:
            self.tag = tag
            self.note = "live"

        def __del__(self) -> None:
            seen8.append(
                (self.__class__.__name__, self.__class__.kind, self.tag, self.note)
            )

    def run_with_state() -> None:
        obj = WithState(99)
        obj.note = "updated"
        del obj
        gc.collect()

    run_with_state()
    print("with_state", seen8)
