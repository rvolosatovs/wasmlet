use core::net::SocketAddr;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod network;

pub use network::Config as Network;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HttpTrigger {
    pub address: SocketAddr,
}

#[derive(Clone, Default, Debug, Deserialize, Serialize)]
pub struct Cli {
    pub run: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NatsTrigger {
    pub subject: Box<str>,
    /// NATS queue group to use
    #[serde(default)]
    pub group: Box<str>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WrpcNatsTrigger {
    /// Prefix to listen for export invocations on
    #[serde(default)]
    pub prefix: Box<str>,
    /// NATS queue group to use
    #[serde(default)]
    pub group: Box<str>,
    pub instance: Box<str>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct WrpcTrigger {
    #[serde(default)]
    pub nats: Box<[WrpcNatsTrigger]>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Trigger {
    #[serde(default)]
    pub http: Box<[HttpTrigger]>,
    #[serde(default)]
    pub nats: Box<[NatsTrigger]>,
    #[serde(default)]
    pub wrpc: WrpcTrigger,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct KeyvalueBucket {
    pub target: Box<str>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum Mount {
    #[serde(rename = "host")]
    Host { path: PathBuf },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Filesystem {
    #[serde(default)]
    pub mounts: BTreeMap<Box<str>, Mount>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    pub src: Box<str>,
    #[serde(default)]
    pub trigger: Trigger,
    #[serde(default)]
    pub cli: Cli,
    #[serde(default)]
    pub filesystem: Filesystem,
    #[serde(default)]
    pub network: network::Config,
}
