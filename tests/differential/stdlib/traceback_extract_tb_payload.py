"""Purpose: validate extract_tb uses payload-shaped traceback entries."""

import traceback


def boom():
    raise ValueError("boom")


try:
    boom()
except Exception as exc:  # noqa: BLE001
    stack = traceback.extract_tb(exc.__traceback__)
    lines = traceback.format_list(stack)
    tb_lines = traceback.format_tb(exc.__traceback__)
    print("has_frames", len(stack) > 0)
    print("format_list_has_boom", any("boom" in line for line in lines))
    print("format_tb_has_boom", any("boom" in line for line in tb_lines))
