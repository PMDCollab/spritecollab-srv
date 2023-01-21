use once_cell::sync::OnceCell;
use regex::Regex;
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;

static DISCORD_REGEX: OnceCell<Regex> = OnceCell::new();

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct CreditRow<'a> {
    #[serde(
        deserialize_with = "cleanup_discord_id",
        rename(deserialize = "Discord")
    )]
    pub credit_id: Cow<'a, str>,
    #[serde(rename(deserialize = "Name"))]
    pub name: Option<Cow<'a, str>>,
    #[serde(rename(deserialize = "Contact"))]
    pub contact: Option<Cow<'a, str>>,
}

pub fn cleanup_discord_id<'de, D>(deser: D) -> Result<Cow<'static, str>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(parse_credit_id(String::deserialize(deser)?))
}

pub fn parse_credit_id<S: AsRef<str> + ToString>(credit_id_raw: S) -> Cow<'static, str> {
    let cell = &DISCORD_REGEX;
    let regex = cell.get_or_init(|| Regex::new(r"<@!(\d+)>").unwrap());

    if let Some(discord_id) = regex.captures(credit_id_raw.as_ref()) {
        discord_id[1].to_string()
    } else {
        credit_id_raw.to_string()
    }
    .into()
}
