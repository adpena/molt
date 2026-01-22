# Linux packaging templates

These templates assume you already built a release bundle (tar.gz) and want
`.deb`/`.rpm` packages that install the bundle into `/usr/local/molt`.

## fpm (recommended)

```bash
VERSION=0.0.001
ARCH=x86_64
BUNDLE=molt-$VERSION-linux-$ARCH.tar.gz

mkdir -p /tmp/molt_pkg
 tar -xzf $BUNDLE -C /tmp/molt_pkg

fpm -s dir -t deb \
  -n molt \
  -v $VERSION \
  --license Apache-2.0 \
  --prefix /usr/local/molt \
  -C /tmp/molt_pkg/molt-$VERSION \
  .
```

Repeat for rpm with `-t rpm`.
