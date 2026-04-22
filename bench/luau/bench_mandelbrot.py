def mandelbrot(width: int, height: int, max_iter: int) -> int:
    count: int = 0
    y: int = 0
    while y < height:
        x: int = 0
        while x < width:
            cr: float = (2.0 * x / width) - 1.5
            ci: float = (2.0 * y / height) - 1.0
            zr: float = 0.0
            zi: float = 0.0
            i: int = 0
            while i < max_iter:
                tr: float = zr * zr - zi * zi + cr
                zi = 2.0 * zr * zi + ci
                zr = tr
                if zr * zr + zi * zi > 4.0:
                    break
                i = i + 1
            if i == max_iter:
                count = count + 1
            x = x + 1
        y = y + 1
    return count


def main() -> None:
    result: int = mandelbrot(200, 200, 50)
    print(result)


main()
