use serde::{Deserialize, Serialize};

use crate::config::{Component, Env};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Config<T = Box<str>> {
    #[serde(flatten)]
    pub component: Component<T>,

    #[serde(flatten)]
    pub env: Env,
}
