//! Printing.
//!
//! [`types`] is platform-neutral vocabulary; the backend behind it is chosen at
//! compile time. Windows drives GDI directly (see [`windows`]). macOS and Linux
//! would go through CUPS, which is a genuinely different model — enough so that
//! sharing an implementation is not worth it. The stub below keeps the rest of
//! the app compiling on those targets and fails loudly if called.

pub mod types;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use self::windows::{capabilities, default_printer_name, list_printers, print_document};

#[cfg(not(windows))]
mod unsupported;

#[cfg(not(windows))]
pub use self::unsupported::{capabilities, default_printer_name, list_printers, print_document};
