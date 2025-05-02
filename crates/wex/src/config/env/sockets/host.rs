use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "type")]
pub enum Loopback {
    #[default]
    #[serde(rename = "none")]
    None,

    #[serde(rename = "host")]
    Host,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct Config<T> {
    pub address: Option<T>,

    #[serde(default)]
    pub loopback: Loopback,
}

impl<T> Default for Config<T> {
    fn default() -> Self {
        Self {
            address: None,
            loopback: Loopback::default(),
        }
    }
}
