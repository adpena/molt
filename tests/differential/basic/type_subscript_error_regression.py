cls = type(3)

for label, thunk in (
    ("slice", lambda: cls[:1]),
    ("getitem", lambda: cls[int]),
):
    try:
        value = thunk()
        print(label, "ok", value)
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))

print("type_generic_alias", type[int])
