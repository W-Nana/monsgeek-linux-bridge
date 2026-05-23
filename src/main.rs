mod bridge;
mod constants;
mod grpc;
mod smoke_test;
mod udev_installer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None | Some("serve") => bridge::run().await,
        Some("install-udev") => udev_installer::install(args.collect()),
        Some("smoke-test") => smoke_test::run().await,
        Some("print-udev-rule") => {
            print!("{}", udev_installer::RULE_TEXT);
            Ok(())
        }
        Some("-h" | "--help" | "help") => {
            print_help();
            Ok(())
        }
        Some(command) => anyhow::bail!("unknown command `{command}`; run with --help"),
    }
}

fn print_help() {
    println!(
        "monsgeek-linux-bridge\n\n\
Usage:\n  \
monsgeek-linux-bridge [serve]\n  \
monsgeek-linux-bridge install-udev [--no-reload]\n  \
monsgeek-linux-bridge smoke-test\n  \
monsgeek-linux-bridge print-udev-rule\n\n\
Commands:\n  \
serve            Start the local gRPC-Web connector on 127.0.0.1:3814\n  \
install-udev     Install the MonsGeek hidraw udev rule and reload udev\n  \
smoke-test       Call the local connector's 21 known DriverGrpc methods\n  \
print-udev-rule  Print the bundled udev rule"
    );
}
