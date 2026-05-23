# monsgeek-linux-bridge

Rust Linux connector for the legacy MonsGeek web driver at
`https://web.monsgeek.com`.

The official web driver expects a local `iot_driver` connector on
`http://127.0.0.1:3814`. MonsGeek provides that connector for macOS, but not for
Linux. This project implements a user-space gRPC-Web bridge that speaks the same
local API and forwards confirmed keyboard operations to Linux HID feature
reports.

This is independent compatibility work. It is not an official MonsGeek project
and it is not a kernel driver.

## Status

Tested with a MonsGeek FUN60 PRO visible on Linux as USB `3151:502d`.

Working and hardware-verified on the current test keyboard:

- Device discovery through `watchDevList`
- 64-byte HID feature-report write/read through `sendMsg` and `readMsg`
- Raw feature send/read compatibility
- Lighting writes triggered by the official web UI
- Main-layer and Fn-layer remapping
- Macro storage and macro assignment
- Full calibration flow through the official web UI
- Local DB methods used by the web driver
- API-compatible responses for all 21 currently exposed `DriverGrpc` methods

Not implemented by design:

- `upgradeOTAGATT` firmware flashing. The bridge returns a valid progress
  response with an error string and does not write firmware bytes.

See [docs/features.md](docs/features.md) for the full feature matrix.

## Quick Start

Install the udev rule so your active desktop user can read and write the
MonsGeek hidraw node:

```sh
sudo install -m 0644 udev/99-monsgeek-hidraw.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Replug the keyboard after installing the rule.

Build and start the local connector:

```sh
cargo run --release
```

Open the official web driver:

```text
https://web.monsgeek.com
```

If Chrome asks for local network access, allow it. Without that permission the
page may keep showing the driver download prompt even while this bridge is
running.

## Verification

With the bridge running, execute the smoke test:

```sh
node tools/grpc-smoke-test.mjs
```

Expected result: all 21 methods return HTTP 200 and `grpc-status: 0`.

For a dry API-only smoke test without a real hidraw device:

```sh
MONSGEEK_DEVICE_ID=2304 MONSGEEK_HIDRAW=/dev/null cargo run --release
MONSGEEK_HIDRAW=/dev/null node tools/grpc-smoke-test.mjs
```

The HID send/read methods will report ioctl errors inside their protobuf
payloads, but the gRPC-Web compatibility surface should still pass.

## Documentation

- [Configuration](docs/configuration.md)
- [Architecture](docs/architecture.md)
- [Feature matrix](docs/features.md)
- [Confirmed UI flows](docs/ui-flows.md)
- [Reverse-engineering notes](docs/reverse-engineering.md)
- [Safety notes](docs/safety.md)

## Repository Layout

```text
src/main.rs                    thin Rust binary entrypoint
src/bridge.rs                  Rust local gRPC-Web connector
docs/                          split project documentation
tools/grpc-smoke-test.mjs      21-method API smoke test
tools/grpc-web-probe.mjs       legacy Node.js prototype / fallback connector
tools/hid-sysfs-probe.mjs      Linux hidraw/sysfs inspector
tools/hid-feature-io.c         legacy prototype HID feature-report helper
tools/hid-feature-version.c    narrow GET_REV feature-report test
udev/99-monsgeek-hidraw.rules  udev rule for 3151:502d
```

## License

No license has been selected yet. Use at your own risk.
