#!/usr/bin/env python3
"""Minimal os.setsid daemonizing launcher for long calibration runs (task #55).

The detach hazard: a `nohup cmd &` launched from inside an agent Bash tool is
reaped when the launcher's process GROUP is terminated (the harness SIGKILLs the
group; nohup only blocks SIGHUP). A long differential calibration (1935 stdlib
tests serial) outlives many launcher turns, so it must run in its OWN session,
immune to the launcher group's teardown.

This wrapper double-forks + os.setsid() so the child becomes a session leader in
a brand-new process group with no controlling terminal, then execs the target
command with stdout/stderr redirected to a log file. The grandchild's PID is
written to --pidfile so the run can be polled/inspected. Exit status is appended
to --donefile when the child exits, so a poller can detect completion without a
live parent.

USAGE
-----
  python3 tools/calib_daemon.py \\
      --log RUN.log --pidfile RUN.pid --donefile RUN.done -- \\
      python3 tests/molt_diff.py --jobs 1 --files-from LIST.txt

Environment is inherited (set MOLT_DIFF_RESULTS_JSONL etc. before launching).
"""

from __future__ import annotations

import argparse
import os
import sys
import time


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--log", required=True, help="stdout+stderr log for the run")
    ap.add_argument("--pidfile", required=True, help="grandchild PID is written here")
    ap.add_argument("--donefile", required=True, help="exit status appended on finish")
    ap.add_argument("--cwd", default=None, help="working directory for the command")
    ap.add_argument("command", nargs=argparse.REMAINDER, help="-- then the command")
    args = ap.parse_args(argv)

    cmd = args.command
    if cmd and cmd[0] == "--":
        cmd = cmd[1:]
    if not cmd:
        print("no command given (use -- before the command)", file=sys.stderr)
        return 2

    log_path = os.path.abspath(args.log)
    pidfile = os.path.abspath(args.pidfile)
    donefile = os.path.abspath(args.donefile)
    cwd = os.path.abspath(args.cwd) if args.cwd else os.getcwd()

    # First fork: parent returns to the launcher immediately.
    pid = os.fork()
    if pid > 0:
        # Wait briefly for the pidfile so the launcher can report the real PID.
        for _ in range(50):
            if os.path.exists(pidfile):
                break
            time.sleep(0.1)
        try:
            real = open(pidfile).read().strip()
        except OSError:
            real = "?"
        print(f"daemon launched; run PID={real}; log={log_path}")
        return 0

    # Child: become a session leader (new pgroup, no controlling tty).
    os.setsid()

    # Second fork: ensure we are not a session leader (cannot reacquire a tty).
    pid2 = os.fork()
    if pid2 > 0:
        os._exit(0)

    # Grandchild: the actual long-lived runner.
    os.chdir(cwd)
    # Detach stdio: stdin from /dev/null, stdout/stderr to the log.
    with open(os.devnull, "rb") as devnull:
        os.dup2(devnull.fileno(), 0)
    logf = open(log_path, "ab", buffering=0)
    os.dup2(logf.fileno(), 1)
    os.dup2(logf.fileno(), 2)

    # Run the command as a child so we can capture its exit status and write the
    # donefile (a bare exec would lose the completion signal).
    child = os.fork()
    if child == 0:
        try:
            os.execvp(cmd[0], cmd)
        except OSError as exc:
            sys.stderr.write(f"exec failed: {exc}\n")
            os._exit(127)

    with open(pidfile, "w") as pf:
        pf.write(str(child))

    _pid, status = os.waitpid(child, 0)
    if os.WIFEXITED(status):
        code = os.WEXITSTATUS(status)
    elif os.WIFSIGNALED(status):
        code = 128 + os.WTERMSIG(status)
    else:
        code = -1
    with open(donefile, "w") as df:
        df.write(f"{code}\n")
    os._exit(0)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
