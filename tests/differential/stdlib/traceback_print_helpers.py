"""Purpose: differential coverage for traceback print helpers."""

import io
import traceback


def boom():
    raise ValueError("boom")


def main():
    try:
        boom()
    except Exception as exc:
        buf = io.StringIO()
        traceback.print_list(traceback.extract_tb(exc.__traceback__), file=buf)
        text = buf.getvalue()
        print("list_has_boom", "boom" in text)

    def stack_probe():
        buf = io.StringIO()
        traceback.print_stack(limit=2, file=buf)
        text = buf.getvalue()
        print("stack_has_probe", "stack_probe" in text)

    stack_probe()


if __name__ == "__main__":
    main()
