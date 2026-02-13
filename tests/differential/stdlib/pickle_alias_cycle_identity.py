"""Differential coverage for pickle memo identity and cycles."""

from __future__ import annotations

import pickle


def main() -> None:
    shared = ["shared"]
    payload = [shared, shared]
    out = pickle.loads(pickle.dumps(payload, protocol=5))
    print("alias_list", out[0] is out[1], out[0][0], out[1][0])

    cyc = []
    cyc.append(cyc)
    out_cyc = pickle.loads(pickle.dumps(cyc, protocol=4))
    print("cycle_list", out_cyc is out_cyc[0], len(out_cyc))

    value = {"k": "v"}
    carrier = {"a": value, "b": value}
    out_dict = pickle.loads(pickle.dumps(carrier, protocol=4))
    print("alias_dict", out_dict["a"] is out_dict["b"], list(sorted(out_dict.keys())))


if __name__ == "__main__":
    main()
