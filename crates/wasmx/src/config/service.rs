use serde::{Deserialize, Serialize};

use crate::config::{Component, Env};

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
pub struct Config<T> {
    #[serde(flatten)]
    pub component: Component<T>,

    #[serde(flatten)]
    pub env: Env,
}
