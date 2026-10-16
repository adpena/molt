# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,os.cpu_count,os.getpid,os.uname
"""Purpose: differential coverage for os.access, os.cpu_count, os.getpid,
os.getppid, os.devnull, os.F_OK/R_OK/W_OK/X_OK, os.umask, os.uname."""

import os

print("cpu_count:", type(os.cpu_count()).__name__)
print("cpu_count > 0:", os.cpu_count() > 0)
print("getpid:", type(os.getpid()).__name__)
print("getpid > 0:", os.getpid() > 0)
print("devnull:", os.devnull)
print("F_OK:", os.F_OK)
print("R_OK:", os.R_OK)
print("W_OK:", os.W_OK)
print("X_OK:", os.X_OK)
print("access /tmp:", os.access("/tmp", os.F_OK))
# umask: get and restore
old = os.umask(0o022)
os.umask(old)
print("umask round-trip ok")
print("uname type:", type(os.uname()).__name__)
print("uname sysname:", type(os.uname().sysname).__name__)

# The public operations share access semantics, but which converts mode only
# after a candidate exists and repeats conversion for each existing candidate.
import shutil
import tempfile


class AccessMode:
    def __init__(self):
        self.calls = 0

    def __index__(self):
        self.calls += 1
        return os.X_OK


with tempfile.TemporaryDirectory() as access_root:
    first_dir = os.path.join(access_root, "first")
    second_dir = os.path.join(access_root, "second")
    os.mkdir(first_dir)
    os.mkdir(second_dir)
    first = os.path.join(first_dir, "molt-access-probe")
    second = os.path.join(second_dir, "molt-access-probe")
    for filename, permissions in ((first, 0o600), (second, 0o700)):
        with open(filename, "wb") as probe:
            probe.write(b"probe")
        os.chmod(filename, permissions)
    mode = AccessMode()
    print("access indexed:", os.access(second, mode), mode.calls)
    mode = AccessMode()
    search = os.pathsep.join((first_dir, second_dir))
    print(
        "which indexed:",
        shutil.which("molt-access-probe", mode=mode, path=search) == second,
        mode.calls,
    )
    print(
        "which missing invalid mode:",
        shutil.which("absent-molt-access-probe", mode=object(), path=search),
    )
    for filename in (second, os.path.join(access_root, "absent")):
        try:
            os.access(filename, object())
        except Exception as error:
            print("access invalid mode:", type(error).__name__)
    try:
        shutil.which("molt-access-probe", mode=object(), path=search)
    except Exception as error:
        print("which existing invalid mode:", type(error).__name__)

    # C-int conversion is required even when the target is a real executable.
    # A single existing which candidate makes its callback count unambiguous.
    class CheckedAccessMode:
        def __init__(self, value, error=None):
            self.value = value
            self.error = error
            self.calls = 0

        def __index__(self):
            self.calls += 1
            if self.error is not None:
                raise self.error
            return self.value

    for operation in ("access", "which"):
        for sign, value in (("positive", 2**31), ("negative", -(2**31) - 1)):
            for indexed in (False, True):
                mode = CheckedAccessMode(value) if indexed else value
                try:
                    if operation == "access":
                        os.access(second, mode)
                    else:
                        shutil.which("molt-access-probe", mode=mode, path=second_dir)
                except Exception as error:
                    print(
                        "C-int overflow:",
                        operation,
                        sign,
                        indexed,
                        type(error).__name__,
                        str(error),
                        mode.calls if indexed else 0,
                    )
                else:
                    print("C-int overflow accepted:", operation, sign, indexed)

        marker = RuntimeError("access mode callback")
        mode = CheckedAccessMode(0, marker)
        try:
            if operation == "access":
                os.access(second, mode)
            else:
                shutil.which("molt-access-probe", mode=mode, path=second_dir)
        except Exception as error:
            print(
                "index callback error:",
                operation,
                type(error).__name__,
                str(error),
                error is marker,
                mode.calls,
            )
        else:
            print("index callback error swallowed:", operation, mode.calls)
