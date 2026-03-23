from faker import Faker

fake = Faker()
Faker.seed(42)

print("faker", Faker().seed_instance)
name = fake.name()
print("name type:", type(name).__name__)
print("name nonempty:", len(name) > 0)
email = fake.email()
print("email has @:", "@" in email)
