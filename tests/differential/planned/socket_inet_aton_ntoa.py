# MOLT_ENV: MOLT_CAPABILITIES=net.outbound
"""Purpose: differential coverage for socket inet aton ntoa."""

import socket


packed = socket.inet_aton("127.0.0.1")
print(len(packed))
print(socket.inet_ntoa(packed))
