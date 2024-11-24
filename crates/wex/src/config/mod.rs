use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod component;

pub use component::Config as Component;

pub const DEFAULT_NATS_ADDRESS: &str = "nats://localhost:4222";

fn default_nats_address() -> Box<str> {
    DEFAULT_NATS_ADDRESS.into()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Nats {
    /// NATS address to use
    #[serde(default = "default_nats_address")]
    pub address: Box<str>,
    // TODO: TLS etc.
}

impl Default for Nats {
    fn default() -> Self {
        Self {
            address: default_nats_address(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct KeyvalueBucket {
    pub target: Box<str>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Keyvalue {
    #[serde(default)]
    pub buckets: HashMap<Box<str>, KeyvalueBucket>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct MessagingClient {
    pub target: Box<str>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Messaging {
    #[serde(default)]
    pub clients: HashMap<Box<str>, MessagingClient>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Plugin {
    pub protocol: Box<str>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum Cgroup {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "host")]
    Host { path: PathBuf },
    #[serde(rename = "dynamic")]
    #[default]
    Dynamic,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Cpu {
    #[serde(default)]
    pub max: Box<str>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Resources {
    #[serde(default)]
    pub cpu: Cpu,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Namespaces {
    #[serde(default)]
    pub cpu: Cpu,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Linux {
    #[serde(default)]
    pub cgroup: Cgroup,
    #[serde(default)]
    pub namespaces: Namespaces,
    #[serde(default)]
    pub resources: Resources,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Composition {
    #[serde(default)]
    pub components: HashMap<Box<str>, component::Config>,
    #[serde(default)]
    pub linux: Linux,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub compositions: HashMap<Box<str>, Composition>,

    // TODO: Figure out how to manage these
    #[serde(default)]
    pub keyvalue: Keyvalue,
    #[serde(default)]
    pub messaging: Messaging,
    #[serde(default)]
    pub plugin: HashMap<Box<str>, Plugin>,
    #[serde(default)]
    pub nats: Nats,
}
