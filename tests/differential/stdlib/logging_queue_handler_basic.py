"""Purpose: differential coverage for logging queue handler basic."""

import io
import logging
import logging.handlers
import queue


stream = io.StringIO()
stream_handler = logging.StreamHandler(stream)
stream_handler.setFormatter(logging.Formatter("%(levelname)s:%(message)s"))

q: queue.Queue[logging.LogRecord] = queue.Queue()
queue_handler = logging.handlers.QueueHandler(q)
listener = logging.handlers.QueueListener(q, stream_handler)

logger = logging.getLogger("molt.queue")
logger.handlers[:] = []
logger.setLevel(logging.INFO)
logger.addHandler(queue_handler)
logger.propagate = False

listener.start()
logger.info("hello")
logger.warning("warn")
listener.stop()

stream_handler.flush()
print(stream.getvalue().strip().splitlines())
