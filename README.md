# monsgeek-linux-bridge

Experimental Linux connector for the legacy MonsGeek web driver at
`https://web.monsgeek.com`.

The official web driver expects a local `iot_driver` connector on
`http://127.0.0.1:3814`. MonsGeek provides that connector for macOS, but not
for Linux. This project implements a small Linux gRPC-Web bridge that speaks the
same local API and forwards confirmed keyboard operations to the Linux HID
feature-report interface.

This is not an official MonsGeek project, and it is not a kernel driver. It is a
user-space compatibility bridge for testing and development.

## Status

Tested with a MonsGeek FUN60 PRO visible on Linux as USB `3151:502d`.

Working and hardware-verified on the current test keyboard:

- Device discovery through `watchDevList`
- 64-byte HID feature-report write/read through `sendMsg` and `readMsg`
- Raw feature send/read compatibility
- Lighting writes triggered by the official web UI
- Main-layer remapping
- Fn-layer remapping
- Macro storage and macro assignment
- Full calibration flow through the official web UI
- Local DB methods used by the web driver
- API-compatible responses for all 21 currently exposed `DriverGrpc` methods

Implemented for API compatibility, but not confirmed as keyboard hardware
operations:

- Weather response
- Linux system-info stream event
- Microphone mute state compatibility
- Empty vendor-event stream placeholder
- Wireless-loop acknowledgement
- `cleanDev` acknowledgement

Intentionally not implemented:

- `upgradeOTAGATT` firmware flashing. The bridge returns a valid progress
  response with an error string and does not write firmware bytes.

## Quick Start

Install the udev rule so your user can read and write the MonsGeek hidraw node:

