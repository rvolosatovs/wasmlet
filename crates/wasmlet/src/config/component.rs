use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum ImportKind {
    #[serde(rename = "workload")]
    Workload,
    #[serde(rename = "plugin")]
    Plugin,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Import {
    #[serde(rename = "type")]
    pub kind: ImportKind,
    pub target: Box<str>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct Config<T> {
    pub src: T,

    #[serde(default)]
    pub imports: BTreeMap<Box<str>, Import>,
}

impl<T> Config<T> {
    pub fn take_src(self) -> (T, Config<()>) {
        let Self { src, imports } = self;
        (src, Config { src: (), imports })
    }

    pub fn map_src<U>(self, f: impl FnOnce(T) -> U) -> Config<U> {
        let Self { src, imports } = self;
        let src = f(src);
        Config { src, imports }
    }
}
