//! Parsing of snapd API responses
use crate::{socket_client::body_json, Error, Result};
use hyper::{body::Incoming, Response};
use serde::{
    de::{self, DeserializeOwned, Deserializer},
    Deserialize,
};
use serde_json::Value;

/// Parse a raw response body from snapd into our internal Result type
pub async fn parse_response<T>(res: Response<Incoming>) -> Result<T>
where
    T: DeserializeOwned,
{
    let status = res.status();
    let resp: SnapdResponse<T> = body_json(res).await?;

    resp.result.map_err(|raw| Error::SnapdError {
        status,
        message: raw.message,
        kind: raw.kind,
        value: raw.value,
    })
}

#[derive(Debug)]
struct SnapdResponse<T> {
    result: std::result::Result<T, RawError>,
}

#[derive(Debug, Deserialize)]
struct RawError {
    message: String,
    kind: Option<String>,
    value: Option<Value>,
}

impl<'de, T> Deserialize<'de> for SnapdResponse<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let RawResponse { ty, result } = RawResponse::deserialize(deserializer)?;
        let result = if ty != "error" {
            Ok(T::deserialize(result).map_err(de::Error::custom)?)
        } else {
            Err(RawError::deserialize(result).map_err(de::Error::custom)?)
        };

        return Ok(SnapdResponse { result });

        // serde structs

        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        struct RawResponse {
            #[serde(rename = "type")]
            ty: String,
            result: Value,
        }
    }
}
