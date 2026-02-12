"""Purpose: differential coverage for multiprocessing start-method selection."""

import multiprocessing as mp


def main():
    methods = mp.get_all_start_methods()
    print("methods", methods)
    ctx = mp.get_context("spawn")
    print("ctx", ctx.get_start_method())
    default = mp.get_start_method(allow_none=True)
    print("default", default)


if __name__ == "__main__":
    main()
