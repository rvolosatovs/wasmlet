use serde::{Deserialize, Serialize};

use crate::config::{Component, Env};

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default, rename = "execution-time-ms")]
    pub execution_time_ms: Option<u64>,

    #[serde(default)]
    pub instances: Option<usize>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config<T> {
    #[serde(flatten)]
    pub component: Component<T>,

    #[serde(flatten)]
    pub env: Env,

    #[serde(default)]
    pub pool: usize,

    #[serde(default)]
    pub limits: Limits,
}
