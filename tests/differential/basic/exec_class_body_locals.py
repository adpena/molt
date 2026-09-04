"""Purpose: differential coverage for exec in class body locals mapping."""
# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy


class Demo:
    namespace = {}
    exec("x = 10", {}, namespace)


if __name__ == "__main__":
    print("class_has_x", hasattr(Demo, "x"))
    print("namespace", Demo.namespace.get("x"))
