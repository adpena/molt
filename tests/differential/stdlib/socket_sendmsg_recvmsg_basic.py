"""Purpose: differential coverage for socket.sendmsg/recvmsg intrinsic paths."""

import socket

left, right = socket.socketpair()
try:
    sent = left.sendmsg([b"he", b"llo"])
    print("sendmsg_sent", sent)

    data, anc, msg_flags, addr = right.recvmsg(16)
    print("recvmsg_data", data)
    print("recvmsg_anc_len", len(anc))
    print("recvmsg_flags", int(msg_flags))
    print("recvmsg_addr_is_none", addr is None)
    sent2 = left.sendmsg((b"ab", b"cd"))
    print("sendmsg_sent_tuple", sent2)
    data2, anc2, msg_flags2, addr2 = right.recvmsg(16)
    print("recvmsg_data_tuple", data2)
    print("recvmsg_anc_len_tuple", len(anc2))
    print("recvmsg_flags_tuple", int(msg_flags2))
    print("recvmsg_addr_is_none_tuple", addr2 is None)
finally:
    left.close()
    right.close()
