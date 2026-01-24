"""Purpose: differential coverage for pickle basic."""

import copy
import pickle


def main():
    s = slice(1, 10, 2)
    payload = {"s": s, "t": (1, None, True), "l": [3, 4], "d": {"x": 5, "y": 6}}
    data = pickle.dumps(payload, protocol=0)
    out = pickle.loads(data)
    encoded = "ok".encode()
    print(out["s"].start, out["s"].stop, out["s"].step)
    print(out["t"], out["l"], out["d"]["x"], out["d"]["y"])
    print(encoded.decode())
    print(copy.copy(s) is s, copy.deepcopy(s) is s)


if __name__ == "__main__":
    main()
