mod bridge;
mod constants;
mod grpc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bridge::run().await
}
