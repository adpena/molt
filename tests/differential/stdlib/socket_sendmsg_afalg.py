"""Purpose: differential coverage for socket.sendmsg_afalg (Linux-only)."""

import socket
import sys

if sys.platform != "linux":
    print("skip not_linux")
else:
    # Test that sendmsg_afalg method exists
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    print(hasattr(s, "sendmsg_afalg"))
    s.close()

    if not hasattr(socket, "AF_ALG"):
        print("skip no_af_alg")
    else:
        # Test with AF_ALG socket for SHA256
        try:
            alg = socket.socket(socket.AF_ALG, socket.SOCK_SEQPACKET, 0)
            alg.bind(("hash", "sha256"))
            op, _ = alg.accept()
            try:
                op.sendmsg_afalg(msg=b"hello", op=socket.ALG_OP_ENCRYPT)
                digest = op.recv(32)
                print(len(digest) == 32)
                print(
                    digest.hex()
                    == "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                )
            finally:
                op.close()
                alg.close()
        except OSError as e:
            # Kernel may not have af_alg module loaded
            print("afalg_oserror")
