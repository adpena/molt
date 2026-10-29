# MOLT_META: backends=jit,llvm,native platforms=posix
# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,process.exec
"""Purpose: a failing checked child raises subprocess.CalledProcessError with its fields."""

import subprocess


def probe(label, func):
    try:
        func()
    except subprocess.CalledProcessError as exc:
        print(
            label,
            type(exc) is subprocess.CalledProcessError,
            isinstance(exc, subprocess.SubprocessError),
            exc.returncode,
            exc.cmd,
            exc.output,
            exc.stderr,
        )
        print(label, str(exc))
    except Exception as exc:  # noqa: BLE001 - the observation is the escape
        print(label, "escaped", type(exc).__name__)
    else:
        print(label, "no error")


def main():
    probe("check_output", lambda: subprocess.check_output(["sh", "-c", "echo out; exit 3"]))
    probe("check_call", lambda: subprocess.check_call(["sh", "-c", "exit 4"]))
    probe(
        "run check",
        lambda: subprocess.run(["sh", "-c", "echo err >&2; exit 5"], check=True, capture_output=True),
    )


if __name__ == "__main__":
    main()
