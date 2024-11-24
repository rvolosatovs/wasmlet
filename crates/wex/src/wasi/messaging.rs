#![allow(unused)]

use core::iter::{repeat, zip};

use anyhow::{bail, Context as _};
use async_nats::header::{IntoHeaderName as _, IntoHeaderValue as _};
use tracing::instrument;
use wasmtime::component::Resource;
use wasmtime_wasi::async_trait;

use crate::bindings::wasi::messaging::types::{Error, Metadata, Topic};
use crate::bindings::wasi::messaging::{producer, request_reply, types};
use crate::{nats_client, Ctx};

pub mod bindings {
    wasmtime::component::bindgen!({
        world: "messaging-guest",
        async: true,
        tracing: true,
        trappable_imports: true,
        with: {
            "wasi:messaging/types": crate::bindings::wasi::messaging::types,
        },
    });
}

/// Options for a request/reply operation.
#[derive(Debug, Default)]
pub struct RequestOptions {
    /// The maximum amount of time to wait for a response. If the timeout value is not set, then
    /// the request/reply operation will block until a message is received in response.
    pub timeout_ms: Option<u32>,

    /// The maximum number of replies to expect before returning.
    pub expected_replies: Option<u32>,
}

/// A message originating from the guest
#[derive(Debug, Default)]
pub struct GuestMessage {
    /// An optional content-type describing the format of the data in the message. This is
    /// sometimes described as the "format" type
    pub content_type: Option<String>,
    /// An opaque blob of data
    pub data: Vec<u8>,
    /// Optional metadata (also called headers or attributes in some systems) attached to the
    /// message. This metadata is simply decoration and should not be interpreted by a host
    /// to ensure portability across different implementors (e.g., Kafka -> NATS, etc.).
    pub metadata: Option<Vec<(String, String)>>,
}

pub enum Message {
    Nats(async_nats::Message),
    Guest(GuestMessage),
}

pub enum Client {
    Nats(async_nats::Client),
}

impl types::Host for Ctx {}

#[async_trait]
impl types::HostClient for Ctx {
    #[instrument(level = "debug", skip_all)]
    async fn connect(&mut self, name: String) -> wasmtime::Result<Result<Resource<Client>, Error>> {
        Ok(Err(Error::Other("not supported yet".into())))
    }

    #[instrument(level = "debug", skip_all)]
    async fn disconnect(&mut self, _msg: Resource<Client>) -> wasmtime::Result<Result<(), Error>> {
        Ok(Err(Error::Other("not supported yet".into())))
    }

    #[instrument(level = "debug", skip_all)]
    async fn drop(&mut self, rep: Resource<Client>) -> wasmtime::Result<()> {
        self.table.delete(rep).context("failed to delete client")?;
        Ok(())
    }
}

#[async_trait]
impl types::HostMessage for Ctx {
    #[instrument(level = "debug", skip_all)]
    async fn new(&mut self, data: Vec<u8>) -> wasmtime::Result<Resource<Message>> {
        self.table
            .push(Message::Guest(GuestMessage {
                data,
                ..Default::default()
            }))
            .context("failed to push message to table")
    }

