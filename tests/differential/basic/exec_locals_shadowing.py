"""Purpose: differential coverage for exec locals shadowing."""
# MOLT_META: expect_fail=molt expect_fail_reason=too_dynamic_policy


def main():
    x = "outer"
    local_map = {"x": "local"}
    exec("x = 'exec'", {}, local_map)
    print("locals", local_map["x"])
    print("outer", x)


if __name__ == "__main__":
    main()
