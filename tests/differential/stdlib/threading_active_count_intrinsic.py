import threading
import time


done = []


def worker():
    time.sleep(0.05)
    done.append("ok")


start = threading.active_count()
thread = threading.Thread(target=worker)
thread.start()
mid = threading.active_count()
thread.join()
end = threading.active_count()

print("THREAD", start >= 1, mid >= start, end >= 1, len(done))
