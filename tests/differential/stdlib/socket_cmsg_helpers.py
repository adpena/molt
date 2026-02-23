"""Purpose: differential coverage for socket.CMSG_LEN, CMSG_SPACE, has_dualstack_ipv6."""

import socket

# CMSG_LEN and CMSG_SPACE return integers
cmsg_len_4 = socket.CMSG_LEN(4)
cmsg_space_4 = socket.CMSG_SPACE(4)
print(isinstance(cmsg_len_4, int))
print(isinstance(cmsg_space_4, int))
print(cmsg_len_4 > 0)
print(cmsg_space_4 >= cmsg_len_4)

# Zero-length
cmsg_len_0 = socket.CMSG_LEN(0)
cmsg_space_0 = socket.CMSG_SPACE(0)
print(cmsg_len_0 > 0)
print(cmsg_space_0 >= cmsg_len_0)

# has_dualstack_ipv6 returns a bool
result = socket.has_dualstack_ipv6()
print(isinstance(result, bool))
