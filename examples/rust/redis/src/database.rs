use core::cell::RefCell;
use core::iter;

use std::collections::VecDeque;

use bytes::{Bytes, BytesMut};
use redis_protocol::tokio_util::codec::Decoder;
use redis_protocol::{codec::Resp2, resp3::encode::complete::extend_encode};
use redis_protocol::{codec::Resp3, resp2, resp3};
use redis_protocol::{
    error::RedisProtocolError,
    resp3::types::{BytesFrame, Resp3Frame as _},
};
use wasi::io::poll::Pollable;
use wasi::io::streams::{InputStream, OutputStream};
use wasi::sockets::instance_network::instance_network;
use wasi::sockets::network::{self, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress};
use wasi::sockets::tcp::TcpSocket;
use wasi::sockets::tcp_create_socket::create_tcp_socket;

use crate::bindings::exports::wex_examples::redis::database::{
    self, ActiveConnection, FrameStreamError, Guest, GuestCommandSink, GuestFrame,
    GuestFrameStream, GuestPendingConnection,
};
use crate::bindings::wex_examples::redis::commands::{
    Command, Get, Hello, Incr, Ping, Publish, Set, Watch,
};
use crate::Handler;

impl Guest for Handler {
    type Frame = Frame;
    type FrameStream = FrameStream;
    type CommandSink = CommandSink;
    type PendingConnection = PendingConnection;

    fn connect() -> Result<database::PendingConnection, network::ErrorCode> {
        let net = instance_network();
        let sock = create_tcp_socket(IpAddressFamily::Ipv4)?;
        sock.start_connect(
            &net,
            IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (127, 0, 0, 1),
                port: 6379,
            }),
        )?;
        Ok(database::PendingConnection::new(PendingConnection(sock)))
    }
}

#[derive(Debug)]
pub enum Frame {
    Resp2(resp2::types::BytesFrame),
    Resp3(resp3::types::BytesFrame),
}

impl GuestFrame for Frame {}

enum Codec {
    Resp2(Resp2),
    Resp3(Resp3),
}

impl Decoder for Codec {
    type Item = Frame;
    type Error = RedisProtocolError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self {
            Codec::Resp2(resp2) => {
                let frame = resp2.decode(src)?;
                Ok(frame.map(Frame::Resp2))
            }
            Codec::Resp3(resp3) => {
                let frame = resp3.decode(src)?;
                Ok(frame.map(Frame::Resp3))
            }
        }
    }
}

impl Default for Codec {
    fn default() -> Self {
        Self::Resp2(Resp2::default())
    }
}

pub struct FrameStream {
    stream: InputStream,
    frames: RefCell<VecDeque<database::Frame>>,
    buffer: RefCell<BytesMut>,
    codec: RefCell<Codec>,
}

impl GuestFrameStream for FrameStream {
    fn new(rx: InputStream) -> Self {
        Self {
            stream: rx,
            frames: RefCell::default(),
            buffer: RefCell::default(),
            codec: RefCell::default(),
        }
    }

    fn use_resp2(&self) -> Result<(), ()> {
        let mut codec = self.codec.borrow_mut();
        if !matches!(&*codec, Codec::Resp2(..)) {
            *codec = Codec::Resp2(Resp2::default());
        }
        Ok(())
    }

    fn use_resp3(&self) -> Result<(), ()> {
        let mut codec = self.codec.borrow_mut();
        if !matches!(&*codec, Codec::Resp3(..)) {
            // TODO: Verify that we're not in a middle of a stream
            *codec = Codec::Resp3(Resp3::default());
        }
        Ok(())
    }

    fn next(&self) -> Option<database::Frame> {
        self.frames.borrow_mut().pop_front()
    }

    fn next_many(&self, limit: u32) -> Vec<database::Frame> {
        let mut frames = self.frames.borrow_mut();
        let mut n = frames.len();
        if let Ok(limit) = usize::try_from(limit) {
            n = limit.min(n);
        }
        frames.drain(..n).collect()
    }

    fn buffered_frames(&self) -> u32 {
        let frames = self.frames.borrow();
        let n = frames.len();
        n.try_into().unwrap_or(u32::MAX)
    }

    fn unparsed_bytes(&self) -> u32 {
        let buffer = self.buffer.borrow();
        let n = buffer.len();
        n.try_into().unwrap_or(u32::MAX)
    }

    fn subscribe(&self) -> Pollable {
        self.stream.subscribe()
    }

    fn read(&self) -> Result<bool, FrameStreamError> {
        let buf = self.stream.read(8096).map_err(FrameStreamError::Io)?;
        let mut buffer = self.buffer.borrow_mut();
        if buffer.is_empty() {
            *buffer = Bytes::from(buf).into();
        } else {
            buffer.extend_from_slice(&buf);
        }
        let mut frames = self.frames.borrow_mut();
        let mut codec = self.codec.borrow_mut();
        while let Some(frame) = codec
            .decode(&mut buffer)
            .map_err(|err| FrameStreamError::Decode(err.to_string()))?
        {
            frames.push_back(database::Frame::new(frame));
        }
        Ok(true)
    }

    fn finish(this: database::FrameStream) -> Result<Vec<database::Frame>, u32> {
        let Self { frames, buffer, .. } = this.into_inner();
        let buffer = buffer.take();
        if !buffer.is_empty() {
            return Err(buffer.len().try_into().unwrap_or(u32::MAX));
        }
        Ok(frames.take().into())
    }
}

