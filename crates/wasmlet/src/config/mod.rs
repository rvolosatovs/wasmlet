use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod component;
pub mod env;
pub mod plugin;
pub mod service;
pub mod workload;

pub use component::Config as Component;
pub use env::Config as Env;
pub use plugin::Config as Plugin;
pub use service::Config as Service;
pub use workload::Config as Workload;

/// Deployment manifest
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Manifest<T = Box<str>> {
    #[serde(default)]
    pub plugins: BTreeMap<Box<str>, plugin::Config>,

    #[serde(default)]
    pub services: BTreeMap<Box<str>, service::Config<T>>,

    #[serde(default)]
    pub workloads: BTreeMap<Box<str>, workload::Config<T>>,
}

impl<T> Default for Manifest<T> {
    fn default() -> Self {
        Self {
            plugins: BTreeMap::default(),
            services: BTreeMap::default(),
            workloads: BTreeMap::default(),
        }
    }
}
