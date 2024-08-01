# Raspberry Pi server
Primarily this project written to cover my own needs, but you can find something useful for
yourself.

## Cross-compilation on Debian-based systems
First you need to install the build dependency for the Bluetooth library.
```
# dpkg --add-architecture arm64
# apt update
# apt install libdbus-1-dev:arm64
```

After that you can build the binary.
```
$ export PKG_CONFIG_SYSROOT_DIR=/usr/aarch64-linux-gnu
$ cargo build --target aarch64-unknown-linux-gnu
```
