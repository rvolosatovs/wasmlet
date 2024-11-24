use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum Loopback {
    #[serde(rename = "none")]
    #[default]
    None,

    #[serde(rename = "tun")]
    Tun,

    #[serde(rename = "composition")]
    Composition { name: Option<Box<str>> },
}
