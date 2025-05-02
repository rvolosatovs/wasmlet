use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "type")]
pub enum Loopback {
    #[default]
    #[serde(rename = "none")]
    None,
}
