import concurrent.futures as cf

print(getattr(cf, "_MOLT_THREADPOOL", "missing"))
