"""Purpose: differential coverage for metaclass __getattribute__ ordering."""

class Meta(type):
    def __getattribute__(cls, name):
        if name == "special":
            return "meta_special"
        return super().__getattribute__(name)


class Demo(metaclass=Meta):
    special = "class_special"


if __name__ == "__main__":
    print("special", Demo.special)
