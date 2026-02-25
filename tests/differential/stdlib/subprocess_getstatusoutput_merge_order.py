"""Purpose: differential coverage for subprocess.getstatusoutput stderr merge order."""

import subprocess


def main():
    cmd = (
        "printf 'out1\\n'; "
        "printf 'err1\\n' 1>&2; "
        "printf 'out2\\n'; "
        "exit 7"
    )
    status, output = subprocess.getstatusoutput(cmd)
    print("status", status)
    print("output", output.replace("\n", "|"))


if __name__ == "__main__":
    main()
