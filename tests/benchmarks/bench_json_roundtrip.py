import json


def main() -> None:
    items: list[dict[str, object]] = []
    i = 0
    while i < 200:
        items.append(
            {
                "id": i,
                "sku": "SKU-" + str(i % 97),
                "qty": (i * 7) % 50 + 1,
                "price": (i * 13) % 200 + 5,
                "active": (i % 3) != 0,
            }
        )
        i += 1

    payload = {
        "items": items,
        "meta": {"source": "bench", "batch": 42, "region": "NA"},
    }

    total = 0
    outer = 0
    while outer < 10:
        text = json.dumps(payload)
        obj = json.loads(text)
        total += obj["items"][0]["qty"]
        total += obj["items"][10]["price"]
        outer += 1

    print(total)


if __name__ == "__main__":
    main()
