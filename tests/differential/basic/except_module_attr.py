import socket

print(socket.gaierror)
try:
    raise socket.gaierror(1, "boom")
except socket.gaierror:
    print("caught")
