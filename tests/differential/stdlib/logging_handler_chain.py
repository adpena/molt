"""Purpose: differential coverage for logging handler chains and propagation."""

import io
import logging

stream_a = io.StringIO()
stream_b = io.StringIO()

handler_a = logging.StreamHandler(stream_a)
handler_b = logging.StreamHandler(stream_b)

root = logging.getLogger("root_demo")
root.setLevel(logging.INFO)
root.handlers[:] = [handler_a]
root.propagate = False

child = root.getChild("child")
child.handlers[:] = [handler_b]
child.propagate = True

child.info("hi")
print("a", stream_a.getvalue().strip())
print("b", stream_b.getvalue().strip())
