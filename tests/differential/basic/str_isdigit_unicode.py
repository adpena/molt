"""Purpose: differential coverage for Unicode digit classification in str.isdigit."""

if __name__ == "__main__":
    samples = [
        "",
        "123",
        "\u0662",
        "\u00b2",
        "\u2155",
        "\uff11\uff12\uff13",
    ]
    for item in samples:
        print(repr(item), item.isdigit())
