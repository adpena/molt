"""Purpose: differential coverage for socket.if_nameindex/if_nametoindex/if_indextoname."""

import socket

# if_nameindex returns a list of (index, name) tuples
result = socket.if_nameindex()
print(isinstance(result, list))
if len(result) > 0:
    first = result[0]
    print(isinstance(first, tuple))
    print(len(first) == 2)
    print(isinstance(first[0], int))
    print(isinstance(first[1], str))
    # Round-trip: name -> index -> name
    idx = socket.if_nametoindex(first[1])
    print(idx == first[0])
    name = socket.if_indextoname(first[0])
    print(name == first[1])
else:
    print("no interfaces")

# Error cases
try:
    socket.if_nametoindex("nonexistent_interface_xyz_999")
    print("no error")
except OSError:
    print("OSError raised")

try:
    socket.if_indextoname(999999999)
    print("no error")
except OSError:
    print("OSError raised")
