import cachetools

print("cachetools", cachetools.__version__)

cache = cachetools.LRUCache(maxsize=3)
cache["a"] = 1
cache["b"] = 2
cache["c"] = 3
cache["d"] = 4  # should evict "a"

print("len:", len(cache))
print("has a:", "a" in cache)
print("has d:", "d" in cache)
print("get b:", cache["b"])
