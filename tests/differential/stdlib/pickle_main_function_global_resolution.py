"""Differential coverage for pickle function globals in __main__ resolution."""

from __future__ import annotations

import pickle


class Sentinel:
    pass


def rebuild_and_resolve() -> type[Sentinel]:
    return Sentinel


def rebuild_and_instantiate() -> Sentinel:
    return Sentinel()


def main() -> None:
    resolve_payload = pickle.dumps(rebuild_and_resolve, protocol=5)
    resolve_restored = pickle.loads(resolve_payload)
    print("function_identity", resolve_restored is rebuild_and_resolve)
    print("global_resolution", resolve_restored() is Sentinel)

    instantiate_payload = pickle.dumps(rebuild_and_instantiate, protocol=5)
    instantiate_restored = pickle.loads(instantiate_payload)
    instance = instantiate_restored()
    print("instantiate_identity", instantiate_restored is rebuild_and_instantiate)
    print("instantiate_type", type(instance).__name__)


if __name__ == "__main__":
    main()
