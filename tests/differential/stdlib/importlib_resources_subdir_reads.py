# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-383 expect_molt_returncode=1 expect_molt_stdout_sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
"""Purpose: differential coverage for importlib.resources reads in subdirs."""

from importlib import resources


def main():
    base = resources.files("importlib")
    sub = base.joinpath("resources")
    if sub.is_dir():
        items = [entry.name for entry in sub.iterdir()]
        print("subdir", len(items) >= 0)
    else:
        print("subdir", False)


if __name__ == "__main__":
    main()
