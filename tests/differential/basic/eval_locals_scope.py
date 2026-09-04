"""Purpose: differential coverage for eval locals/globals scope."""
# MOLT_META: verified_subset_scope=dynamic_execution_policy expect_fail=molt expect_fail_reason=too_dynamic_policy


def main():
    ns = {"x": 5}
    print("eval", eval("x + 1", ns))

    locals_ns = {"x": 3}
    globals_ns = {"x": 10}
    print("locals", eval("x + 2", globals_ns, locals_ns))


if __name__ == "__main__":
    main()
