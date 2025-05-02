use anyhow::bail;
use redis_protocol::resp2;
use redis_protocol::resp3::types::{BytesFrame, VerbatimStringFormat};

use crate::bindings::exports::wex_examples::redis::database;
use crate::bindings::exports::wex_examples::redis::resp3::{
    FrameValue, Guest, GuestIndirectValue, IndirectValue, Value, VerbatimFormat, VerbatimString,
};
use crate::database::Frame;
use crate::Handler;

impl TryFrom<Frame> for FrameValue {
    type Error = anyhow::Error;

    fn try_from(frame: Frame) -> Result<Self, Self::Error> {
        match frame {
            Frame::Resp2(frame) => Ok(Self::Value(frame.into())),
            Frame::Resp3(frame) => frame.try_into(),
        }
    }
}

impl From<resp2::types::BytesFrame> for Value {
    fn from(frame: resp2::types::BytesFrame) -> Self {
        use resp2::types::BytesFrame;

        match frame {
            BytesFrame::SimpleString(data) => Self::SimpleString(data.into()),
            BytesFrame::Error(data) => Self::SimpleError(data.to_string()),
            BytesFrame::Integer(data) => Self::Number(data),
            BytesFrame::BulkString(data) => Self::BlobString(data.into()),
            BytesFrame::Array(data) => {
                let data = data.into_iter().map(IndirectValue::from).collect();
                Self::Array(data)
            }
            BytesFrame::Null => Self::Null,
        }
    }
}

impl TryFrom<BytesFrame> for FrameValue {
    type Error = anyhow::Error;

    fn try_from(frame: BytesFrame) -> Result<Self, Self::Error> {
        match frame {
            BytesFrame::Push { data, .. } => {
                let data = data
                    .into_iter()
                    .map(IndirectValue::try_from)
                    .collect::<anyhow::Result<_>>()?;
                Ok(Self::Push(data))
            }
            frame => {
                let frame = frame.try_into()?;
                Ok(Self::Value(frame))
            }
        }
    }
}

impl From<VerbatimStringFormat> for VerbatimFormat {
    fn from(format: VerbatimStringFormat) -> Self {
        match format {
            VerbatimStringFormat::Text => Self::Text,
            VerbatimStringFormat::Markdown => Self::Markdown,
        }
    }
}

impl TryFrom<BytesFrame> for Value {
    type Error = anyhow::Error;

    fn try_from(frame: BytesFrame) -> Result<Self, Self::Error> {
        match frame {
            BytesFrame::BlobString { data, .. } => Ok(Self::BlobString(data.into())),
            BytesFrame::BlobError { data, .. } => Ok(Self::BlobError(data.into())),
            BytesFrame::SimpleString { data, .. } => Ok(Self::SimpleString(data.into())),
            BytesFrame::SimpleError { data, .. } => Ok(Self::SimpleError(data.to_string())),
            BytesFrame::Boolean { data, .. } => Ok(Self::Boolean(data)),
            BytesFrame::Null => Ok(Self::Null),
            BytesFrame::Number { data, .. } => Ok(Self::Number(data)),
            BytesFrame::Double { data, .. } => Ok(Self::Double(data)),
            BytesFrame::BigNumber { data, .. } => Ok(Self::BigNumber(data.into())),
            BytesFrame::VerbatimString { data, format, .. } => {
                Ok(Self::VerbatimString(VerbatimString {
                    format: format.into(),
                    data: data.into(),
                }))
            }
            BytesFrame::Array { data, .. } => {
                let data = data
                    .into_iter()
                    .map(IndirectValue::try_from)
                    .collect::<anyhow::Result<_>>()?;
                Ok(Self::Array(data))
            }
            BytesFrame::Map { data, .. } => {
                let data = data
                    .into_iter()
                    .map(|(k, v)| {
                        let k = k.try_into()?;
                        let v = v.try_into()?;
                        Ok((k, v))
                    })
                    .collect::<anyhow::Result<_>>()?;
                Ok(Self::Map(data))
            }
            BytesFrame::Set { data, .. } => {
                let data = data
                    .into_iter()
                    .map(IndirectValue::try_from)
                    .collect::<anyhow::Result<_>>()?;
                Ok(Self::Set(data))
            }
            BytesFrame::Hello {
                version,
                auth,
                setname,
            } => todo!(),
            BytesFrame::ChunkedString(bytes) => todo!(),
            BytesFrame::Push { .. } => bail!("inner value cannot be a push"),
        }
    }
}

impl From<resp2::types::BytesFrame> for IndirectValue {
    fn from(frame: resp2::types::BytesFrame) -> Self {
        IndirectValue::new(Value::from(frame))
    }
}

impl TryFrom<BytesFrame> for IndirectValue {
    type Error = anyhow::Error;

    fn try_from(frame: BytesFrame) -> Result<Self, Self::Error> {
        let frame = Value::try_from(frame)?;
        Ok(IndirectValue::new(frame))
    }
}

impl Guest for Handler {
    type IndirectValue = Value;

    fn into_value(frame: database::Frame) -> Result<FrameValue, String> {
        frame
            .into_inner::<Frame>()
            .try_into()
            .map_err(|err| format!("{err:#}"))
    }
}

impl GuestIndirectValue for Value {
    fn unwrap(this: IndirectValue) -> Value {
        this.into_inner()
    }
}
