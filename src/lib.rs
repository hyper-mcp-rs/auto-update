//! Self-updating support for CLI binaries.
//!
//! This crate provides composable wrappers around any type implementing
//! [`self_update::update::ReleaseUpdate`], each exposed through a builder in
//! the style of `self_update`'s own backends:
//!
//! - [`throttle::Update`] limits how often update checks run by recording the
//!   time of the last check in a throttle file in the system temp directory.
//! - [`restart::Update`] re-executes the process with the freshly installed
//!   binary after a successful update, using a guard environment variable to
//!   prevent restart loops.
//! - [`silence::Update`] redirects the wrapped update's standard output (fd 1)
//!   to either standard error or `/dev/null` while it runs, then restores it,
//!   keeping a headless update from polluting a stdio stream a parent process
//!   is monitoring (for example, a stdio-based MCP server).
//!
//! Both wrappers implement `ReleaseUpdate` themselves and their builders
//! produce a `Box<dyn ReleaseUpdate>`, so they can be layered over a backend
//! (or over each other) and used anywhere a `ReleaseUpdate` is expected.
//!
//! # Example
//!
//! ```ignore
//! use self_update_extras::{restart, silence, throttle};
//! use self_update::backends::github;
//! use self_update::update::ReleaseUpdate;
//! use std::time::Duration;
//!
//! // Any `ReleaseUpdate` implementation, e.g. a self_update GitHub backend.
//! let backend = github::Update::configure().build()?;
//!
//! // `silence` sits closest to the backend so it diverts exactly the noisy
//! // download/install output, and stays *inside* `restart` so the re-executed
//! // process inherits the real stdout.
//! let quiet = silence::Update::configure()
//!     .release_update(backend)
//!     .sink(silence::Sink::Null)
//!     .build()?;
//!
//! let throttled = throttle::Update::configure()
//!     .release_update(quiet)
//!     .throttle_window(Duration::from_secs(15 * 60))
//!     .build()?;
//!
//! let updater = restart::Update::configure()
//!     .release_update(throttled)
//!     .guard_env("MY_APP_AUTO_UPDATED")
//!     .build()?;
//!
//! let status = updater.update()?;
//! ```

pub mod restart;
pub mod silence;
pub mod throttle;

#[cfg(test)]
mod test_support;
