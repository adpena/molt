"""Purpose: differential coverage for traceback print helpers."""

import io
import sys
import traceback


def boom():
    raise ValueError("boom")


def main():
    try:
        boom()
    except Exception as exc:
        buf = io.StringIO()
        extracted = traceback.extract_tb(exc.__traceback__)
        traceback.print_list(extracted, file=buf)
        text = buf.getvalue()
        print("list_has_boom", "boom" in text)
        formatted = traceback.format_tb(exc.__traceback__)
        print("tb_entries", len(formatted) == len(extracted))
        print(
            "tb_headers",
            [entry.splitlines()[0].split("in ")[-1] for entry in formatted],
        )

    # Explicit source text makes the entry-shape oracle independent of host
    # paths, source availability and backend line-number tables.
    for entries in (
        [],
        [("example.py", 7, "inner", "    x = 1")],
        [("example.py", 7, "inner", "    x = 1"), ("other.py", 9, "outer", None)],
        [("repeat.py", 3, "recursive", None)] * 3,
        [("repeat.py", 3, "recursive", None)] * 4,
        [("repeat.py", 3, "recursive", None)] * 5
        + [("other.py", 9, "outer", None)]
        + [("repeat.py", 3, "recursive", None)] * 4,
    ):
        print("explicit_entries", traceback.format_list(entries))

    summary = traceback.StackSummary.from_list(
        [("example.py", 7, "before", "    x = 1")]
    )
    summary[0].name = "after"
    summary[0].lineno = 8
    print("summary_owns_frames", summary.format())
    print("normalized_source", summary[0].line)

    def stack_probe():
        buf = io.StringIO()
        traceback.print_stack(limit=2, file=buf)
        text = buf.getvalue()
        print("stack_has_probe", "stack_probe" in text)

        # Each public adapter captures its caller before delegating. Check both
        # implicit selection and an explicit frame without comparing filenames
        # or source line numbers across targets.
        frame = sys._getframe()
        for selected in (None, frame):
            extracted = traceback.extract_stack(selected, limit=1)
            print("extract_caller", len(extracted), extracted[-1].name)
            formatted = traceback.format_stack(selected, limit=1)
            print(
                "format_caller",
                len(formatted),
                formatted[0].splitlines()[0].endswith("in stack_probe"),
            )
            buf = io.StringIO()
            traceback.print_stack(selected, limit=1, file=buf)
            print(
                "print_caller",
                buf.getvalue().splitlines()[0].endswith("in stack_probe"),
            )
            extracted[-1].name = "captured"
            print(
                "captured_summary",
                extracted.format()[0].splitlines()[0].endswith("in captured"),
            )

    stack_probe()


if __name__ == "__main__":
    main()
