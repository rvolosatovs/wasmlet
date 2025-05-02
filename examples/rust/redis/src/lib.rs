pub mod bindings;
pub mod database;
pub mod pool;
pub mod resp3;

pub struct Handler;

//pub struct ConnectionState {
//    rx_buffer: BytesMut,
//    rx: InputStream,
//
//    tx_buffer: BytesMut,
//    tx: OutputStream,
//
//    requests: Vec<Rc<RefCell<Option<BytesFrame>>>>,
//    replies: Vec<Rc<RefCell<Option<BytesFrame>>>>,
//}
//
//pub struct ActiveConnection {
//    sock: TcpSocket,
//    state: Rc<RefCell<ConnectionState>>,
//}
//
//pub struct Request {
//    command: Rc<RefCell<Option<BytesFrame>>>,
//    reply: Rc<RefCell<Option<BytesFrame>>>,
//    state: Rc<RefCell<ConnectionState>>,
//}
//
//impl Request {
//    fn subscribe(&self) -> Pollable {
//        if self.command.borrow().is_none() || self.reply.borrow().is_some() {
//            subscribe_duration(0)
//        } else {
//            self.state.borrow().tx.subscribe()
//        }
//    }
//}
//
//pub struct Response {
//    reply: Rc<RefCell<Option<BytesFrame>>>,
//    state: Rc<RefCell<ConnectionState>>,
//}
//
//impl Response {
//    fn subscribe(&self) -> Pollable {
//        if self.reply.borrow().is_some() {
//            subscribe_duration(0)
//        } else {
//            self.state.borrow().rx.subscribe()
//        }
//    }
//}
//
//static CONN: Mutex<Option<Conn>> = Mutex::new(None);
//
//struct Conn {
//    sock: TcpSocket,
//    codec: Resp3,
//    rx: InputStream,
//    tx: OutputStream,
//}
//
//impl Conn {
//    fn connect() -> anyhow::Result<Self> {
//        let net = instance_network();
//        let sock = create_tcp_socket(IpAddressFamily::Ipv4).context("failed to create socket")?;
//        sock.start_connect(
//            &net,
//            IpSocketAddress::Ipv4(Ipv4SocketAddress {
//                address: (127, 0, 0, 1),
//                port: 6379,
//            }),
//        )
//        .context("failed to start connect")?;
//        sock.subscribe().block();
//        let (rx, tx) = sock.finish_connect().context("failed to connect")?;
//
//        let mut codec = Resp3::default();
//        let hello = BorrowedFrame::Hello {
//            version: RespVersion::RESP3,
//            auth: None,
//            setname: None,
//        };
//        let mut buf = BytesMut::with_capacity(hello.encode_len(false));
//        codec
//            .encode(hello, &mut buf)
//            .context("failed to encode `HELLO`")?;
//        tx.blocking_write_and_flush(&buf)
//            .context("failed to send `HELLO`")?;
//
//        let buf = rx
//            .blocking_read(1024)
//            .context("failed to read from stream")?;
//        let buf = Bytes::from(buf);
//        let frame = if let Some((frame, n)) = resp3::decode::complete::decode_bytes(&buf)
//            .context("failed to decode `HELLO` response")?
//        {
//            if buf.len() != n {
//                bail!("short `HELLO` response read");
//            };
//            frame
//        } else {
//            let mut buf = buf.into();
//            loop {
//                if let Some(frame) = codec
//                    .decode(&mut buf)
//                    .context("failed to read `HELLO` response")?
//                {
//                    if !buf.is_empty() {
//                        bail!("short `HELLO` response read");
//                    }
//                    break frame;
//                }
//                let chunk = rx
//                    .blocking_read(1024)
//                    .context("failed to read from stream")?;
//                buf.extend_from_slice(&chunk);
//            }
//        };
//        match frame {
//            BytesFrame::SimpleError { data, .. } => bail!("`HELLO` failed: {data}"),
//            BytesFrame::Map { data, .. } => {
//                for (k, v) in data {
//                    let k = k.as_str().context("map key is not a string")?;
//                    eprintln!("`HELLO` returned `{k}`: {v:?}")
//                }
//            }
//            _ => bail!("unexpected `HELLO` response frame received: {frame:?}"),
//        }
//        Ok(Self {
//            sock,
//            codec,
//            rx,
//            tx,
//        })
//    }
//}
