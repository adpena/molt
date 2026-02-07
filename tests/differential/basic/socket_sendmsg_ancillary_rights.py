"""Purpose: differential coverage for socket sendmsg/recvmsg ancillary data on Unix."""

import socket
import struct

if not hasattr(socket, "SCM_RIGHTS") or not hasattr(socket, "SOL_SOCKET"):
    print("skip", "no_scm_rights")
    raise SystemExit(0)

left, right = socket.socketpair()
data_left, data_right = socket.socketpair()
try:
    fd_size = struct.calcsize("i")
    payload = struct.pack("i", data_left.fileno())
    sent = left.sendmsg([b"R"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, payload)])
    print("sent", sent)

    data, anc, msg_flags, addr = right.recvmsg(16, 256)
    print("data", data)
    print("anc_len", len(anc))
    print("flags", int(msg_flags))
    print("addr_is_none", addr is None)

    if anc:
        level, ctype, cdata = anc[0]
        print("anc_meta", int(level), int(ctype), len(cdata))
        recv_fd = struct.unpack("i", bytes(cdata[:fd_size]))[0]
        transferred = socket.socket(fileno=recv_fd)
        try:
            transferred.sendall(b"ok")
            print("peer_read", data_right.recv(2))
        finally:
            transferred.close()
finally:
    left.close()
    right.close()
    data_left.close()
    data_right.close()
