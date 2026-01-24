"""Purpose: differential coverage for socket eai constants."""

import socket


names = ["EAI_AGAIN", "EAI_FAIL", "EAI_NONAME", "EAI_SERVICE", "EAI_SOCKTYPE"]
values = []
for name in names:
    values.append((name, isinstance(getattr(socket, name, None), int)))
print(values)
