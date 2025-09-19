use crate::{response::parse_response, socket_client::UnixSocketClient};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use hyper::{StatusCode, Uri};
use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize};
use std::{collections::HashMap, env, str::FromStr};
use tracing::error;

mod response;
mod socket_client;

const SNAPD_BASE_URI: &str = "http://localhost/v2";
const SNAPD_SOCKET: &str = "/run/snapd.socket";
const SNAPD_SNAP_SOCKET: &str = "/run/snapd-snap.socket";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Hyper(#[from] hyper::Error),
    #[error(transparent)]
    HyperHttp(#[from] hyper::http::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("{uri} is not valid: {reason}")]
    InvalidUri { reason: &'static str, uri: String },
    #[error("error message returned from snapd: {message}")]
    SnapdError {
        status: StatusCode,
        message: String,
        kind: Option<String>,
        value: Option<serde_json::Value>,
    },
    #[error(transparent)]
    UrlEncoded(#[from] serde_urlencoded::ser::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Abstraction layer to make swapping out the underlying client possible for
/// testing.
#[async_trait]
pub trait Client {
    async fn get_json<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned + Send;

    async fn post_json<T, U>(&self, path: &str, body: U) -> Result<T>
    where
        T: DeserializeOwned + Send,
        U: Serialize + Send;
}

fn build_uri(path: &str) -> Result<Uri> {
    let s = format!("{SNAPD_BASE_URI}/{path}");
    Uri::from_str(&s).map_err(|_| Error::InvalidUri {
        reason: "malformed",
        uri: s,
    })
}

#[async_trait]
impl Client for UnixSocketClient {
    async fn get_json<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned + Send,
    {
        let uri = build_uri(path)?;
        let res = self.get(uri).await?;
        parse_response(res).await
    }

    async fn post_json<T, U>(&self, path: &str, body: U) -> Result<T>
    where
        T: DeserializeOwned + Send,
        U: Serialize + Send,
    {
        let uri = build_uri(path)?;
        let res = self
            .post(uri, "application/json", serde_json::to_vec(&body)?)
            .await?;
        parse_response(res).await
    }
}

#[derive(Debug, Clone)]
pub struct SnapdClient<C>
where
    C: Client,
{
    client: C,
}

pub type SnapdSocketClient = SnapdClient<UnixSocketClient>;

impl Default for SnapdSocketClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapdSocketClient {
    pub fn new() -> Self {
        let socket = if env::var("SNAP_NAME").is_ok() {
            SNAPD_SNAP_SOCKET
        } else {
            SNAPD_SOCKET
        };

        Self {
            client: UnixSocketClient::new(socket),
        }
    }
}

// Payloads for serialization, kept private to the module.
#[derive(Debug, Serialize)]
struct InstallPayload {
    action: &'static str,
    snaps: Vec<String>,
    classic: bool,
}

#[derive(Debug, Serialize)]
struct UninstallPayload {
    action: &'static str,
    purge: bool,
}

impl<C> SnapdClient<C>
where
    C: Client,
{
    pub async fn installed_snaps(&self, filter: Option<SnapsFilter>) -> Result<Vec<Snap>> {
        let uri = if let Some(filter) = filter {
            format!("snaps?select={}", filter.kebab_case())
        } else {
            "snaps".to_owned()
        };
        self.client.get_json(&uri).await
    }

    pub async fn snap_details(&self, snap: &str) -> Result<Snap> {
        self.client.get_json(&format!("snaps/{snap}")).await
    }

    pub async fn snap_categories(&self) -> Result<Vec<String>> {
        #[derive(Debug, Deserialize)]
        struct Category {
            name: String,
        }
        let res: Vec<Category> = self.client.get_json("categories").await?;
        Ok(res.into_iter().map(|c| c.name).collect())
    }

    pub async fn install_snaps(&self, snaps: Vec<String>, classic: bool) -> Result<ChangeId> {
        let payload = InstallPayload {
            action: "refresh",
            snaps,
            classic,
        };
        self.client.post_json("snaps", payload).await
    }

    pub async fn uninstall_snap(&self, snap: &str, purge: bool) -> Result<ChangeId> {
        let payload = UninstallPayload {
            action: "remove",
            purge,
        };
        self.client
            .post_json(&format!("snaps/{snap}"), payload)
            .await
    }

    pub async fn change_status(&self, change_id: &ChangeId) -> Result<ChangeStatus> {
        self.client
            .get_json(&format!("changes/{}", change_id.change))
            .await
    }

    pub async fn find(&self, query: FindQuery) -> Result<Vec<Snap>> {
        let query = serde_urlencoded::to_string(query)?;
        self.client.get_json(&format!("find?{query}")).await
    }
}

/// A snapd change ID which may be polled to check on the status of an ongoing
/// asynchronous operation
#[derive(Debug, Deserialize)]
pub struct ChangeId {
    change: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ChangeStatus {
    pub id: String,
    pub err: Option<String>,
    #[serde(default)]
    pub ready: bool,
    pub spawn_time: Option<DateTime<Utc>>,
    pub ready_time: Option<DateTime<Utc>>,
    pub kind: Option<String>,
    pub summary: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub tasks: Vec<SnapdTask>,
    #[serde(deserialize_with = "snap_names")]
    pub snap_names: Vec<String>,
}

fn snap_names<'de, D>(de: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct Raw {
        snap_names: Vec<String>,
    }
    let raw: Raw = Deserialize::deserialize(de)?;
    Ok(raw.snap_names)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SnapdTask {
    pub id: String,
    pub spawn_time: Option<DateTime<Utc>>,
    pub ready_time: Option<DateTime<Utc>>,
    pub kind: Option<String>,
    pub summary: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub progress: TaskProgress,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct TaskProgress {
    pub label: String,
    pub done: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapsFilter {
    All,
    Enabled,
    RefreshInhibited,
}

impl SnapsFilter {
    fn kebab_case(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Enabled => "enabled",
            Self::RefreshInhibited => "refresh-inhibited",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SnapPublisher {
    id: String,
    display_name: String,
    username: Option<String>,
    validation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SnapChannel {
    released_at: DateTime<Utc>,
    #[serde(default)]
    confinement: SnapConfinement,
    revision: Option<String>,
    #[serde(default)]
    size: u32,
    version: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SnapConfinement {
    #[default]
    Unknown,
    Strict,
    Devmode,
    Classic,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct SnapMedia {
    #[serde(rename = "type")]
    ty: String,
    url: String,
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RefreshInhibit {
    proceed_time: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Snap {
    id: String,
    name: String,
    version: String,
    channel: String,
    #[serde(rename = "type")]
    ty: String,
    // Should be i32 but we're just using a string for now while we work on the parser
    // #[serde(deserialize_with = "parse_revision")]
    revision: String,
    contact: Option<String>,
    description: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    channels: HashMap<String, SnapChannel>,
    #[serde(default)]
    tracks: Vec<String>,
    #[serde(default)]
    common_ids: Vec<String>,
    #[serde(default)]
    media: Vec<SnapMedia>,
    #[serde(default)]
    confinement: SnapConfinement,
    #[serde(default)]
    devmode: bool,
    #[serde(default)]
    jailmode: bool,
    #[serde(default)]
    private: bool,
    base: Option<String>,
    title: Option<String>,
    tracking_channel: Option<String>,
    website: Option<String>,
    license: Option<String>,
    mounted_from: Option<String>,
    store_url: Option<String>,
    hold: Option<DateTime<Utc>>,
    install_date: Option<DateTime<Utc>>,
    download_size: Option<u32>,
    publisher: Option<SnapPublisher>,
    refresh_inhibit: Option<RefreshInhibit>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindFilter {
    Refresh,
    Private,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindScope {
    Wide,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct FindQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<FindFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wide_scope: Option<FindScope>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use simple_test_case::dir_cases;

    #[dir_cases("resources/example_snap_data")]
    #[test]
    fn snap_data_parsing(_: &str, content: &str) {
        let res = serde_json::from_str::<Snap>(content);
        assert!(res.is_ok())
    }
} 