```sh
sudo install -m 0644 udev/99-monsgeek-hidraw.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Replug the keyboard after installing the rule.

Inspect detected HID endpoints:

```sh
node tools/hid-sysfs-probe.mjs
```

Start the local connector:

```sh
node tools/grpc-web-probe.mjs
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
MONSGEEK_DEVICE_ID=2304 MONSGEEK_HIDRAW=/dev/null node tools/grpc-web-probe.mjs
MONSGEEK_HIDRAW=/dev/null node tools/grpc-smoke-test.mjs
```

The HID send/read methods will report ioctl errors inside their protobuf
payloads, but the gRPC-Web compatibility surface should still pass.

## Configuration

The bridge is configured with environment variables:

| Variable | Default | Purpose |
| --- | --- | --- |
| `MONSGEEK_HIDRAW` | auto-detected, fallback `/dev/hidraw4` | Override the Linux hidraw endpoint |
| `MONSGEEK_VENDOR_HIDRAWS` | all MonsGeek `3151:502d` hidraw nodes | Comma-separated hidraw nodes to monitor for vendor/input events |
| `MONSGEEK_DEVICE_ID` | read from keyboard with `GET_INFOR` | Override the web-driver device ID while testing |
| `MONSGEEK_TRACE_HID` | unset | Set to `1` to log full 64-byte HID reports |
| `MONSGEEK_TRACE_FOCUS` | unset | Set to `1` to log only vendor-input and magnet calibration/simulation events |
| `MONSGEEK_DB_FILE` | `~/.local/state/monsgeek-linux-bridge/db.json` | Override the local JSON DB path |
| `MONSGEEK_FEATURE_IO` | `/tmp/hid-feature-io` | Override the compiled C HID helper path |
| `MONSGEEK_LIVE_WEATHER` | unset | Set to `1` to call the same weather endpoint found in the macOS connector |
| `MONSGEEK_ALLOW_OTA` | unset | Reserved. OTA still refuses even when set today |

Example with explicit hidraw and verbose report logging:

```sh
MONSGEEK_HIDRAW=/dev/hidraw4 MONSGEEK_TRACE_HID=1 node tools/grpc-web-probe.mjs
```

Focused calibration/simulation logging:

```sh
MONSGEEK_TRACE_FOCUS=1 node tools/grpc-web-probe.mjs
```

## How It Works

The official web bundle calls a local gRPC-Web service at:

```text
http://127.0.0.1:3814/driver.DriverGrpc/<method>
```

This bridge implements that local service in Node.js. For real keyboard I/O, it
uses a small C helper to issue Linux `HIDIOCSFEATURE` and `HIDIOCGFEATURE`
ioctls against the vendor configuration hidraw endpoint.

On the current FUN60 PRO, the vendor configuration endpoint is:

- USB vendor/product: `3151:502d`
- HID usage page: `0xffff`
- Feature report size: 64 bytes
- Observed interface: interface 2

The bridge auto-detects this endpoint from `/sys/class/hidraw`. Use
`MONSGEEK_HIDRAW=/dev/hidrawN` if auto-detection chooses the wrong node.

## Feature Matrix

| Area | Linux bridge status | Hardware effect |
| --- | --- | --- |
| Device discovery | Implemented | Real local device enumeration |
| `sendMsg` / `readMsg` | Implemented and verified | Real USB HID feature-report I/O |
| `sendRawFeature` / `readRawFeature` | Implemented | Real USB HID feature-report I/O |
| Lighting writes | Implemented through HID reports | Real keyboard write |
| `setLightType` RPC | Implemented as official protobuf decode plus event-state update | Starts/stops official light/simulation event state; pixel/audio streaming is still connector-side compatibility |
| Main remap | Implemented and verified | Real keyboard write |
| Fn remap | Implemented and verified | Real keyboard write |
| Macro DB editing | Implemented | Local DB only |
| Macro assignment | Implemented and verified | Real keyboard write |
| Calibration | Implemented and verified | Real keyboard read/write flow |
| Local DB RPCs | Implemented as JSON DB | Local connector persistence only |
| `getWeather` | Synthetic by default, optional live fetch | Network helper only |
| `watchSystemInfo` | Implemented as a live gRPC-Web stream with Linux system data | Host telemetry only |
| Microphone RPCs | In-memory compatibility state | No keyboard write evidence |
| `watchVender` | Implemented as a live gRPC-Web stream plus Linux hidraw input reader | Mirrors bridge-known events and forwards hardware vendor/input events |
| `changeWirelessLoopStatus` | Acknowledged | Internal loop/lock control |
| `cleanDev` | Acknowledged | Local connector state cleanup only |
| `upgradeOTAGATT` | Refused by design | Not implemented |

## Reverse-Engineering Notes

The official `iot_v189.dmg` contains `iotdriver.app` with two Mach-O binaries:

- `Contents/MacOS/rust-iot-tray`
- `Contents/Resources/iot_driver`

`rust-iot-tray` is only a tray launcher. The real connector is
`Contents/Resources/iot_driver`, an x86_64 Rust user-space service using tonic
gRPC-Web on `127.0.0.1:3814`.

The macOS connector keeps enough Rust symbols and strings to identify its
major modules:

- `grpc_server`
- `dj_dev_manger`
- `dj_dev_api`
- `dj_hid_device`
- `ble_upgrade`
- `dj_sound`
- `dj_screen`
- `system_info`
- `get_weather`

Those symbols and imports show:

- HIDAPI is used for USB feature reports.
- CoreBluetooth/btleplug is used for BLE OTA/GATT.
- CPAL/CoreAudio is used for audio-related helpers.
- screen-capture support exists for screen/light features.
- sled is used for local key/value persistence.
- reqwest is used for weather HTTP requests.

Useful macOS binary evidence:

| API area | macOS evidence | Assessment |
| --- | --- | --- |
| HID write/read | `DJDevApi::send_msg`, `read_msg`, `send_msg_by_dev_path`, `read_msg_by_dev_path` | Real keyboard feature-report path |
| Lighting | `handle_light`, `check_light_type`, plus `LIGHT TYPE == not yet implemented` for the RPC wrapper | Actual writes happen through HID reports, not `setLightType` alone |
| Key matrix and macros | `FEA_CMD_SET_KEYMATRIX`, `FEA_CMD_SET_MACRO`, `FEA_CMD_GET_MACRO` | Real keyboard configuration commands |
| Weather | `src/get_weather.rs`, reqwest, `http://w2.yiketianqi.com/` URL | Network helper |
| System info | `src/system_info/mod.rs`, `SystemInfoManger::get`, `machdep.cpu.vendor` | Host telemetry |
| Microphone/audio | AudioUnit/CoreAudio imports, `dj_sound`, `CpalSound`, `ToggleMicrophoneMute` | OS/audio helper, not proven keyboard write |
| Vendor stream | `start_watch_vender`, `handle_vender`, `VenderMsg` variants | Real async event stream on macOS |
| Wireless loop | `LOCK_WIRELESS_LOOP`, `get_wireless_lock_status`, `set_lock_all_dev` | Internal connector lock/loop control |
| Clean device | `DJDeviceManger::clean_dev`, `clean_all_vendor`, `CLEAN DEV` strings | Local connector state cleanup |
| OTA | `src/ble_upgrade/mod.rs`, CoreBluetooth/btleplug, GATT UUIDs, `RY KB OTA` | Real BLE firmware flashing path; not ported |

