# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for selectors timeout."""

import selectors


sel = selectors.DefaultSelector()
events = sel.select(timeout=0.01)
print(events == [])
