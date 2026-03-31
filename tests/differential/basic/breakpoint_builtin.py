"""Purpose: differential coverage for breakpoint() builtin."""
# MOLT_META: expect_fail=molt expect_fail_reason=too_dynamic_policy
import sys


if __name__ == "__main__":
    # breakpoint exists and is callable
    print("breakpoint callable", callable(breakpoint))
    print("breakpoint type", type(breakpoint).__name__)

    # Custom breakpoint hook captures calls
    calls = []

    def custom_hook(*args, **kwargs):
        calls.append(("hook", args, tuple(sorted(kwargs.items()))))

    old_hook = sys.breakpointhook
    try:
        sys.breakpointhook = custom_hook

        # Call with no args
        breakpoint()
        print("call no args", len(calls))

        # Call with positional args
        breakpoint("trace1", 42)
        print("call with args", len(calls))
        print("args value", calls[-1][1])

        # Call with keyword args
        breakpoint(header="debug")
        print("call with kwargs", len(calls))
        print("kwargs value", calls[-1][2])

        # Call with mixed args
        breakpoint("a", "b", level=3)
        print("call mixed", len(calls))
        print("mixed args", calls[-1][1])
        print("mixed kwargs", calls[-1][2])

    finally:
        sys.breakpointhook = old_hook

    # sys.breakpointhook is the default hook
    print("breakpointhook exists", hasattr(sys, "breakpointhook"))
    print("breakpointhook callable", callable(sys.breakpointhook))

    # Hook that raises is propagated
    def raising_hook(*args, **kwargs):
        raise RuntimeError("hook error")

    old_hook = sys.breakpointhook
    try:
        sys.breakpointhook = raising_hook
        try:
            breakpoint()
            print("raising should not reach")
        except RuntimeError as e:
            print("hook error caught", str(e))
    finally:
        sys.breakpointhook = old_hook

    # Hook returning a value (breakpoint returns what hook returns)
    def returning_hook(*args, **kwargs):
        return "hook_result"

    old_hook = sys.breakpointhook
    try:
        sys.breakpointhook = returning_hook
        result = breakpoint()
        print("hook return", result)
    finally:
        sys.breakpointhook = old_hook

    print("done")
