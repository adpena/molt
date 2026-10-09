import tenacity

print("tenacity", tenacity.__version__)
print("retry exists:", hasattr(tenacity, "retry"))
print("stop_after_attempt exists:", hasattr(tenacity, "stop_after_attempt"))
print("wait_fixed exists:", hasattr(tenacity, "wait_fixed"))

counter = 0


@tenacity.retry(stop=tenacity.stop_after_attempt(3), wait=tenacity.wait_none())
def flaky():
    global counter
    counter += 1
    if counter < 3:
        raise ValueError("not yet")
    return "ok"


result = flaky()
print("result:", result)
print("attempts:", counter)
