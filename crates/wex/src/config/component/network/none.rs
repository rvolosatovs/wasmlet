use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum Loopback {
    #[serde(rename = "none")]
    None,

    #[serde(rename = "local")]
    #[default]
    Local,

    #[serde(rename = "composition")]
    Composition { name: Option<Box<str>> },

    #[serde(rename = "tun")]
    Tun,
}
