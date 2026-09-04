"""Purpose: differential coverage for exec in function locals."""
# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy


def inner():
    x = 1
    exec("x = 2")
    return x, locals().get("x")


if __name__ == "__main__":
    result = inner()
    print("result", result)
