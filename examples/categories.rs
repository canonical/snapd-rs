use snapd_rs::{SnapdClient, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let client = SnapdClient::new();

    let categories = client.snap_categories().await?;
    for cat in categories.iter() {
        println!("{cat}");
    }

    Ok(())
}

