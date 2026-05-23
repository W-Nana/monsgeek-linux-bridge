# MonsGeek Linux connector notes

This workspace contains the official macOS `iot_v189.dmg` and a Linux
gRPC-Web/HID bridge probe for the legacy web driver.

## Current findings

- `web.monsgeek.com` uses gRPC-Web against `http://127.0.0.1:3814`.
- The macOS package launches a Rust `iot_driver` local bridge for that port.
- The connected MonsGeek keyboard is visible on Linux as USB `3151:502d`.
- The vendor configuration HID interface is a 64-byte Feature Report hidraw
  endpoint: interface 2 and usage page `0xffff` on the current FUN60 PRO. The
  bridge detects the matching Linux endpoint from sysfs; it was `hidraw4` in
  the test environment.

## macOS driver reverse-engineering notes

`iot_v189.dmg` contains `iotdriver.app` with a tray launcher
`Contents/MacOS/rust-iot-tray` and the real connector binary
`Contents/Resources/iot_driver`. Both are x86_64 Mach-O executables. The tray
launcher only starts `iot_driver`; the connector is a user-space Rust service
using tonic gRPC-Web on `127.0.0.1:3814`, not a kernel driver.

The macOS connector binary keeps Rust symbols and source-path strings. Those
symbols confirm the major local modules: `grpc_server`, `dj_dev_manger`,
`dj_dev_api`, `dj_hid_device`, `ble_upgrade`, `dj_sound`, `dj_screen`,
`system_info`, and `get_weather`. The imports and strings show HIDAPI for USB
feature reports, CoreBluetooth/btleplug for BLE OTA/GATT, CPAL/CoreAudio for
audio spectrum/mic-related helpers, `scrap`-style screen capture support, sled
for a local key/value DB, and reqwest for weather HTTP.

| Function area | Linux bridge status | macOS reverse evidence | Hardware effect assessment |
| --- | --- | --- | --- |
| `watchDevList` | Implemented | mac symbols include `DJDeviceManger::refresh_dev_list`, HIDAPI, `SupportDev`, `vid`, `pid`, `usage`, and `hid_raw_path` | Real local device enumeration |
| `sendMsg` / `readMsg` | Implemented and verified on FUN60 PRO | mac symbols include `DJDevApi::send_msg`, `read_msg`, `send_msg_by_dev_path`, `read_msg_by_dev_path`; strings show `fb wait write/read` | Real 64-byte USB HID feature-report I/O |
| `sendRawFeature` / `readRawFeature` | Implemented | same HIDAPI/raw feature path as above | Real USB HID feature-report I/O |
| Lighting writes | Implemented through `sendMsg`; `setLightType` ack-compatible | mac binary has `setLightType` RPC but also the string `LIGHT TYPE == not yet implemented`; real light traffic is in `handle_light` and device API writes | Hardware effect comes from follow-up HID reports, not from `setLightType` alone |
| Main remap / Fn remap / macro assignment | Implemented and verified | mac binary exposes command names such as `FEA_CMD_SET_KEYMATRIX`, `FEA_CMD_SET_MACRO`, `FEA_CMD_GET_MACRO` | Real HID writes, verified on current keyboard |
| Calibration | Implemented through existing HID read/write flow; verified full UI flow | mac binary exposes generic HID report paths, no separate calibration RPC is present in the 21-method service | Real HID reports; observed `e5 fe` polling and final `1e` report |
| Local DB methods | Implemented as JSON DB | mac binary links sled and exposes `insertDb`, `getItemFromDb`, `getAllKeysFromDb`, `getAllValuesFromDb`, `deleteItemFromDb` | Local connector persistence only, not direct keyboard hardware |
| `getWeather` | Compatible response; optional live fetch with `MONSGEEK_LIVE_WEATHER=1` | mac binary contains `src/get_weather.rs`, reqwest, and `http://w2.yiketianqi.com/?version=day&unit=m&language=&query=&appid=48129135&appsecret=6Zojc6j0` | Network helper only; no keyboard hardware effect |
| `watchSystemInfo` | Implemented with Linux system data | mac binary contains `src/system_info/mod.rs`, `SystemInfoManger::get`, and system strings such as `machdep.cpu.vendor` | Host system telemetry only; no keyboard hardware effect |
| Microphone methods | API-compatible in-memory state | mac binary links AudioUnit/CoreAudio and has `dj_sound`, `CpalSound`, `ToggleMicrophoneMute` strings | mac side is an OS/audio helper; no evidence it writes the keyboard on this path |
| `watchVender` | Empty compatible stream event | mac symbols include `start_watch_vender`, `handle_vender`, broadcast channels, and `VenderMsg` variants `Music1`, `Music2`, `Screen`, `ToggleMicrophoneMute`, `StartLightSwitch`, `ProfileSwitch`, `ResetSystemSwitch` | Real vendor-event stream exists on mac; Linux bridge does not yet mirror async vendor events |
| `changeWirelessLoopStatus` | Acknowledged | mac symbols include `LOCK_WIRELESS_LOOP`, `get_wireless_lock_status`, and `set_lock_all_dev` | Internal loop/lock control; no direct HID write evidence from the RPC alone |
| `cleanDev` | Acknowledged without persistence | mac symbols include `DJDeviceManger::clean_dev`, `clean_all_vendor`, and strings `CLEAN DEV` / `CLEAN ALL VENDOR` | Clears local connector device/vendor state; not a keyboard programming operation |
| `upgradeOTAGATT` | Guarded and refused | mac binary contains `src/ble_upgrade/mod.rs`, CoreBluetooth/btleplug, GATT UUIDs, `RY KB OTA`, and success/checksum strings | Real BLE/GATT firmware flashing path on mac; intentionally not implemented on Linux bridge |

