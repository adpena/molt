# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for logging handlers timed rotation."""

import logging
import logging.handlers
import tempfile
from pathlib import Path


tmpdir = Path(tempfile.gettempdir())
path = tmpdir / "molt_timed.log"

for candidate in (path, Path(str(path) + ".2024-01-01")):
    if candidate.exists():
        candidate.unlink()

handler = logging.handlers.TimedRotatingFileHandler(
    path,
    when="S",
    interval=1,
    backupCount=1,
)
logger = logging.getLogger("molt.timed")
logger.handlers[:] = []
logger.setLevel(logging.INFO)
logger.addHandler(handler)
logger.propagate = False

logger.info("first")
handler.flush()
handler.doRollover()
logger.info("second")
handler.flush()
handler.close()

rotated = list(tmpdir.glob("molt_timed.log.*"))
print(path.exists(), len(rotated) >= 1)

for candidate in rotated:
    candidate.unlink()
if path.exists():
    path.unlink()
