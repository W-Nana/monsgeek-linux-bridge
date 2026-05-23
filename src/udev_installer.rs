use anyhow::{anyhow, bail, Context, Result};
use std::{
    fs,
    path::Path,
    process::{Command, ExitStatus},
};

pub const RULE_FILE: &str = "/etc/udev/rules.d/99-monsgeek-hidraw.rules";
pub const RULE_TEXT: &str = concat!(
    "# MonsGeek vendor HID feature interface for the connected 3151:502d keyboard.\n",
    "SUBSYSTEM==\"hidraw\", ATTRS{idVendor}==\"3151\", ATTRS{idProduct}==\"502d\", MODE=\"0660\", TAG+=\"uaccess\"\n",
);

pub fn install(args: Vec<String>) -> Result<()> {
    let mut reload = true;
    for arg in args {
        match arg.as_str() {
            "--no-reload" => reload = false,
            "-h" | "--help" => {
                print_install_help();
                return Ok(());
            }
            _ => bail!("unknown install-udev option `{arg}`"),
        }
    }

    if running_as_root() && parent_process_name().is_some_and(|name| name == "cargo") {
        bail!(
            "do not run `sudo cargo run --release -- install-udev`; it makes target/ root-owned. Run `cargo build --release` as your user, then `sudo ./target/release/monsgeek-linux-bridge install-udev`"
        );
    }

    if !running_as_root() {
        bail!(
            "install-udev must write {RULE_FILE}; run `cargo build --release` first, then `sudo ./target/release/monsgeek-linux-bridge install-udev`"
        );
    }

    let path = Path::new(RULE_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, RULE_TEXT).with_context(|| format!("write {RULE_FILE}"))?;
    println!("Installed {RULE_FILE}");

    if reload {
        run_udevadm(&["control", "--reload-rules"])?;
        run_udevadm(&["trigger", "--subsystem-match=hidraw"])?;
        println!("Reloaded udev rules and triggered hidraw devices");
        println!("Replug the keyboard if permissions do not update immediately");
    }

    Ok(())
}

fn print_install_help() {
    println!(
        "Usage:\n  monsgeek-linux-bridge install-udev [--no-reload]\n\n\
Options:\n  --no-reload  Write the rule but skip udevadm reload/trigger"
    );
}

fn running_as_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn parent_process_name() -> Option<String> {
    let ppid = unsafe { libc::getppid() };
    fs::read_to_string(format!("/proc/{ppid}/comm"))
        .ok()
        .map(|name| name.trim().to_string())
}

fn run_udevadm(args: &[&str]) -> Result<()> {
    let status = Command::new("udevadm")
        .args(args)
        .status()
        .with_context(|| format!("run udevadm {}", args.join(" ")))?;
    ensure_success(status, &format!("udevadm {}", args.join(" ")))
}

fn ensure_success(status: ExitStatus, command: &str) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{command} exited with {status}"))
    }
}
