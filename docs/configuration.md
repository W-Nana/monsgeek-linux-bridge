# Configuration

The bridge is configured with environment variables.

| Variable | Default | Purpose |
| --- | --- | --- |
| `MONSGEEK_HIDRAW` | auto-detected, fallback `/dev/hidraw4` | Override the Linux hidraw endpoint |
| `MONSGEEK_VENDOR_HIDRAWS` | all MonsGeek `3151:502d` hidraw nodes | Comma-separated hidraw nodes to monitor for vendor/input events |
| `MONSGEEK_DEVICE_ID` | read from keyboard with `GET_INFOR` | Override the web-driver device ID while testing |
| `MONSGEEK_TRACE_HID` | unset | Set to `1` to log full 64-byte HID reports |
| `MONSGEEK_TRACE_FOCUS` | unset | Set to `1` to log only vendor-input and magnet calibration/simulation events |
| `MONSGEEK_DB_FILE` | `~/.local/state/monsgeek-linux-bridge/db.json` | Override the local JSON DB path |
| `MONSGEEK_CALIBRATION_INPUT_CACHE` | `1` | Cache hardware magnet travel events while `e5 fe` calibration reads are active |
| `MONSGEEK_CALIBRATION_INPUT_CACHE_TTL_MS` | `2500` | Time window for calibration travel max retention after the last `e5 fe` poll |
| `MONSGEEK_CALIBRATION_PHYSICAL_INPUT_GRACE_MS` | `700` | Require a nearby boot-keyboard input report before accepting calibration travel |
| `MONSGEEK_CALIBRATION_INPUT_STABILIZE_MS` | `180` | Ignore initial calibration vendor-input samples after entering calibration/max mode |
| `MONSGEEK_CALIBRATION_INPUT_CONFIRM_MS` | `90` | Require a second same-key travel sample before accepting a calibration max update |
| `MONSGEEK_CALIBRATION_PRESS_SELECT_MS` | `45` | Per-keypress candidate window used to lock calibration to one dominant key |
| `MONSGEEK_MAC_SEND_SETTLE_MS` | `0` | Optional macOS-driver-observed delay around feature writes |
| `MONSGEEK_MAC_READ_POLL_MS` | `0` | Optional macOS-driver-observed poll interval for transient calibration reads |
| `MONSGEEK_MAC_READ_POLL_ATTEMPTS` | `1` | Optional retry count for transient calibration reads |
| `MONSGEEK_ALLOW_OTA` | unset | Reserved. OTA still refuses even when set today |

Example with explicit hidraw and verbose report logging:

```sh
MONSGEEK_HIDRAW=/dev/hidraw4 MONSGEEK_TRACE_HID=1 cargo run --release
```

Focused calibration/simulation logging:

```sh
MONSGEEK_TRACE_FOCUS=1 cargo run --release
```
