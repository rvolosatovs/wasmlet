use serde::{Deserialize, Serialize};

use crate::config::{Component, Env};

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Limits {
    #[serde(default, rename = "execution-time-ms")]
    pub execution_time_ms: Option<u64>,

    #[serde(default)]
    pub instances: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Config<T = Box<str>> {
    #[serde(flatten)]
    pub component: Component<T>,

    #[serde(flatten)]
    pub env: Env,

    #[serde(default)]
    pub pool: usize,

    #[serde(default)]
    pub limits: Limits,
}