    #[instrument(level = "debug", skip_all)]
    async fn topic(&mut self, msg: Resource<Message>) -> wasmtime::Result<Option<Topic>> {
        let msg = self.table.get(&msg).context("failed to get message")?;
        match msg {
            Message::Nats(async_nats::Message { subject, .. }) => Ok(Some(subject.to_string())),
            Message::Guest(..) => Ok(None),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn content_type(&mut self, msg: Resource<Message>) -> wasmtime::Result<Option<String>> {
        let msg = self.table.get(&msg).context("failed to get message")?;
        match msg {
            Message::Nats(..) => Ok(None),
            Message::Guest(GuestMessage { content_type, .. }) => Ok(content_type.clone()),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn set_content_type(
        &mut self,
        msg: Resource<Message>,
        content_type: String,
    ) -> wasmtime::Result<()> {
        let msg = self.table.get_mut(&msg).context("failed to get message")?;
        match msg {
            Message::Nats(..) => bail!("not supported for NATS.io"),
            Message::Guest(msg) => {
                msg.content_type = Some(content_type);
                Ok(())
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn data(&mut self, msg: Resource<Message>) -> wasmtime::Result<Vec<u8>> {
        let msg = self.table.get(&msg).context("failed to get message")?;
        match msg {
            Message::Nats(async_nats::Message { payload, .. }) => Ok(payload.to_vec()),
            Message::Guest(GuestMessage { data, .. }) => Ok(data.clone()),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn set_data(&mut self, msg: Resource<Message>, buf: Vec<u8>) -> wasmtime::Result<()> {
        let msg = self.table.get_mut(&msg).context("failed to get message")?;
        match msg {
            Message::Nats(async_nats::Message { payload, .. }) => {
                *payload = buf.into();
                Ok(())
            }
            Message::Guest(GuestMessage { data, .. }) => {
                *data = buf;
                Ok(())
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn metadata(&mut self, msg: Resource<Message>) -> wasmtime::Result<Option<Metadata>> {
        let msg = self.table.get(&msg).context("failed to get message")?;
        match msg {
            Message::Nats(async_nats::Message { headers: None, .. }) => Ok(None),
            Message::Nats(async_nats::Message {
                headers: Some(headers),
                ..
            }) => Ok(Some(headers.iter().fold(
                Vec::with_capacity(headers.len()),
                |mut headers, (k, vs)| {
                    for v in vs {
                        headers.push((k.to_string(), v.to_string()))
                    }
                    headers
                },
            ))),
            Message::Guest(GuestMessage { metadata, .. }) => Ok(metadata.clone()),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn add_metadata(
        &mut self,
        msg: Resource<Message>,
        key: String,
        value: String,
    ) -> wasmtime::Result<()> {
        let msg = self.table.get_mut(&msg).context("failed to get message")?;
        match msg {
            Message::Nats(async_nats::Message {
                headers: Some(headers),
                ..
            }) => {
                headers.append(key, value);
                Ok(())
            }
            Message::Nats(async_nats::Message { headers, .. }) => {
                *headers = Some(async_nats::HeaderMap::from_iter([(
                    key.into_header_name(),
                    value.into_header_value(),
                )]));
                Ok(())
            }
            Message::Guest(GuestMessage {
                metadata: Some(metadata),
                ..
            }) => {
                metadata.push((key, value));
                Ok(())
            }
            Message::Guest(GuestMessage { metadata, .. }) => {
                *metadata = Some(vec![(key, value)]);
                Ok(())
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn set_metadata(
        &mut self,
        msg: Resource<Message>,
        meta: Metadata,
    ) -> wasmtime::Result<()> {
        let msg = self.table.get_mut(&msg).context("failed to get message")?;
        match msg {
            Message::Nats(async_nats::Message { headers, .. }) => {
                *headers = Some(
                    meta.into_iter()
                        .map(|(k, v)| (k.into_header_name(), v.into_header_value()))
                        .collect(),
                );
                Ok(())
            }
            Message::Guest(GuestMessage { metadata, .. }) => {
                *metadata = Some(meta);
                Ok(())
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn remove_metadata(
        &mut self,
        msg: Resource<Message>,
        key: String,
    ) -> wasmtime::Result<()> {
        let msg = self.table.get_mut(&msg).context("failed to get message")?;
        match msg {
            Message::Nats(async_nats::Message {
                headers: Some(headers),
                ..
            }) => {
                *headers = headers
                    .iter()
                    .filter(|&(k, vs)| (k.as_ref() != key))
                    .flat_map(|(k, vs)| zip(repeat(k.clone()), vs.iter().cloned()))
                    .collect();
                Ok(())
            }
            Message::Guest(GuestMessage {
                metadata: Some(metadata),
                ..
            }) => {
                metadata.retain(|(k, _)| *k != key);
                Ok(())
            }
            Message::Nats(..) | Message::Guest(..) => Ok(()),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn drop(&mut self, rep: Resource<Message>) -> wasmtime::Result<()> {
        self.table.delete(rep).context("failed to delete message")?;
        Ok(())
    }
}

#[async_trait]
impl producer::Host for Ctx {
    #[instrument(level = "debug", skip_all)]
    async fn send(
        &mut self,
        c: Resource<Client>,
        topic: Topic,
        message: Resource<Message>,
    ) -> wasmtime::Result<Result<(), Error>> {
        todo!()
    }
}

#[async_trait]
impl request_reply::Host for Ctx {
    #[instrument(level = "debug", skip_all)]
    async fn request(
        &mut self,
        c: Resource<Client>,
        topic: Topic,
        message: Resource<Message>,
        options: Option<Resource<RequestOptions>>,
    ) -> wasmtime::Result<Result<Vec<Resource<Message>>, Error>> {
        todo!()
    }

    #[instrument(level = "debug", skip_all)]
    async fn reply(
        &mut self,
        reply_to: Resource<Message>,
        message: Resource<Message>,
    ) -> wasmtime::Result<Result<(), Error>> {
        let message = self
            .table
            .delete(message)
            .context("failed to delete outgoing message")?;
        let reply_to = self
            .table
            .get(&reply_to)
            .context("failed to get incoming message")?;
        match reply_to {
            Message::Nats(async_nats::Message {
                reply: Some(subject),
                ..
            }) => {
                let nats = match nats_client(&self.nats_once, self.nats_addr.as_ref()).await {
                    Ok(nats) => nats,
                    Err(err) => return Ok(Err(Error::Connection(err.to_string()))),
                };
                if let Err(err) = match message {
                    Message::Nats(async_nats::Message {
                        payload,
                        reply: None,
                        headers: None,
                        ..
                    }) => nats.publish(subject.clone(), payload).await,
                    Message::Nats(async_nats::Message {
                        payload,
                        reply: Some(reply),
                        headers: None,
                        ..
                    }) => {
                        nats.publish_with_reply(subject.clone(), reply, payload)
                            .await
                    }
                    Message::Nats(async_nats::Message {
                        payload,
                        reply: None,
                        headers: Some(headers),
                        ..
                    }) => {
                        nats.publish_with_headers(subject.clone(), headers, payload)
                            .await
                    }
                    Message::Nats(async_nats::Message {
                        payload,
                        reply: Some(reply),
                        headers: Some(headers),
                        ..
                    }) => {
                        nats.publish_with_reply_and_headers(
                            subject.clone(),
                            reply,
                            headers,
                            payload,
                        )
                        .await
                    }
                    Message::Guest(GuestMessage {
                        content_type,
                        data,
                        metadata,
                        ..
                    }) => {
                        if content_type.is_some() {
                            return Ok(Err(Error::Other(
                                "`content-type` not supported by NATS.io".into(),
                            )));
                        }
                        if let Some(metadata) = metadata {
                            nats.publish_with_headers(
                                subject.clone(),
                                metadata
                                    .into_iter()
                                    .map(|(k, v)| (k.into_header_name(), v.into_header_value()))
                                    .collect(),
                                data.into(),
                            )
                            .await
                        } else {
                            nats.publish(subject.clone(), data.into()).await
                        }
                    }
                } {
                    // TODO: check type
                    Ok(Err(Error::Other(err.to_string())))
                } else {
                    Ok(Ok(()))
                }
            }
            Message::Nats(async_nats::Message { reply: None, .. }) => Ok(Err(Error::Other(
                "NATS.io reply subject missing in original message".into(),
            ))),
            Message::Guest(..) => Ok(Err(Error::Other("cannot reply to guest message".into()))),
        }
    }
}

#[async_trait]
impl request_reply::HostRequestOptions for Ctx {
    #[instrument(level = "debug", skip_all)]
    async fn new(&mut self) -> wasmtime::Result<Resource<RequestOptions>> {
        self.table
            .push(RequestOptions::default())
            .context("failed to push request options to table")
    }

    #[instrument(level = "debug", skip_all)]
    async fn set_timeout_ms(
        &mut self,
        opts: Resource<RequestOptions>,
        timeout_ms: u32,
    ) -> wasmtime::Result<()> {
        let opts = self
            .table
            .get_mut(&opts)
            .context("failed to get request options")?;
        opts.timeout_ms = Some(timeout_ms);
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn set_expected_replies(
        &mut self,
        opts: Resource<RequestOptions>,
        expected_replies: u32,
    ) -> wasmtime::Result<()> {
        let opts = self
            .table
            .get_mut(&opts)
            .context("failed to get request options")?;
        opts.expected_replies = Some(expected_replies);
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn drop(&mut self, rep: Resource<RequestOptions>) -> wasmtime::Result<()> {
        self.table
            .delete(rep)
            .context("failed to delete request options")?;
        Ok(())
    }
}
