from concurrent.futures import ThreadPoolExecutor

ex = ThreadPoolExecutor.__new__(ThreadPoolExecutor)
print("probe: calling __init__")
try:
    ThreadPoolExecutor.__init__(ex, 2)
    print("probe: init ok")
except Exception as exc:
    print("probe: init error", type(exc).__name__, exc)
    raise
