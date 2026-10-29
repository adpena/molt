# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-207 expect_molt_returncode=0 expect_molt_stdout_sha256=e12c216e42263a21ce48d249d39e36efd420c422910dfdc6fcd939b1b36559a3
"""Purpose: an unreadable compressed archive raises tarfile.ReadError."""

import os
import tarfile
import tempfile


def probe(label, path, mode):
    try:
        archive = tarfile.open(path, mode)
    except tarfile.ReadError as exc:
        print(
            label,
            "ReadError",
            type(exc) is tarfile.ReadError,
            isinstance(exc, tarfile.TarError),
            type(exc).__module__,
        )
    except tarfile.TarError as exc:
        print(label, "TarError only", type(exc).__name__)
    except Exception as exc:  # noqa: BLE001 - the observation is the escape
        print(label, "escaped", type(exc).__name__)
    else:
        archive.close()
        print(label, "no error")


def main():
    with tempfile.TemporaryDirectory() as root:
        path = os.path.join(root, "junk.tar")
        with open(path, "wb") as handle:
            handle.write(b"this is not a compressed archive" * 32)
        probe("gzip", path, "r:gz")
        probe("bzip2", path, "r:bz2")
    print("ReadError base", tarfile.ReadError.__mro__[1].__name__)


if __name__ == "__main__":
    main()
