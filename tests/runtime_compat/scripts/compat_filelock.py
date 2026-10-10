import filelock
import tempfile
import os

print("filelock", filelock.__version__)
path = os.path.join(tempfile.gettempdir(), "molt_test.lock")
lock = filelock.FileLock(path)
with lock:
    print("acquired:", lock.is_locked)
print("released:", not lock.is_locked)
