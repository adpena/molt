import codecs
import os


def dump_encoding(label: str, sample: str) -> None:
    info = codecs.lookup(label)
    print(label, "->", info.name)
    encoded = sample.encode(label)
    print("encode", label, list(encoded))
    print("decode", label, encoded.decode(label))
    print("byte80", label, ord(bytes([0x80]).decode(label)))
    try:
        "𝄞".encode(label)
    except UnicodeEncodeError as exc:
        print("error", label, exc.encoding, exc.reason)


def dump_file_encoding(label: str, sample: str) -> None:
    path = f"_molt_io_charmap_{label}.txt"
    try:
        with open(path, "w", encoding=label) as handle:
            handle.write(sample)
        with open(path, "rb") as handle:
            data = handle.read()
        print("file", label, list(data))
    finally:
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass


for name, sample in (
    ("cp437", "Café"),
    ("cp850", "Café"),
    ("cp860", "Olá"),
    ("cp862", "שלום"),
    ("cp863", "Québec"),
    ("cp865", "ÆØÅæøå"),
    ("cp866", "Привет"),
    ("cp874", "สวัสดี"),
    ("cp1250", "Český"),
    ("cp1251", "Привет"),
    ("cp1253", "Καλημέρα"),
    ("cp1254", "İstanbul"),
    ("cp1255", "שלום"),
    ("cp1256", "مرحبا"),
    ("cp1257", "Žalgiris"),
    ("koi8-r", "Привет"),
    ("koi8-u", "Привіт"),
    ("iso-8859-2", "Café"),
    ("iso-8859-3", "Ħalfa"),
    ("iso-8859-4", "Ąžuolas"),
    ("iso-8859-5", "Привет"),
    ("iso-8859-6", "مرحبا"),
    ("iso-8859-7", "Καλημέρα"),
    ("iso-8859-8", "שלום"),
    ("iso-8859-10", "Ångström"),
    ("iso-8859-15", "Café"),
    ("mac-roman", "Café"),
):
    dump_encoding(name, sample)
    dump_file_encoding(name, sample)
