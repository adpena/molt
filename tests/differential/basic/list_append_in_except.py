# MOLT_META: expect_fail=molt expect_fail_reason=backend_cfg_drops_except_handler_when_try_body_unconditionally_raises
"""Purpose: list.append() inside an `except` block must mutate the list.

XFAIL (pre-existing BACKEND bug, not the frontend super fix in this change):
the real root cause is narrower than "append in except" -- when a try body's
ONLY (or first/unconditional) statement is a `raise`, the except handler is
dropped entirely and the exception escapes uncaught. The frontend correctly
emits `TRY_START; RAISE; CHECK_EXCEPTION handler; JUMP handler; LABEL handler;
<handler ops>`, but because RAISE terminates the basic block the backend CFG
construction treats the handler block (reachable only via the TRY_START
exception edge) as unreachable and prunes it. Adding any statement before the
raise (so an auto-CHECK_EXCEPTION creates a live edge) makes it pass. Same root
cause as `super_no_args_errors` (a bare `super()` raising inside a try). Fix:
the backend must keep TRY_START exception-target blocks reachable
(runtime/molt-passes/src/tir/lower_to_simple.rs / lower_from_simple.rs CFG
construction), outside this change's frontend lane. See the session baton.

Output must be byte-identical to CPython 3.14.
"""


def collect_errors() -> list:
    results: list = []
    for i in range(5):
        try:
            if i % 2 == 0:
                raise ValueError("even " + str(i))
            results.append(("ok", i))
        except ValueError as e:
            results.append(("err", str(e)))
    return results


def append_in_except_simple() -> list:
    log: list = []
    try:
        raise RuntimeError("boom")
    except RuntimeError as e:
        log.append("caught:" + str(e))
        log.append("second")
    return log


def nested_append() -> list:
    out: list = []
    for x in range(3):
        try:
            try:
                raise KeyError(x)
            except KeyError:
                out.append("inner:" + str(x))
                raise IndexError(x)
        except IndexError:
            out.append("outer:" + str(x))
    return out


def main() -> None:
    print(collect_errors())
    print(append_in_except_simple())
    print(nested_append())


if __name__ == "__main__":
    main()
