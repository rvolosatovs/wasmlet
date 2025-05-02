use serde::{Deserialize, Serialize};

use crate::config::{Component, Env};

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config<T> {
    #[serde(flatten)]
    pub component: Component<T>,

    #[serde(flatten)]
    pub env: Env,
}
