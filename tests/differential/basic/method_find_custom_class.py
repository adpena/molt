class Probe:
    def find(self, update=False):
        return 7 if update else 3


p = Probe()
print(p.find())
print(p.find(update=True))
