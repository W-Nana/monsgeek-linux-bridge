# Architecture

The official web bundle calls a local gRPC-Web service at:

```text
http://127.0.0.1:3814/driver.DriverGrpc/<method>
```

This bridge implements that local service in Rust. For real keyboard I/O, it
issues Linux `HIDIOCSFEATURE` and `HIDIOCGFEATURE` ioctls directly against the
vendor configuration hidraw endpoint. Vendor/input reports are consumed through
an event-driven Tokio `AsyncFd` reader.

The Rust bridge removes the two largest prototype shortcuts:

- HID feature reports are handled in-process. There is no per-request fork/exec
  C helper, no command-line hex transport, and no binary-to-string-to-binary
  conversion.
- Vendor/input events wait on kernel readiness instead of polling hidraw every
  10 ms with synchronous reads.

`watchVender` keeps a real gRPC-Web text stream open and broadcasts
hardware-originated simulation/calibration events to subscribed web clients.

On the current FUN60 PRO, the vendor configuration endpoint is:

- USB vendor/product: `3151:502d`
- HID usage page: `0xffff`
- Feature report size: 64 bytes
- Observed interface: interface 2

The bridge auto-detects this endpoint from `/sys/class/hidraw`. Use
`MONSGEEK_HIDRAW=/dev/hidrawN` if auto-detection chooses the wrong node.
