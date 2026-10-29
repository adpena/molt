# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-383 expect_molt_returncode=1 expect_molt_stdout_sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
"""Purpose: differential coverage for importlib.resources.as_file."""

from importlib import resources


def main():
    resource = resources.files("importlib").joinpath("__init__.py")
    with resources.as_file(resource) as path:
        print("exists", path.exists())
        print("suffix", path.suffix)


if __name__ == "__main__":
    main()
