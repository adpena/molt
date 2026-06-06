"""Purpose: differential coverage for basic threading — creation, joining, shared state."""

import threading


# 1. Basic thread creation and join
def worker_simple(name, results, index):
    results[index] = f"done-{name}"

results = [None, None, None]
threads = []
for i in range(3):
    t = threading.Thread(target=worker_simple, args=(f"t{i}", results, i))
    threads.append(t)
    t.start()

for t in threads:
    t.join()

for r in results:
    print(r)


# 2. Thread with return value via list
def compute_square(n, out, idx):
    out[idx] = n * n

output = [0] * 5
threads = []
for i in range(5):
    t = threading.Thread(target=compute_square, args=(i, output, i))
    threads.append(t)
    t.start()

for t in threads:
    t.join()

print("squares", output)


# 3. Thread is_alive before and after join
def sleeper():
    pass

t = threading.Thread(target=sleeper)
print("before-start", t.is_alive())
t.start()
t.join()
print("after-join", t.is_alive())


# 4. Daemon thread attribute
t = threading.Thread(target=sleeper, daemon=True)
print("daemon", t.daemon)
t.start()
t.join()


# 5. Thread name
t = threading.Thread(target=sleeper, name="my-worker")
print("name", t.name)
t.start()
t.join()


# 6. Current thread
main = threading.current_thread()
print("main-thread-name", main.name)


# 7. Shared counter with lock (deterministic)
counter = [0]
lock = threading.Lock()

def increment(n, lock, counter):
    for _ in range(1000):
        with lock:
            counter[0] += 1

threads = []
for i in range(4):
    t = threading.Thread(target=increment, args=(i, lock, counter))
    threads.append(t)
    t.start()

for t in threads:
    t.join()

print("counter", counter[0])


# 8. Thread ident is not None after start
t = threading.Thread(target=sleeper)
t.start()
t.join()
print("ident-is-int", isinstance(t.ident, int))


# 9. Multiple joins are safe
t = threading.Thread(target=sleeper)
t.start()
t.join()
t.join()
print("double-join-ok")


# 10. Thread with exception does not crash parent
def raiser():
    raise ValueError("thread-boom")

t = threading.Thread(target=raiser)
t.start()
t.join()
print("parent-survived")
