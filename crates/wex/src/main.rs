use core::future::{poll_fn, Future as _};
use core::pin::pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::Poll;

use std::{sync::Arc, time::Instant};

use anyhow::{anyhow, bail, Context as _};
use clap::Parser;
use hyper_util::rt::TokioIo;
use tokio::sync::{broadcast, mpsc, watch, Notify, OnceCell};
use tokio::task::JoinSet;
use tokio::time::sleep;
use tokio::{fs, net::TcpListener};
use tokio::{select, signal};
use tracing::{debug, error, info, info_span, Instrument as _};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use wex::config::Config;
use wex::{
    handle_http_admin, handle_http_proxy, handle_join_result, new_engine, Args, Cmd, Event, Runtime,
};

fn main() -> anyhow::Result<()> {
    let Args {
        http_proxy,
        http_admin,
        shutdown_timeout,
        nats_address,
    } = Args::parse();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let (cmd_tx, cmd_rx) = mpsc::channel(1024);
    let (config_tx, config_rx) = watch::channel(Config::default());

    let ready = Arc::<AtomicBool>::new(AtomicBool::new(true));
    let shutdown: Arc<Notify> = Arc::default();
    let engine = new_engine(1000)?;
    let (events, _) = broadcast::channel(65536);
    let shutdown_timeout = *shutdown_timeout;
    let rt = Arc::new(Runtime {
        nats_addr: nats_address,
        nats_once: OnceCell::new(),
        engine,
        shutdown: Arc::clone(&shutdown),
        shutdown_timeout,
        config: config_rx.clone(),
        events: events.clone(),
    });

    #[cfg(target_os = "linux")]
    {
        //use nix::sched::{unshare, CloneFlags};
        use nix::sys::stat::{makedev, mknod, Mode, SFlag};

        match std::fs::exists("/dev/net/tun") {
            Ok(true) => {}
            Ok(false) => {
                if let Err(err) = std::fs::create_dir_all("/dev/net") {
                    error!(?err, "failed to create `/dev/net` directory")
                }
                if let Err(err) = mknod(
                    "/dev/net/tun",
                    SFlag::S_IFCHR,
                    Mode::S_IWOTH | Mode::S_IWGRP | Mode::S_IWUSR,
                    makedev(10, 200),
                ) {
                    error!(?err, "failed to create `/dev/net/tun`")
                }
            }
            Err(err) => {
                error!(?err, "failed to check if `/dev/net/tun` exists")
            }
        }
        //unshare(CloneFlags::CLONE_NEWUSER).context("failed to unshare user namespace")?;
    }

    let main = tokio::runtime::Runtime::new().context("failed to build Tokio runtime")?;
    main.block_on(async {
        let mut tasks = JoinSet::new();
        tasks.spawn({
            let rt = Arc::clone(&rt);
            async move {
                rt.handle_commands(cmd_rx, &config_tx).await?;
                debug!("command handler closed");
                Ok(())
            }
        });
        match fs::read_to_string("wex.toml").await {
            Ok(config) => {
                let config = toml::from_str(&config).context("failed to parse `wex.toml`")?;
                cmd_tx
                    .send(Cmd::ApplyConfig { config })
                    .await
                    .context("failed to send command")?
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                debug!("`wex.toml` not found")
            }
            Err(err) => {
                bail!(anyhow!(err).context("failed to read `wex.toml`"))
            }
        };
        if let Some(addr) = http_admin {
            let socket = TcpListener::bind(addr)
                .await
                .context("failed to bind HTTP administration endpoint")?;
            let addr = socket.local_addr().unwrap_or(addr);
            let ready = Arc::clone(&ready);
            let cmd_tx = cmd_tx.clone();
            let svc = hyper::service::service_fn(move |req| {
                handle_http_admin(
                    req,
                    Arc::clone(&ready),
                    config_rx.clone(),
                    cmd_tx.clone(),
                )
            });
            let srv = hyper::server::conn::http1::Builder::new();
            let shutdown = Arc::clone(&shutdown);
            tasks.spawn(
                async move {
                    let shutdown = shutdown.notified();
                    let mut shutdown = pin!(shutdown);
                    let mut tasks = JoinSet::new();
                    info!(?addr, "HTTP administration endpoint started");
                    loop {
                        let mut accept = pin!(socket.accept());
                        if !poll_fn(|cx|{
                            if let Poll::Ready(Some(res)) = tasks.poll_join_next(cx) {
                                handle_join_result(res);
                                Poll::Ready(true)
                            } else if let Poll::Ready(()) = shutdown.as_mut().poll(cx) {
                                Poll::Ready(false)
                            } else if let Poll::Ready(res) = accept.as_mut().poll(cx) {
                                let stream = match res {
                                    Ok((stream, addr)) => {
                                        info!(?addr, "accepted HTTP administration connection");
                                        stream
                                    }
                                    Err(err) => {
                                        error!(
                                            ?err,
                                            "failed to accept HTTP administration endpoint connection"
                                        );
                                        return Poll::Ready(true);
                                    }
                                };
                                let svc = svc.clone();
                                let conn = srv.serve_connection(TokioIo::new(stream), svc).with_upgrades();
                                tasks.spawn(async move {
                                    conn.await.context("failed to serve HTTP administration endpoint connection")
                                });
                                Poll::Ready(true)
                            } else {
                                Poll::Pending
                            }
                        }).await {
                            // wait for all invocations to complete
                            let start = Instant::now();
                            loop {
                                if !poll_fn(|cx| {
                                    if let Poll::Ready(res) = tasks.poll_join_next(cx) {
                                        if let Some(res) = res {
                                            handle_join_result(res);
                                            Poll::Ready(true)
                                        } else {
                                            Poll::Ready(false)
                                        }
                                    } else if Instant::now().duration_since(start) > shutdown_timeout {
                                            tasks.abort_all();
                                            Poll::Ready(false)
                                    } else {
                                        Poll::Pending
                                    }
                                }).await {
                            return Ok(())
                                }
                            }
                        }
                        //select! {
                        //    res = socket.accept() => {
                        //        let stream = match res {
                        //            Ok((stream, addr)) => {
                        //                info!(?addr, "accepted HTTP administration connection");
                        //                stream
                        //            }
                        //            Err(err) => {
                        //                error!(
                        //                    ?err,
                        //                    "failed to accept HTTP administration endpoint connection"
                        //                );
                        //                continue;
                        //            }
                        //        };
                        //        let svc = svc.clone();
                        //        let conn = srv.serve_connection(TokioIo::new(stream), svc).with_upgrades();
                        //        tasks.spawn(async move {
                        //            conn.await.context("failed to serve HTTP administration endpoint connection")
                        //        });
                        //    },
                        //    Some(res) = tasks.join_next() => {
                        //        handle_join_result(res)
                        //    },
                        //    _ = &mut shutdown => {
                        //        // wait for all invocations to complete
                        //        let deadline = sleep(shutdown_timeout);
                        //        let mut deadline = pin!(deadline);
                        //        loop {
                        //            select! {
                        //                res = tasks.join_next() => {
                        //                    if let Some(res) = res {
                        //                        handle_join_result(res);
                        //                    } else {
                        //                        return Ok(())
                        //                    }
                        //                }
                        //                _ = &mut deadline => {
                        //                    tasks.abort_all();
                        //                    while let Some(res) = tasks.join_next().await {
                        //                        handle_join_result(res);
                        //                    }
                        //                    return Ok(())
                        //                }
                        //            }
                        //        }
                        //    },
                        //}
                    }
                }
                .instrument(info_span!("handle_http_admin")),
            );
        }
        if let Some(addr) = http_proxy {
            let socket = TcpListener::bind(addr)
                .await
                .context("failed to bind HTTP proxy endpoint")?;
            let addr = socket.local_addr().unwrap_or(addr);
            let svc = hyper::service::service_fn(move |req| {
                handle_http_proxy(
                    req,
                )
            });
            let srv = hyper::server::conn::http1::Builder::new();
            let shutdown = Arc::clone(&shutdown);
            tasks.spawn(
                async move {
                    let mut tasks = JoinSet::new();
                    info!(?addr, "HTTP proxy endpoint started");
                    // TODO: check for shutdown, gracefully shutdown HTTP
                    // TODO: join conn tasks
                    loop {
                        let stream = match socket.accept().await {
                            Ok((stream, addr)) => {
                                info!(?addr, "accepted HTTP proxy connection");
                                stream
                            }
                            Err(err) => {
                                error!(
                                    ?err,
                                    "failed to accept HTTP proxy endpoint connection"
                                );
                                continue;
                            }
                        };
                        let svc = svc.clone();
                        let conn = srv.serve_connection(TokioIo::new(stream), svc).with_upgrades();
                        tasks.spawn(async move {
                            if let Err(err) = conn.await {
                                error!(
                                    ?err,
                                    "failed to serve HTTP proxy endpoint connection"
                                );
                            }
                        });
                    }
                }
                .instrument(info_span!("handle_http_proxy")),
            );
        }
        let ctrl_c = signal::ctrl_c();
        #[cfg(unix)]
        let mut sighup = signal::unix::signal(signal::unix::SignalKind::hangup())
            .context("failed to listen for SIGHUP")?;
        let mut ctrl_c = pin!(ctrl_c);
        loop {
            select! {
                res = tasks.join_next() => {
                    if let Some(res) = res {
                        handle_join_result(res);
                    } else {
                        info!("no tasks left, exiting");
                        return Ok(())
                    }
                },
                Some(()) = async {
                    if cfg!(unix) {
                        sighup.recv().await
                    } else {
                        core::future::pending().await
                    }
                } => {
                    info!("SIGHUP received, reloading config");
                    match fs::read_to_string("wex.toml").await {
                        Ok(config) => {
                            let config = toml::from_str(&config).context("failed to parse `wex.toml`")?;
                            cmd_tx
                                .send(Cmd::ApplyConfig { config })
                                .await
                                .context("failed to send command")?;
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                            debug!("`wex.toml` not found")
                        }
                        Err(err) => {
                            bail!(anyhow!(err).context("failed to read `wex.toml`"))
                        }
                    };
                }
                ctrl_c = &mut ctrl_c => {
                    info!(?shutdown_timeout, "^C received, shutting down");
                    _ = events.send(Event::Shutdown);
                    ready.store(false, Ordering::Relaxed);
                    shutdown.notify_waiters();
                    let ctrl_c = ctrl_c.context("failed to listen for ^C");
                    let deadline = sleep(shutdown_timeout);
                    let mut deadline = pin!(deadline);
                    loop {
                        select! {
                            res = tasks.join_next() => {
                                if let Some(res) = res {
                                    handle_join_result(res);
                                } else {
                                    return ctrl_c
                                }
                            }
                            _ = &mut deadline => {
                                tasks.abort_all();
                                while let Some(res) = tasks.join_next().await {
                                    handle_join_result(res);
                                }
                                return ctrl_c
                            }
                        }
                    }
                },
            }
        }
    })
}
