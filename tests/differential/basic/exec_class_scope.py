"""Purpose: differential coverage for exec in class body."""
# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy


class Demo:
    exec("x = 1")
    exec("y = x + 1")


if __name__ == "__main__":
    print("attrs", Demo.x, Demo.y)
