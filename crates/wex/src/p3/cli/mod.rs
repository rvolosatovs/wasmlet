mod host;

use core::fmt;

use tokio::io::{empty, AsyncRead, AsyncWrite, Empty};
use wasmtime::component::{Linker, ResourceTable};

use crate::p3::ResourceView;

#[repr(transparent)]
pub struct WasiCliImpl<T>(pub T);

impl<T: WasiCliView> WasiCliView for &mut T {
    fn cli(&self) -> &WasiCliCtx {
        (**self).cli()
    }
}

impl<T: WasiCliView> WasiCliView for WasiCliImpl<T> {
    fn cli(&self) -> &WasiCliCtx {
        self.0.cli()
    }
}

impl<T: ResourceView> ResourceView for WasiCliImpl<T> {
    fn table(&mut self) -> &mut ResourceTable {
        self.0.table()
    }
}

pub trait WasiCliView: ResourceView + Send {
    fn cli(&self) -> &WasiCliCtx;
}

pub struct WasiCliCtx {
    pub environment: Vec<(String, String)>,
    pub arguments: Vec<String>,
    pub initial_cwd: Option<String>,
    pub stdin: Box<dyn InputStream + Send>,
    pub stdout: Box<dyn OutputStream + Send>,
    pub stderr: Box<dyn OutputStream + Send>,
}

impl Default for WasiCliCtx {
    fn default() -> Self {
        Self {
            environment: Vec::default(),
            arguments: Vec::default(),
            initial_cwd: None,
            stdin: Box::new(empty()),
            stdout: Box::new(empty()),
            stderr: Box::new(empty()),
        }
    }
}

/// Add all WASI interfaces from this module into the `linker` provided.
///
/// This function will add the `async` variant of all interfaces into the
/// [`Linker`] provided. By `async` this means that this function is only
/// compatible with [`Config::async_support(true)`][async]. For embeddings with
/// async support disabled see [`add_to_linker_sync`] instead.
///
/// This function will add all interfaces implemented by this crate to the
/// [`Linker`], which corresponds to the `wasi:cli/imports` world supported by
/// this crate.
///
/// [async]: wasmtime::Config::async_support
///
/// # Example
///
/// ```
/// use wasmtime::{Engine, Result, Store, Config};
/// use wasmtime::component::{ResourceTable, Linker};
/// use wasmtime_wasi::p3::cli::{WasiCliView, WasiCliCtx};
/// use wasmtime_wasi::p3::ResourceView;
///
/// fn main() -> Result<()> {
///     let mut config = Config::new();
///     config.async_support(true);
///     let engine = Engine::new(&config)?;
///
///     let mut linker = Linker::<MyState>::new(&engine);
///     wasmtime_wasi::p3::cli::add_to_linker(&mut linker)?;
///     // ... add any further functionality to `linker` if desired ...
///
///     let mut store = Store::new(
///         &engine,
///         MyState {
///             cli: WasiCliCtx::default(),
///             table: ResourceTable::default(),
///         },
///     );
///
///     // ... use `linker` to instantiate within `store` ...
///
///     Ok(())
/// }
///
/// struct MyState {
///     cli: WasiCliCtx,
///     table: ResourceTable,
/// }
///
/// impl ResourceView for MyState {
///     fn table(&mut self) -> &mut ResourceTable { &mut self.table }
/// }
///
/// impl WasiCliView for MyState {
///     fn cli(&self) -> &WasiCliCtx { &self.cli }
/// }
/// ```
pub fn add_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()>
where
    T: WasiCliView + 'static,
{
    let exit_options = crate::p3::bindings::cli::exit::LinkOptions::default();
    add_to_linker_with_options(linker, &exit_options)
}

/// Similar to [`add_to_linker`], but with the ability to enable unstable features.
pub fn add_to_linker_with_options<T>(
    linker: &mut Linker<T>,
    exit_options: &crate::p3::bindings::cli::exit::LinkOptions,
) -> anyhow::Result<()>
where
    T: WasiCliView + 'static,
{
    let closure = annotate_cli(|cx| WasiCliImpl(cx));
    crate::p3::bindings::cli::environment::add_to_linker_get_host(linker, closure)?;
    crate::p3::bindings::cli::exit::add_to_linker_get_host(linker, exit_options, closure)?;
    crate::p3::bindings::cli::stdin::add_to_linker_get_host(linker, closure)?;
    crate::p3::bindings::cli::stdout::add_to_linker_get_host(linker, closure)?;
    crate::p3::bindings::cli::stderr::add_to_linker_get_host(linker, closure)?;
    crate::p3::bindings::cli::terminal_input::add_to_linker_get_host(linker, closure)?;
    crate::p3::bindings::cli::terminal_output::add_to_linker_get_host(linker, closure)?;
    crate::p3::bindings::cli::terminal_stdin::add_to_linker_get_host(linker, closure)?;
    crate::p3::bindings::cli::terminal_stdout::add_to_linker_get_host(linker, closure)?;
    crate::p3::bindings::cli::terminal_stderr::add_to_linker_get_host(linker, closure)?;
    Ok(())
}

fn annotate_cli<T, F>(val: F) -> F
where
    F: Fn(&mut T) -> WasiCliImpl<&mut T>,
{
    val
}

/// An error returned from the `proc_exit` host syscall.
///
/// Embedders can test if an error returned from wasm is this error, in which
/// case it may signal a non-fatal trap.
#[derive(Debug)]
pub struct I32Exit(pub i32);

impl fmt::Display for I32Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Exited with i32 exit status {}", self.0)
    }
}

impl std::error::Error for I32Exit {}

pub struct TerminalInput;
pub struct TerminalOutput;

pub trait IsTerminal {
    /// Returns whether this stream is backed by a TTY.
    fn is_terminal(&self) -> bool;
}

impl IsTerminal for Empty {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl InputStream for Empty {
    fn reader(&self) -> Box<dyn AsyncRead + Send + Sync + Unpin> {
        Box::new(empty())
    }
}

impl OutputStream for Empty {
    fn writer(&self) -> Box<dyn AsyncWrite + Send + Sync + Unpin> {
        Box::new(empty())
    }
}

impl IsTerminal for std::io::Empty {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl InputStream for std::io::Empty {
    fn reader(&self) -> Box<dyn AsyncRead + Send + Sync + Unpin> {
        Box::new(empty())
    }
}

impl OutputStream for std::io::Empty {
    fn writer(&self) -> Box<dyn AsyncWrite + Send + Sync + Unpin> {
        Box::new(empty())
    }
}

pub trait InputStream: IsTerminal {
    fn reader(&self) -> Box<dyn AsyncRead + Send + Sync + Unpin>;
}

pub trait OutputStream: IsTerminal {
    fn writer(&self) -> Box<dyn AsyncWrite + Send + Sync + Unpin>;
}
