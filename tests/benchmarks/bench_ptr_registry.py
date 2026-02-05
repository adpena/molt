from molt import intrinsics as _intrinsics

molt_chan_new = _intrinsics.require("molt_chan_new", globals())
molt_chan_drop = _intrinsics.require("molt_chan_drop", globals())


def main() -> None:
    total = 0
    i = 0
    while i < 100_000:
        handle = molt_chan_new(0)
        molt_chan_drop(handle)
        total += 1
        i += 1

    print(total)


if __name__ == "__main__":
    main()
