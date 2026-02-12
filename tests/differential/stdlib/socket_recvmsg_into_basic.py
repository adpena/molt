"""Purpose: differential coverage for socket.recvmsg_into intrinsic paths."""

import socket

left, right = socket.socketpair()
try:
    left.sendmsg([b"abcdef"])
    first = bytearray(2)
    second = bytearray(4)
    n, anc, msg_flags, addr = right.recvmsg_into([first, second], 0)
    print("recvmsg_into_n", n)
    print("recvmsg_into_first", bytes(first))
    print("recvmsg_into_second", bytes(second))
    print("recvmsg_into_anc_len", len(anc))
    print("recvmsg_into_flags", int(msg_flags))
    print("recvmsg_into_addr_is_none", addr is None)

    left.sendmsg([b"xyz"])
    mv_buf = bytearray(3)
    mv = memoryview(mv_buf)
    n2, anc2, msg_flags2, addr2 = right.recvmsg_into([mv], 0)
    print("recvmsg_into_mv_n", n2)
    print("recvmsg_into_mv_buf", bytes(mv_buf))
    print("recvmsg_into_mv_anc_len", len(anc2))
    print("recvmsg_into_mv_flags", int(msg_flags2))
    print("recvmsg_into_mv_addr_is_none", addr2 is None)
finally:
    left.close()
    right.close()
