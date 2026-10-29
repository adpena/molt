# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-383 expect_molt_returncode=0 expect_molt_stdout_sha256=29b476be0a40935f9a2f27247551f19008e37a329fb5ee6cab7934d033ff1576
"""Purpose: differential coverage for importlib.resources read_* errors."""

from importlib import resources


def main():
    try:
        resources.read_text("importlib", "missing.txt")
        print("read_text", "missed")
    except Exception as exc:
        print("read_text", type(exc).__name__)

    try:
        resources.read_binary("importlib", "missing.bin")
        print("read_binary", "missed")
    except Exception as exc:
        print("read_binary", type(exc).__name__)


if __name__ == "__main__":
    main()
