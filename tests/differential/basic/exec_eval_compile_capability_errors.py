"""Purpose: differential coverage for exec/eval/compile capability errors."""
# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy


def show(label: str, thunk) -> None:
    try:
        result = thunk()
        print(label, "ok", result)
    except Exception as exc:
        print(label, type(exc).__name__, exc)


show("eval", lambda: eval("1 + 2"))
show("exec", lambda: exec("x = 5"))
show("compile-eval", lambda: compile("1+1", "<string>", "eval"))
show("compile-exec", lambda: compile("x = 1", "<string>", "exec"))
