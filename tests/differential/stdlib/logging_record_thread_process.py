# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,os.getpid
"""Purpose: differential coverage for logging LogRecord thread/process fields."""

import logging
import os
import threading


def main() -> None:
    record = logging.LogRecord("demo", logging.INFO, __file__, 12, "hi", (), None)
    print("thread_match", record.thread == threading.get_ident())
    print("process_match", record.process == os.getpid())
    print("thread_name", record.threadName)
    print("process_name", record.processName)

    original_getpid = os.getpid
    original_log_processes = logging.logProcesses
    calls = []

    def marked_getpid():
        calls.append("pid")
        return 73

    try:
        os.getpid = marked_getpid
        logging.logProcesses = True
        marked = logging.LogRecord("demo", logging.INFO, __file__, 18, "hi", (), None)
        print("process_override", marked.process == 73, calls == ["pid"])
        print("process_formatted", logging.Formatter("%(process)s").format(marked))
        calls.clear()
        logging.logProcesses = False
        disabled = logging.LogRecord("demo", logging.INFO, __file__, 24, "hi", (), None)
        print("process_disabled", disabled.process is None, calls == [])
        print("disabled_formatted", logging.Formatter("%(process)s").format(disabled))
    finally:
        os.getpid = original_getpid
        logging.logProcesses = original_log_processes


if __name__ == "__main__":
    main()
