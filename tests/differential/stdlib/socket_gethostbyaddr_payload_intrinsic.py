"""Purpose: validate intrinsic reverse-DNS payload shape for gethostbyaddr."""

import socket


entry = socket.gethostbyaddr("127.0.0.1")

print(isinstance(entry, tuple))
print(len(entry) == 3)
print(isinstance(entry[0], str))
print(isinstance(entry[1], list))
print(isinstance(entry[2], list))
print(any(addr == "127.0.0.1" for addr in entry[2]))
