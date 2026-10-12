# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read,os.getpid
"""File/stream emission must use the selected persistent I/O object."""

import io
import logging
from pathlib import Path
import tempfile


record = logging.LogRecord("authority", logging.INFO, "fixture", 1, "first", (), None)
stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.setFormatter(logging.Formatter("%(levelname)s:%(message)s"))
handler.terminator = "|"
handler.emit(record)
print("stream", stream.getvalue())
handler.close()

path = Path(tempfile.gettempdir()) / "molt_logging_stream_authority.log"
if path.exists():
    path.unlink()
try:
    handler = logging.FileHandler(path, mode="w", delay=True)
    handler.setFormatter(logging.Formatter("%(message)s"))
    print("delayed", handler.stream is None, path.exists())
    handler.emit(record)
    first_stream = handler.stream
    record = logging.LogRecord("authority", logging.INFO, "fixture", 2, "second", (), None)
    handler.emit(record)
    print("persistent", handler.stream is first_stream)
    handler.close()
    handler.emit(record)
    print("closed-w", handler.stream is None, repr(path.read_text()))

    for delay in (False, True):
        errors = []
        handler = logging.FileHandler(path, mode="r", delay=delay)
        handler.handleError = lambda record: errors.append("write refused")
        handler.emit(record)
        handler.close()
        print("readonly", delay, errors, repr(path.read_text()))
finally:
    if path.exists():
        path.unlink()