## Probes

List Linux HID endpoints without touching the device:

```sh
node tools/hid-sysfs-probe.mjs
```

Start a local gRPC-Web probe for the legacy web driver:

```sh
node tools/grpc-web-probe.mjs
```

Run a repeatable smoke test against a running bridge:

```sh
node tools/grpc-smoke-test.mjs
```

The gRPC-Web probe answers the connector version check, reports the local
`3151:502d` keyboard to the device stream, and forwards the confirmed
feature-report send/read calls through the Linux helper. The device stream uses
the driver's read-only `GET_INFOR` command (`0x8f`) to fetch the web driver
device ID from the keyboard before emitting the device list; on the current
keyboard that ID is `2304`. Set `MONSGEEK_DEVICE_ID` to override the detected ID
while testing. Set `MONSGEEK_HIDRAW=/dev/hidrawN` when sysfs detection needs an
override. The probe also acknowledges the webpage's wireless-loop call and
keeps the web driver's byte-keyed local DB in
`$XDG_STATE_HOME/monsgeek-linux-bridge/db.json` or
`~/.local/state/monsgeek-linux-bridge/db.json`. Set `MONSGEEK_DB_FILE` to use a
different DB file. It also permits Chrome's private-network preflight from
`https://web.monsgeek.com`.

The official web bundle currently exposes 21 `DriverGrpc` methods. The smoke
test exercises all of them and expects HTTP 200 plus `grpc-status:0`. The
confirmed local FUN60 PRO paths covered by the bridge are device discovery,
`sendMsg`/`readMsg`, raw feature send/read, the byte-keyed DB calls used by
macros, `setLightType`, wireless-loop acknowledgement, microphone mute state
compatibility, weather response compatibility, and a Linux system-info stream
event. `getWeather` returns an offline synthetic response by default; set
`MONSGEEK_LIVE_WEATHER=1` to call the same weather endpoint found in the macOS
connector and fall back to the offline payload on failure. `cleanDev` is
acknowledged without persistence and unsupported future methods are logged.

Set `MONSGEEK_TRACE_HID=1` when starting the probe to log whole 64-byte HID
reports instead of the first eight bytes. The light page already exercises the
write path: toggling the current `Dazzle` checkbox calls `setLightType` and then
sends a `sendMsg` report beginning with `07 01 04 03`. On the current FUN60 PRO,
the observed revertible toggle changed that report byte from `07` back to `08`
while the probe forwarded both reports to the vendor HID interface.

