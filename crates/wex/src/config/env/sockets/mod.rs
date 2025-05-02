use core::net::{Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

pub mod host;
pub mod none;

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "type")]
pub enum Network<T> {
    #[serde(rename = "none")]
    None {
        #[serde(default)]
        loopback: none::Loopback,
    },

    #[serde(rename = "host")]
    Host(host::Config<T>),
}

impl<T> Default for Network<T> {
    fn default() -> Self {
        Self::None {
            loopback: none::Loopback::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "type")]
pub struct Transport {
    #[serde(default)]
    pub ipv4: Network<Ipv4Addr>,

    #[serde(default)]
    pub ipv6: Network<Ipv6Addr>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub tcp: Transport,

    #[serde(default)]
    pub udp: Transport,
}
