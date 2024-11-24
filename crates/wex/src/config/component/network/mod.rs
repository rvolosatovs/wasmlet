use core::net::{Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

pub mod host;
pub mod none;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum Ports {
    /// each port is bound 1:1
    #[serde(rename = "direct")]
    #[default]
    Direct,

    /// each port bind acts as bind on port `0`, remapped in the component
    #[serde(rename = "dynamic")]
    Dynamic,
    ///// non-zero ports are mapped using a static mapping
    //#[serde(rename = "map")]
    //Map { map: HashMap<NonZeroUsize, u64> },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(tag = "type")]
pub struct Transport {
    #[serde(default)]
    pub ipv4: Network<Ipv4Addr>,

    #[serde(default)]
    pub ipv6: Network<Ipv6Addr>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub tcp: Transport,

    #[serde(default)]
    pub udp: Transport,
}
