"""Purpose: differential coverage for __traceback__ attribute access."""


def main():
    try:
        raise RuntimeError("boom")
    except RuntimeError as exc:
        tb = exc.__traceback__
        print("tb", tb is not None)
        print("frame", tb.tb_frame.f_code.co_name)


if __name__ == "__main__":
    main()
