# Reverse-Engineering Notes

The official `iot_v189.dmg` contains `iotdriver.app` with two Mach-O binaries:

- `Contents/MacOS/rust-iot-tray`
- `Contents/Resources/iot_driver`

`rust-iot-tray` is only a tray launcher. The real connector is
`Contents/Resources/iot_driver`, an x86_64 Rust user-space service using tonic
gRPC-Web on `127.0.0.1:3814`.

The macOS connector keeps enough Rust symbols and strings to identify its major
modules:

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

The HID timing model has been partially identified from the macOS binary. The
`DJDevice::send_read_msg` path marks the device as reading/sending, waits 10 ms,
calls `send_msg`, waits another 10 ms, calls `read_msg`, then unsets the
reading/sending marker. The write/read helpers also call `pasue24loop` for 24G
state and `wait_fb_write_finish`, which polls `fb_24_check_status` every 100 ms
for up to 15 attempts. Those timings are exposed as optional environment
variables, but remain disabled by default because the Linux feature-report path
is already synchronous and the extra wait is visible in the web UI.
