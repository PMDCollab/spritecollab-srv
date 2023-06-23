#![cfg(feature = "discord-reputation")]
//! Extension for getting reputation ("Guild Points") for authors for the official sprites.pmdcollab.org
//! repo via Swablu / SkyTemple Discord.

use std::collections::HashMap;

pub type ReputationMap = HashMap<String, i32>;

pub async fn fetch_reputation(fetch_url: &str) -> Result<ReputationMap, anyhow::Error> {
    let https = hyper_tls::HttpsConnector::new();
    let client = hyper::Client::builder().build::<_, hyper::Body>(https);
    let response = client.get(fetch_url.try_into()?).await?;
    let response_str =
        String::from_utf8(hyper::body::to_bytes(response.into_body()).await?.to_vec())?;
    Ok(serde_json::from_str(&response_str)?)
}
