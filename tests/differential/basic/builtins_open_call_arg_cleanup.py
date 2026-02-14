import builtins
import os


def sink(value):
    return 1


def main() -> None:
    # Use a duplicated fd so we don't mutate the process' stdin (fd=0) state.
    fd = os.dup(0)
    sink(builtins.open(fd))
    try:
        builtins.open(fd)
        print("second ok", flush=True)
    except Exception as exc:  # noqa: BLE001
        print("second err", type(exc).__name__, flush=True)


main()
