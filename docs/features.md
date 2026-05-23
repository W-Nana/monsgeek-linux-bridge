# Feature Matrix

Implemented for API compatibility, but not confirmed as keyboard hardware
operations:

- Weather response
- Linux system-info stream event
- Microphone mute state compatibility
- Wireless-loop acknowledgement
- `cleanDev` acknowledgement

Intentionally not implemented:

- `upgradeOTAGATT` firmware flashing. The bridge returns a valid progress
  response with an error string and does not write firmware bytes.

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
| `getWeather` | Synthetic compatibility response | Network helper only |
| `watchSystemInfo` | Implemented as a live gRPC-Web stream with Linux system data | Host telemetry only |
| Microphone RPCs | In-memory compatibility state | No keyboard write evidence |
| `watchVender` | Implemented as a live gRPC-Web stream plus Linux hidraw input reader | Mirrors bridge-known events and forwards hardware vendor/input events |
| `changeWirelessLoopStatus` | Acknowledged | Internal loop/lock control |
| `cleanDev` | Acknowledged | Local connector state cleanup only |
| `upgradeOTAGATT` | Refused by design | Not implemented |
