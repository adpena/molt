def run():
    # Direct set access - works (based on set_basic.py)
    s = {1}
    s.add(2)
    print(len(s))

    # Access via dict (dynamic)
    d = {"tags": {1}}
    # In CPython, d["tags"] is the set object.
    # In Molt, if d is dict[str, Object], this is a dynamic retrieval.
    tags = d["tags"]
    try:
        tags.add(2)
        print("Dynamic add success")
    except AttributeError as e:
        print(f"Dynamic add failed: {e}")
    print(len(tags))


if __name__ == "__main__":
    run()
