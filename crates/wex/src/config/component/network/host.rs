use serde::{Deserialize, Serialize};

use super::Ports;

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

    #[serde(rename = "host")]
    Host,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config<T> {
    pub address: Option<T>,

    #[serde(default)]
    pub ports: Ports,

    #[serde(default)]
    pub loopback: Loopback,
}

impl<T> Default for Config<T> {
    fn default() -> Self {
        Self {
            address: None,
            ports: Ports::default(),
            loopback: Loopback::default(),
        }
    }
}
