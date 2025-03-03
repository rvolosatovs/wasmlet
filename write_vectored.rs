    async fn send(
        store: &mut Accessor<Self::TcpSocketData>,
        socket: Resource<TcpSocket>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<Result<(), ErrorCode>> {
        let (stream, fut) = match store.with(|mut store| {
            let fut = data.read(&mut store).context("failed to get data stream")?;
            let sock = get_socket(store.data_mut().table(), &socket)?;
            if let TcpState::Connected(stream) = &sock.tcp_state {
                Ok(Ok((Arc::clone(&stream), fut)))
            } else {
                Ok(Err(ErrorCode::InvalidState))
            }
        }) {
            Ok(Ok((stream, fut))) => (stream, fut),
            Ok(Err(err)) => return Ok(Err(err)),
            Err(err) => return Err(err),
        };
        let Some((tail, buf)) = fut.into_future().await else {
            match stream
                .as_socketlike_view::<std::net::TcpStream>()
                .shutdown(Shutdown::Write)
            {
                Ok(()) => return Ok(Ok(())),
                Err(err) => return Ok(Err(err.into())),
            }
        };
        let mut fut = next_item(store, tail)?;
        // TODO: Verify that stream buffering is actually desired here, if so - limit memory usage
        let mut bufs = VecDeque::from([buf]);
        let mut eof = false;
        loop {
            match bufs.as_mut_slices() {
                ([], []) => {
                    if eof {
                        match stream
                            .as_socketlike_view::<std::net::TcpStream>()
                            .shutdown(Shutdown::Write)
                        {
                            Ok(()) => return Ok(Ok(())),
                            Err(err) => return Ok(Err(err.into())),
                        }
                    }
                }
                ([buf], []) | ([], [buf]) => match stream.try_write(&buf) {
                    Ok(n) => {
                        if n == buf.len() {
                            bufs.clear();
                        } else {
                            *buf = buf.split_off(n);
                        }
                        continue;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(err) => return Ok(Err(err.into())),
                },
                _ => match stream.try_write_vectored(
                    &bufs
                        .iter()
                        .map(|buf| IoSlice::new(buf))
                        .collect::<Box<[_]>>(),
                ) {
                    Ok(mut n) => {
                        let mut i: usize = 0;
                        for buf in bufs.iter_mut() {
                            let len = buf.len();
                            if n < len {
                                *buf = buf.split_off(n);
                                break;
                            } else if n == len {
                                i = i.saturating_add(1);
                                break;
                            }
                            n = n.saturating_sub(len);
                            i = i.saturating_add(1);
                        }
                        if i == bufs.len() {
                            bufs.clear();
                        } else if i > 0 {
                            bufs.rotate_left(i);
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(err) => return Ok(Err(err.into())),
                },
            }
            match poll_fn(|cx| {
                if !eof {
                    match fut.as_mut().poll(cx) {
                        Poll::Ready(Some((tail, chunk))) => match next_item(store, tail) {
                            Ok(next) => {
                                fut = next;
                                bufs.push_front(chunk);
                                return Poll::Ready(Ok(Ok(())));
                            }
                            Err(err) => return Poll::Ready(Err(err)),
                        },
                        Poll::Ready(None) => {
                            eof = true;
                            return Poll::Ready(Ok(Ok(())));
                        }
                        Poll::Pending => {}
                    }
                }
                match pin!(stream.writable()).poll(cx) {
                    Poll::Ready(Ok(())) => Poll::Ready(Ok(Ok(()))),
                    Poll::Ready(Err(err)) => Poll::Ready(Ok(Err(err))),
                    Poll::Pending => Poll::Pending,
                }
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(err)) => return Ok(Err(err.into())),
                Err(err) => return Err(err),
            }
        }
    }

