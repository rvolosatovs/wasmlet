use core::fmt;

use tokio::io::{
    empty, stderr, stdin, stdout, AsyncRead, AsyncWrite, Empty, Stderr, Stdin, Stdout,
};
use wasmtime::component::{Linker, ResourceTable};

use crate::engine::ResourceView;

mod host;

#[repr(transparent)]
pub struct WasiCliImpl<T>(pub T);

impl<T: WasiCliView> WasiCliView for &mut T {
    fn cli(&mut self) -> &WasiCliCtx {
        (**self).cli()
    }
}

impl<T: WasiCliView> WasiCliView for WasiCliImpl<T> {
    fn cli(&mut self) -> &WasiCliCtx {
        self.0.cli()
    }
}

impl<T: ResourceView> ResourceView for WasiCliImpl<T> {
    fn table(&mut self) -> &mut ResourceTable {
        self.0.table()
    }
}

pub trait WasiCliView: ResourceView + Send {
    fn cli(&mut self) -> &WasiCliCtx;
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
pub fn add_to_linker<T>(linker: &mut Linker<T>) -> wasmtime::Result<()>
where
    T: WasiCliView,
{
    let exit_options = crate::engine::bindings::wasi::cli::exit::LinkOptions::default();
    add_to_linker_with_options(linker, &exit_options)
}

/// Similar to [`add_to_linker`], but with the ability to enable unstable features.
pub fn add_to_linker_with_options<T>(
    linker: &mut Linker<T>,
    exit_options: &crate::engine::bindings::wasi::cli::exit::LinkOptions,
) -> anyhow::Result<()>
where
    T: WasiCliView,
{
    let closure = annotate_cli(|cx| WasiCliImpl(cx));
    crate::engine::bindings::wasi::cli::environment::add_to_linker_get_host(linker, closure)?;
    crate::engine::bindings::wasi::cli::exit::add_to_linker_get_host(
        linker,
        exit_options,
        closure,
    )?;
    crate::engine::bindings::wasi::cli::stdin::add_to_linker_get_host(linker, closure)?;
    crate::engine::bindings::wasi::cli::stdout::add_to_linker_get_host(linker, closure)?;
    crate::engine::bindings::wasi::cli::stderr::add_to_linker_get_host(linker, closure)?;
    crate::engine::bindings::wasi::cli::terminal_input::add_to_linker_get_host(linker, closure)?;
    crate::engine::bindings::wasi::cli::terminal_output::add_to_linker_get_host(linker, closure)?;
    crate::engine::bindings::wasi::cli::terminal_stdin::add_to_linker_get_host(linker, closure)?;
    crate::engine::bindings::wasi::cli::terminal_stdout::add_to_linker_get_host(linker, closure)?;
    crate::engine::bindings::wasi::cli::terminal_stderr::add_to_linker_get_host(linker, closure)?;
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

pub trait InputStream: IsTerminal {
    fn reader(&self) -> Box<dyn AsyncRead + Send + Sync + Unpin>;
}

pub trait OutputStream: IsTerminal {
    fn writer(&self) -> Box<dyn AsyncWrite + Send + Sync + Unpin>;
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

impl IsTerminal for Stdin {
    fn is_terminal(&self) -> bool {
        std::io::stdin().is_terminal()
    }
}

impl InputStream for Stdin {
    fn reader(&self) -> Box<dyn AsyncRead + Send + Sync + Unpin> {
        Box::new(stdin())
    }
}

impl IsTerminal for std::io::Stdin {
    fn is_terminal(&self) -> bool {
        std::io::IsTerminal::is_terminal(self)
    }
}

impl InputStream for std::io::Stdin {
    fn reader(&self) -> Box<dyn AsyncRead + Send + Sync + Unpin> {
        Box::new(stdin())
    }
}

impl IsTerminal for Stdout {
    fn is_terminal(&self) -> bool {
        std::io::stdout().is_terminal()
    }
}

impl OutputStream for Stdout {
    fn writer(&self) -> Box<dyn AsyncWrite + Send + Sync + Unpin> {
        Box::new(stdout())
    }
}

impl IsTerminal for std::io::Stdout {
    fn is_terminal(&self) -> bool {
        std::io::IsTerminal::is_terminal(self)
    }
}

impl OutputStream for std::io::Stdout {
    fn writer(&self) -> Box<dyn AsyncWrite + Send + Sync + Unpin> {
        Box::new(stdout())
    }
}

impl IsTerminal for Stderr {
    fn is_terminal(&self) -> bool {
        std::io::stderr().is_terminal()
    }
}

impl OutputStream for Stderr {
    fn writer(&self) -> Box<dyn AsyncWrite + Send + Sync + Unpin> {
        Box::new(stderr())
    }
}

impl IsTerminal for std::io::Stderr {
    fn is_terminal(&self) -> bool {
        std::io::IsTerminal::is_terminal(self)
    }
}

impl OutputStream for std::io::Stderr {
    fn writer(&self) -> Box<dyn AsyncWrite + Send + Sync + Unpin> {
        Box::new(stderr())
    }
}
