# Raspberry Pi server
Primarily this project written to cover my own needs, but you can find something useful for
yourself.

## Cross-compilation on Debian-based systems
First you need to install the build dependencies for Bluetooth and Udev libraries.

```
# dpkg --add-architecture arm64
# apt update
# apt install libdbus-1-dev:arm64 libudev-dev:arm64
```

After that you can build the binary.

```
$ export PKG_CONFIG_SYSROOT_DIR=/usr/aarch64-linux-gnu
$ cargo build --target aarch64-unknown-linux-gnu
```

## Running
To run the server you must set some required parameters. It can be achieved in two ways.
1. By settings environment variables with the `RPI_` prefix.
2. By putting values inside the `/etc/rpi-server.yaml` configuration file.

### Configuration
Required parameters described with the `[REQUIRED]` keyword, others are optional.

```yaml
# /etc/rpi-server.yaml

# Address to bind the server to
server_address: 0.0.0.0
# Port which used to bind the server
server_port: 80
# Log filtering. Can be: DEBUG, INFO, WARN, ERROR or another value
log_filter: INFO
# Directory where to store user preferences and database.
data_dir: /var/lib/rpi-server
# If string is specified, requests to the server will require
# authentication with this Bearer Token
access_token: null
# [REQUIRED] Directory with static files to host on "/"
site_path: /path/to/site

# Bluetooth-related parameters
bluetooth:
  # How long to perform the discovery
  discovery_seconds: 5
  # Name of Bluetooth adapter to use for the devices discovering
  adapter_name: null
  # [REQUIRED] MAC address of Xiaomi Mi Temperature and Humidity Monitor 2 (LYWSD03MMC)
  lounge_temp_mac_address: FF:00:FF:00:FF:00

# [OPTIONAL] Hotspot information.
# If this section is not null, all child parameters must be defined.
#
# Hotspot is a device that shares the internet using Wi-Fi. But the same device can connect to
# Raspberry Pi via Bluetooth, for example, to stream the audio. And if the same device will do these
# two operations simultaneously, stability of the audio streaming will be bad. So, we temporary
# disconnect from the Wi-Fi access point while the device connected to us via Bluetooth.
hotspot:
  # [REQUIRED] NetworkManager connection. Can be one of: ID (name), UUID or path.
  connection: AP
  # [REQUIRED] Bluetooth MAC address of the hotpost device.
  bluetooth_mac_address: FF:00:FF:00:FF:00
```
