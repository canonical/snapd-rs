use snapd_rs::{Result, SnapdClient};

#[tokio::main]
async fn main() -> Result<()> {
    let client = SnapdClient::new();

    let snaps = client.installed_snaps(None).await?;
    for snap in snaps.iter() {
        println!("{snap:#?}");
    }

    Ok(())
}
