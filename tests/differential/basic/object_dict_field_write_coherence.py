"""Purpose: materialized __dict__ stays coherent with field-backed attribute writes."""

class Demo:
    def __init__(self) -> None:
        self.note = "live"
        self.value = 7


item = Demo()
first_dict = item.__dict__
item.note = "updated"
print(item.note, sorted(first_dict.items()), sorted(item.__dict__.items()), first_dict is item.__dict__)
