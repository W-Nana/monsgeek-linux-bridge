use anyhow::{anyhow, bail, Context, Result};
use std::{
    env, fs,
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
    let mut group = install_group();
    for arg in args {
        match arg.as_str() {
            "--no-reload" => reload = false,
            "--no-group" => group = None,
            "-h" | "--help" => {
                print_install_help();
                return Ok(());
            }
            _ if arg.starts_with("--group=") => {
                group = Some(arg.trim_start_matches("--group=").to_string())
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
    let rule_text = rule_text(group.as_deref());
    fs::write(path, &rule_text).with_context(|| format!("write {RULE_FILE}"))?;
    println!("Installed {RULE_FILE}");
    if let Some(group) = group {
        println!(
            "Using GROUP=\"{group}\" fallback because uaccess ACLs are not reliable on every setup"
        );
    }

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
        "Usage:\n  monsgeek-linux-bridge install-udev [--group=GROUP] [--no-group] [--no-reload]\n\n\
Options:\n  --group=GROUP  Add GROUP=\"GROUP\" to the udev rule\n  --no-group     Rely only on TAG+=\"uaccess\"\n  --no-reload    Write the rule but skip udevadm reload/trigger"
    );
}

pub fn rule_text(group: Option<&str>) -> String {
    let group = group
        .filter(|name| !name.is_empty())
        .map(|name| format!(", GROUP=\"{name}\""))
        .unwrap_or_default();
    format!(
        "# MonsGeek vendor HID feature interface for the connected 3151:502d keyboard.\n\
SUBSYSTEM==\"hidraw\", ATTRS{{idVendor}}==\"3151\", ATTRS{{idProduct}}==\"502d\", MODE=\"0660\"{group}, TAG+=\"uaccess\"\n"
    )
}

fn install_group() -> Option<String> {
    let gid = env::var("SUDO_GID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_else(|| unsafe { libc::getgid() });
    group_name(gid).or_else(|| env::var("SUDO_USER").ok())
}

fn group_name(gid: u32) -> Option<String> {
    fs::read_to_string("/etc/group")
        .ok()?
        .lines()
        .find_map(|line| {
            let mut parts = line.split(':');
            let name = parts.next()?;
            let _password = parts.next()?;
            let line_gid = parts.next()?.parse::<u32>().ok()?;
            (line_gid == gid).then(|| name.to_string())
        })
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