pub struct CommandSink {
    stream: OutputStream,
    buffer: RefCell<BytesMut>,
}

fn blob_string(data: impl Into<Bytes>) -> BytesFrame {
    BytesFrame::BlobString {
        data: data.into(),
        attributes: None,
    }
}

fn cmd(name: impl Into<Bytes>, arguments: impl IntoIterator<Item = BytesFrame>) -> BytesFrame {
    let data = iter::once(blob_string(name)).chain(arguments).collect();
    BytesFrame::Array {
        data,
        attributes: None,
    }
}

impl From<Get> for BytesFrame {
    fn from(Get { key }: Get) -> Self {
        cmd("get", [blob_string(key)])
    }
}

impl From<Hello> for BytesFrame {
    fn from(
        Hello {
            protover,
            auth,
            setname,
        }: Hello,
    ) -> Self {
        // TODO: Support arguments
        cmd("hello", None)
    }
}

impl From<Incr> for BytesFrame {
    fn from(Incr { key }: Incr) -> Self {
        cmd("incr", [blob_string(key)])
    }
}

impl From<Ping> for BytesFrame {
    fn from(Ping { message }: Ping) -> Self {
        cmd("ping", message.map(blob_string))
    }
}

impl From<Publish> for BytesFrame {
    fn from(Publish { channel, message }: Publish) -> Self {
        cmd("publish", [blob_string(channel), blob_string(message)])
    }
}

impl From<Set> for BytesFrame {
    fn from(
        Set {
            key,
            value,
            ex,
            px,
            exat,
            pxat,
            options,
        }: Set,
    ) -> Self {
        // TODO: Support args
        cmd("set", [blob_string(key), blob_string(value)])
    }
}

impl From<Watch> for BytesFrame {
    fn from(Watch { keys }: Watch) -> Self {
        // TODO: Support args
        cmd("watch", keys.into_iter().map(blob_string))
    }
}

impl From<Command> for BytesFrame {
    fn from(command: Command) -> Self {
        match command {
            Command::Discard => cmd("discard", None),
            Command::Exec => cmd("exec", None),
            Command::Get(cmd) => cmd.into(),
            Command::Hello(cmd) => cmd.into(),
            Command::Incr(cmd) => cmd.into(),
            Command::Multi => cmd("multi", None),
            Command::Ping(cmd) => cmd.into(),
            Command::Publish(cmd) => cmd.into(),
            Command::Quit => cmd("quit", None),
            Command::Set(cmd) => cmd.into(),
            Command::Unwatch => cmd("unwatch", None),
            Command::Watch(cmd) => cmd.into(),
        }
    }
}

impl GuestCommandSink for CommandSink {
    fn new(tx: OutputStream) -> Self {
        Self {
            stream: tx,
            buffer: RefCell::default(),
        }
    }

    fn feed(&self, command: Command) -> Result<bool, String> {
        let mut buffer = self.buffer.borrow_mut();
        let command = BytesFrame::from(command);
        extend_encode(&mut buffer, &command, false).map_err(|err| err.to_string())?;
        Ok(true)
    }

    fn feed_many(&self, commands: Vec<Command>) -> Result<u32, String> {
        let mut buffer = self.buffer.borrow_mut();
        let commands: Vec<_> = commands.into_iter().map(BytesFrame::from).collect();
        let size = commands.iter().map(|frame| frame.encode_len(false)).sum();
        buffer.reserve(size);
        let n = commands.len().try_into().unwrap_or(u32::MAX);
        for command in &commands[..n as _] {
            extend_encode(&mut buffer, command, false).map_err(|err| err.to_string())?;
        }
        Ok(n)
    }

    fn buffered_bytes(&self) -> u32 {
        let buffer = self.buffer.borrow();
        buffer.len().try_into().unwrap_or(u32::MAX)
    }

    fn subscribe(&self) -> Pollable {
        self.stream.subscribe()
    }

    fn write(&self) -> Result<(), wasi::io::streams::StreamError> {
        let n = self.stream.check_write()?;
        let n = n.try_into().unwrap_or(usize::MAX);

        let mut buffer = self.buffer.borrow_mut();
        let n = n.min(buffer.len());

        if n == 0 {
            return Ok(());
        }
        self.stream.write(&buffer.split_to(n))
    }
}

pub struct PendingConnection(TcpSocket);

impl GuestPendingConnection for PendingConnection {
    fn subscribe(&self) -> Pollable {
        self.0.subscribe()
    }

    fn finish(this: database::PendingConnection) -> Result<ActiveConnection, network::ErrorCode> {
        let PendingConnection(socket) = this.into_inner();
        let mut res = socket.finish_connect();
        if let Err(network::ErrorCode::WouldBlock) = res {
            socket.subscribe().block();
            res = socket.finish_connect();
        }
        let (rx, tx) = res?;
        let frames = database::FrameStream::new(FrameStream {
            stream: rx,
            frames: RefCell::default(),
            buffer: RefCell::default(),
            codec: RefCell::default(),
        });
        let commands = database::CommandSink::new(CommandSink {
            stream: tx,
            buffer: RefCell::default(),
        });
        Ok(ActiveConnection {
            socket,
            frames,
            commands,
        })
    }
}
