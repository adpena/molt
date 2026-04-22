"""GPU DataFrame example — cuDF-style data processing."""

from molt.gpu.dataframe import DataFrame


def main():
    # Create DataFrame
    data = DataFrame(
        {
            "price": [10.5, 20.3, 15.7, 30.2, 25.1, 12.0, 28.5, 18.9],
            "quantity": [100, 200, 150, 300, 250, 120, 280, 180],
            "category": ["A", "B", "A", "C", "B", "A", "C", "B"],
        }
    )

    print("DataFrame:")
    print(data)
    print(f"\nShape: {data.shape}")
    print(f"Columns: {data.columns}")
    print()

    # Filter
    expensive = data.filter(data["price"] > 20.0)
    print("Expensive items (price > 20):")
    print(expensive)
    print()

    # Computed column
    data["revenue"] = data["price"] * data["quantity"]
    print("With revenue column:")
    print(data)
    print()

    # Group by + aggregate
    by_cat = data.group_by("category").agg(
        total_revenue=("revenue", "sum"),
        avg_price=("price", "mean"),
        count=("price", "count"),
    )
    print("By category:")
    print(by_cat)
    print()

    # Sort
    sorted_data = data.sort("revenue", descending=True)
    print("Sorted by revenue (desc):")
    print(sorted_data)
    print()

    # Describe
    print("Statistics:")
    for col, stats in data.describe().items():
        print(f"  {col}: {stats}")

    # CSV round-trip
    csv_text = data.to_csv()
    print("\nCSV output:")
    print(csv_text[:200])


if __name__ == "__main__":
    main()
