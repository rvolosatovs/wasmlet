use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod sockets;

pub use sockets::Config as Sockets;

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
#[serde(tag = "type")]
pub enum Mount {
    #[serde(rename = "host")]
    Host { path: PathBuf },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
pub struct Filesystem {
    #[serde(default)]
    pub mounts: BTreeMap<Box<str>, Mount>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
pub struct Cpu {
    #[serde(default)]
    pub max: Box<str>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
pub struct Resources {
    #[serde(default)]
    pub cpu: Cpu,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
pub struct Namespace {
    #[serde(default)]
    pub ipc: PathBuf,
    #[serde(default)]
    pub network: PathBuf,
    #[serde(default)]
    pub uts: PathBuf,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
pub struct Linux {
    #[serde(default)]
    pub cgroup: Cgroup,
    #[serde(default)]
    pub namespace: Namespace,
    #[serde(default)]
    pub resources: Resources,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
pub struct Config {
    // TODO: figure this out
    //#[serde(default)]
    //pub linux: Linux,
    //#[serde(default)]
    //pub filesystem: Filesystem,
    //#[serde(default)]
    //pub sockets: sockets::Config,
}
