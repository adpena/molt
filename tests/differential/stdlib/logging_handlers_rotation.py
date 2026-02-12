# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for logging handlers rotation."""

import logging
import logging.handlers
import tempfile
from pathlib import Path


tmpdir = Path(tempfile.gettempdir())
path = tmpdir / "molt_rotate.log"
if path.exists():
    path.unlink()
rotated = Path(str(path) + ".1")
if rotated.exists():
    rotated.unlink()

handler = logging.handlers.RotatingFileHandler(path, maxBytes=20, backupCount=1)
logger = logging.getLogger("molt.rotate")
logger.handlers[:] = []
logger.setLevel(logging.INFO)
logger.addHandler(handler)
logger.propagate = False

logger.info("first")
logger.info("second")
handler.flush()
handler.close()

print(path.exists(), rotated.exists())
if path.exists():
    path.unlink()
if rotated.exists():
    rotated.unlink()
