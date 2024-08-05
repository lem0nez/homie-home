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

## Running
To run the server you must set the `ACCESS_TOKEN` environment variable and optionally provide
configuration settings. It can be achieved in the two ways.

1. By settings environment variables with the `RPI_` prefix.
2. By putting values inside the `/etc/rpi-server.yaml` configuration file.

Available options you can see in the [src/config.rs](src/config.rs) file.
