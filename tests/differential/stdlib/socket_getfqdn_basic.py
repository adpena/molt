# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for socket getfqdn basic."""

import socket


print(isinstance(socket.getfqdn("127.0.0.1"), str))
