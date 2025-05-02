use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub enum Import {
    #[serde(rename = "workload")]
    Workload { target: Box<str> },
    #[serde(rename = "plugin")]
    Plugin { target: Box<str> },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
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
