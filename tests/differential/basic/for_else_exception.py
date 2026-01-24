"""Purpose: differential coverage for for/else when exception raised."""


def main():
    try:
        for item in [1, 2]:
            if item == 2:
                raise ValueError("boom")
        else:
            print("else", "hit")
    except Exception as exc:
        print("error", type(exc).__name__)


if __name__ == "__main__":
    main()