The main-page remap flow also reaches the write path through the same bridge.
Selecting the current `r_Ctrl` key, capturing `ArrowRight` in the `Remap` input,
and pressing `Confirm` wrote a `sendMsg` report beginning with `0a 00 53` and
restored the observed `r_Ctrl ->` mapping. The `Disable` action commits
immediately instead of waiting for `Confirm`; use a selected-key `Reset` or
reapply the prior remap before leaving a trace session that exercises it.

`FnSetting` reuses the same remap panel and existing bridge methods on the
keyboard's Fn layer. Its selected-key reset and restore trace used `sendMsg`
reports beginning with `10 00 00 53`: resetting the observed Fn-layer
`r_Ctrl ->` mapping cleared that report's ArrowRight value, and capturing
`ArrowRight` plus `Confirm` restored it.

The Macro page does not add a connector method during editing. `Add`, toggling
recording between `Record` and `Stop` without key events, and the macro
assignment-mode checkbox stayed in the page. Saving an empty test macro against
a temporary `MONSGEEK_DB_FILE` used `insertDb` for
`web_driver/iot_db/macro` and produced no HID report; the default DB was left
without that test macro.

Assigning that temporary `Macro_1` from the main remap panel to the observed
`r_Ctrl ->` key used the existing write bridge as well. `Confirm` sent a remap
report beginning with `0a 00 53` for the key and an additional macro payload
report beginning with `0b 00 00 38`; capturing `ArrowRight` and confirming it
afterward restored the original `r_Ctrl ->` mapping.

Opening Calibration reuses existing bridge calls rather than adding a connector
method. The full FUN60 PRO calibration flow was exercised through the official
UI: the page first reads travel data with `e5` reports, asks the user to press
every physical key to the bottom, then `Confirm` exits the calibration overlay.
The trace showed the expected repeated `e5 fe` polling and a final `1e` report;
polling stopped after the confirmation returned to the main page.

The visible Profile and Share pages reached account/cloud UI during this pass
without exposing another confirmed local FUN60 PRO connector call. About showed
firmware `ID2304_V309` and an Upgrade entry point. The bridge now has compatible
responses for the microphone, weather, vendor, and system-info methods present
in the official bundle.

`upgradeOTAGATT` is intentionally guarded. It returns a valid `Progress`
message with an error string and does not flash the keyboard. Set
`MONSGEEK_ALLOW_OTA=1` only after implementing and reviewing a device-specific
Linux OTA transport; with the flag set today, the bridge still refuses with a
clear "not implemented" progress error instead of writing firmware bytes.

Real feature-report traffic needs read/write access to the detected
`/dev/hidrawN`. The workspace includes
[udev/99-monsgeek-hidraw.rules](udev/99-monsgeek-hidraw.rules) for USB
`3151:502d`. Install and reload it with root privileges, then replug the
keyboard:

```sh
sudo install -m 0644 udev/99-monsgeek-hidraw.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

The rule uses mode `0666` for this prototype because this session received the
`uaccess` tag without an ACL on `/dev/hidraw4`. Tighten it to a dedicated group
after the connector path is working. On the current test machine it produces a
read/write `hidraw4` node and the feature-report helper below reaches the
keyboard.

Current Chrome builds may ask for local network access before the public
`https://web.monsgeek.com` page can call the loopback bridge at
`http://127.0.0.1:3814`. Denying that permission makes the page keep showing the
driver-support download prompt even while this bridge is running.

There is also a narrow feature-report test for the vendor HID interface. It only
sends the driver's `GET_REV` query (`0x80`) before reading a reply:

```sh
gcc -Wall -Wextra -o /tmp/hid-feature-version tools/hid-feature-version.c
/tmp/hid-feature-version /dev/hidraw4
```

The bridge compiles its feature-report helper into `/tmp/hid-feature-io` on the
first HID access when it is missing. `CC` selects another compiler and
`MONSGEEK_FEATURE_IO` selects another helper path:

```sh
node tools/grpc-web-probe.mjs
```
