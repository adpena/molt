"""Purpose: differential coverage for exec with explicit locals mapping."""
# MOLT_META: expect_fail=molt expect_fail_reason=too_dynamic_policy


def main():
    globals_ns = {"x": 1}
    locals_ns = {}
    exec("y = x + 2", globals_ns, locals_ns)
    print("globals", globals_ns.get("y"))
    print("locals", locals_ns.get("y"))


if __name__ == "__main__":
    main()
