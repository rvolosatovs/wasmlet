use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod component;
pub mod env;
pub mod service;
pub mod workload;

pub use component::Config as Component;
pub use env::Config as Env;
pub use service::Config as Service;
pub use workload::Config as Workload;

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
pub struct Plugin {
    #[serde(default)]
    pub src: Box<str>,
}

/// Deployment manifest
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(kube::CustomResource, schemars::JsonSchema))]
#[cfg_attr(
    feature = "kube",
    kube(
        group = "wasmx.dev",
        kind = "WasmxManifest",
        namespaced,
        status = "ManifestStatus",
        version = "v1",
    )
)]
pub struct Manifest<T = Box<str>> {
    // TODO: Version should be handled by serde codec
    //    #[serde(default)]
    //    pub version: Option<semver::Version>,
    //
    #[serde(default)]
    pub plugins: BTreeMap<Box<str>, Plugin>,

    #[serde(default)]
    pub services: BTreeMap<Box<str>, service::Config<T>>,

    #[serde(default)]
    pub workloads: BTreeMap<Box<str>, workload::Config<T>>,
}

impl<T> Default for Manifest<T> {
    fn default() -> Self {
        Self {
            //version: None,
            plugins: BTreeMap::default(),
            services: BTreeMap::default(),
            workloads: BTreeMap::default(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
pub struct ManifestStatus {
    pub error: Box<str>,
}
