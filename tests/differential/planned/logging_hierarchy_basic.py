"""Purpose: differential coverage for logging hierarchy basic."""

import io
import logging


stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.setFormatter(logging.Formatter("%(name)s:%(message)s"))

root = logging.getLogger()
root.handlers[:] = []
root.setLevel(logging.INFO)
root.addHandler(handler)

parent = logging.getLogger("molt")
child = logging.getLogger("molt.child")

parent.setLevel(logging.INFO)
child.setLevel(logging.INFO)

parent.info("p")
child.info("c")
handler.flush()

print(stream.getvalue().strip().splitlines())