## Confirmed UI Flows

These flows were exercised through the official web UI on the current FUN60 PRO:

- Light page: toggling the current `Dazzle` checkbox called `setLightType` and
  then sent a `sendMsg` report beginning with `07 01 04 03`. Reverting changed
  the observed report byte from `07` back to `08`.
- Simulation/vendor stream: the web UI consumes `VenderMsg.msg.slice(1, 4)`.
  The Linux bridge now emits the same shapes for start/stop (`0f 01 00` /
  `0f 00 00`), light changes (`04 xx 00`), and magnet travel notifications
  (`1b lo hi index`). Hardware-originated magnet travel events are forwarded
  from the hidraw input stream without synthetic replacement. Set
  `MONSGEEK_VENDOR_INPUT_READER=0` to disable that reader. A synthetic fallback
  can be enabled for UI debugging with `MONSGEEK_SYNTHETIC_SIMULATION=1`; its
  ceiling defaults to `400` and can be tuned with `MONSGEEK_SIM_TRAVEL_MAX`.
- Main remap page: selecting `r_Ctrl`, capturing `ArrowRight`, and pressing
  `Confirm` wrote a report beginning with `0a 00 53`.
- FnSetting page: selected-key reset and restore used reports beginning with
  `10 00 00 53`.
- Macro assignment: assigning a temporary `Macro_1` produced a remap report
  beginning with `0a 00 53` and an additional macro payload beginning with
  `0b 00 00 38`.
- Calibration: the official UI read travel data with `e5` reports, asked for
  all physical keys to be pressed to the bottom, then sent a final `1e` report.
  Repeated `e5 fe` polling stopped after confirmation. The experimental
  read-payload max-hold shim is disabled by default because it can create ghost
  travel values when the report layout is not confirmed; enable only for
  diagnostics with `MONSGEEK_CALIBRATION_HOLD=1`.

## Safety Notes

This bridge can write configuration reports to your keyboard. Use it only if
you are comfortable testing experimental tooling against your device.

OTA is deliberately blocked. Firmware flashing needs a device-specific Linux
BLE/GATT implementation and review before it should be enabled.

The included udev rule uses mode `0666` for this prototype because the test
machine received the `uaccess` tag without an ACL on the hidraw node. For a
multi-user machine, tighten the rule to a dedicated group.

## Repository Contents

```text
tools/grpc-web-probe.mjs       local gRPC-Web connector
tools/grpc-smoke-test.mjs      21-method API smoke test
tools/hid-sysfs-probe.mjs      Linux hidraw/sysfs inspector
tools/hid-feature-io.c         HID feature-report send/read helper
tools/hid-feature-version.c    narrow GET_REV feature-report test
udev/99-monsgeek-hidraw.rules  prototype udev rule for 3151:502d
```

## License and Disclaimer

No license has been selected yet.

This project is independent research and compatibility work. It is not
affiliated with or endorsed by MonsGeek. Use at your own risk.
