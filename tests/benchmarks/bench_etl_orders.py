from dataclasses import dataclass


@dataclass
class Order:
    order_id: int
    region: str
    qty: int
    price: int
    status: str


def parse_int(text: str) -> int:
    value = 0
    i = 0
    while i < len(text):
        value = value * 10 + (ord(text[i]) - 48)
        i += 1
    return value


def main() -> None:
    regions: list[str] = ["NA", "EU", "APAC", "LATAM"]
    statuses: list[str] = ["new", "paid", "shipped", "refunded"]
    rows: list[str] = []
    i = 0
    while i < 800:
        region: str = regions[i % len(regions)]
        status: str = statuses[i % len(statuses)]
        qty: int = (i * 7) % 50 + 1
        price: int = (i * 13) % 200 + 5
        rows.append(
            str(i) + "|" + region + "|" + str(qty) + "|" + str(price) + "|" + status
        )
        i += 1

    totals: dict[str, int] = {}
    grand_total = 0
    outer = 0
    while outer < 60:
        orders: list[Order] = []
        idx = 0
        while idx < len(rows):
            fields: list[str] = rows[idx].split("|")
            order = Order(
                parse_int(fields[0]),
                fields[1],
                parse_int(fields[2]),
                parse_int(fields[3]),
                fields[4],
            )
            orders.append(order)
            idx += 1
        for order in orders:
            if order.status == "refunded":
                continue
            revenue: int = order.qty * order.price
            grand_total += revenue
            totals[order.region] = totals.get(order.region, 0) + revenue
        outer += 1

    print(grand_total + sum(totals.values()))


if __name__ == "__main__":
    main()
