use wasmtime::component::ResourceType;

use crate::engine::wasi;

pub type InputStream = wasi::io::InputStream;
pub type IoError = wasi::io::Error;
pub type OutputStream = wasi::io::OutputStream;
pub type Pollable = wasi::io::Pollable;

pub type TcpSocket = wasi::sockets::tcp::TcpSocket;

pub fn input_stream() -> ResourceType {
    ResourceType::host::<InputStream>()
}

pub fn io_error() -> ResourceType {
    ResourceType::host::<IoError>()
}

pub fn output_stream() -> ResourceType {
    ResourceType::host::<OutputStream>()
}

pub fn pollable() -> ResourceType {
    ResourceType::host::<Pollable>()
}

pub fn tcp_socket() -> ResourceType {
    ResourceType::host::<TcpSocket>()
}
