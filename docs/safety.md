# Safety Notes

This bridge can write configuration reports to your keyboard. Use it only if you
are comfortable testing compatibility tooling against your device.

OTA is deliberately blocked. Firmware flashing needs a device-specific Linux
BLE/GATT implementation and review before it should be enabled.

The included udev rule uses `MODE="0660"` with `TAG+="uaccess"`, allowing the
active local seat user to access the keyboard hidraw node without making it
world-readable. If your distro does not apply `uaccess` ACLs, create a dedicated
group and add `GROUP="your-group"` to the rule instead of relaxing it to `0666`.
