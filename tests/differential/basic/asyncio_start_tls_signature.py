import asyncio
import inspect


def _has_params(obj, names):
    params = inspect.signature(obj).parameters
    return all(name in params for name in names)


required = (
    "server_side",
    "server_hostname",
    "ssl_handshake_timeout",
    "ssl_shutdown_timeout",
)

print(
    "abstract_start_tls_signature",
    _has_params(asyncio.AbstractEventLoop.start_tls, required),
)

loop = asyncio.new_event_loop()
try:
    print("loop_start_tls_signature", _has_params(loop.start_tls, required))
finally:
    loop.close()
