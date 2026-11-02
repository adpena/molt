"""Purpose: differential coverage for multiprocessing start-method selection."""

import multiprocessing as mp


def main():
    methods = mp.get_all_start_methods()
    # Compare the supported common operation. Molt's exact spawn-only set and
    # fork/forkserver refusals are owned by test_stdlib_todo_burndown_fail_closed.
    print("spawn available", "spawn" in methods)
    ctx = mp.get_context("spawn")
    print("ctx", ctx.get_start_method())
    default = mp.get_start_method(allow_none=True)
    print("default", default)
    mp.set_start_method("spawn")
    print("selected", mp.get_start_method())


if __name__ == "__main__":
    main()
