# Safety Notes

This bridge can write configuration reports to your keyboard. Use it only if you
are comfortable testing compatibility tooling against your device.

OTA is deliberately blocked. Firmware flashing needs a device-specific Linux
BLE/GATT implementation and review before it should be enabled.

The included udev rule uses `MODE="0660"` with `TAG+="uaccess"`, allowing the
active local seat user to access the keyboard hidraw node without making it
world-readable. If your distro does not apply `uaccess` ACLs, create a dedicated
group and add `GROUP="your-group"` to the rule instead of relaxing it to `0666`.

Install the bundled rule with:

```sh
cargo build --release
sudo ./target/release/monsgeek-linux-bridge install-udev
```

The command writes `/etc/udev/rules.d/99-monsgeek-hidraw.rules`, reloads udev
rules, and triggers hidraw devices. Use `--no-reload` if you only want to write
the rule file.

By default, the installer adds `GROUP="<sudo user's primary group>"` as a
fallback while keeping `MODE="0660"` and `TAG+="uaccess"`. This avoids the
`0666` keylogger-shaped footgun, but still works on desktops where `uaccess`
tags are present without actual ACLs. Use `--group=input` or another explicit
group if you prefer a dedicated access group; use `--no-group` to rely only on
`uaccess`.

Do not use `sudo cargo run --release -- install-udev`. Running Cargo with sudo
can leave `target/` owned by root and break later non-root builds.
