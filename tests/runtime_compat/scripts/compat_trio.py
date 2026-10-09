import trio

print("trio", trio.__version__)
print("trio.sleep exists:", hasattr(trio, "sleep"))
print("trio.open_tcp_stream exists:", hasattr(trio, "open_tcp_stream"))
