# Confirmed UI Flows

These flows were exercised through the official web UI on the current FUN60 PRO:

- Light page: toggling the current `Dazzle` checkbox called `setLightType` and
  then sent a `sendMsg` report beginning with `07 01 04 03`. Reverting changed
  the observed report byte from `07` back to `08`.
- Simulation/vendor stream: the web UI consumes `VenderMsg.msg.slice(1, 4)`.
  The Linux bridge now emits the same shapes for start/stop (`0f 01 00` /
  `0f 00 00`), light changes (`04 xx 00`), and magnet travel notifications
  (`1b lo hi index`). Hardware-originated magnet travel events are forwarded
  from the hidraw input stream without synthetic replacement. Set
  `MONSGEEK_VENDOR_INPUT_READER=0` to disable that reader.
- Main remap page: selecting `r_Ctrl`, capturing `ArrowRight`, and pressing
  `Confirm` wrote a report beginning with `0a 00 53`.
- FnSetting page: selected-key reset and restore used reports beginning with
  `10 00 00 53`.
- Macro assignment: assigning a temporary `Macro_1` produced a remap report
  beginning with `0a 00 53` and an additional macro payload beginning with
  `0b 00 00 38`.
- Calibration: the official UI reads travel data with paged `e5 fe` reports.
  On Linux, the feature read can return zeros while the keyboard still emits
  real magnet travel on the vendor input hidraw stream. The bridge only
  preserves per-key maxima during the second calibration screen (`1e` / maximum
  mode), starts the vendor travel stream only after an actual boot-keyboard press
  report, locks each physical press to one dominant vendor key candidate,
  requires two same-key samples before accepting a max update, feeds that back
  through the paged `readMsg` response, and clears it when the polling window
  ends. The older read-payload max-hold shim remains disabled by default; enable
  only for diagnostics with `MONSGEEK_CALIBRATION_HOLD=1`.
