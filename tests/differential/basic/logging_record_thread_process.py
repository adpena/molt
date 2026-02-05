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


if __name__ == "__main__":
    main()
