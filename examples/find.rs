use snapd_rs::{FindQuery, Result, SnapdClient};

#[tokio::main]
async fn main() -> Result<()> {
    let client = SnapdClient::new();

    let snaps = client
        .find(FindQuery {
            query: Some("firefox".to_string()),
            ..Default::default()
        })
        .await?;
    for snap in snaps.iter() {
        println!("{snap:#?}");
    }

    Ok(())
}
