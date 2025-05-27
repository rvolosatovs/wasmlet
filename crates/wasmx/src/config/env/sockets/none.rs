use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "kube", derive(schemars::JsonSchema))]
#[serde(tag = "type")]
pub enum Loopback {
    #[default]
    #[serde(rename = "none")]
    None,
}
