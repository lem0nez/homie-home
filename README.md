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
To run the server you must set the `ACCESS_TOKEN` environment variable. Also you must provide some
required configuration settings. It can be achieved in the two ways.

1. By settings environment variables with the `RPI_` prefix.
2. By putting values inside the `/etc/rpi-server.yaml` configuration file.

## Example configuration
```yaml
# /etc/rpi-server.yaml

# Directory path with static files to host on "/"
site_path: /usr/local/share/rpi-control
bluetooth:
  # Name of Bluetooth adapter to use for the devices discovering
  adapter_name: Raspberry Pi
  # MAC address of Xiaomi Mi Temperature and Humidity Monitor 2 (LYWSD03MMC)
  mi_temp_mac_address: FF:00:FF:00:FF:00
```
