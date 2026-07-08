//! Silence wrapper: redirects the wrapped update's stdout while it runs.
//!
//! This is intended for fully headless updates that run underneath a parent
//! process which is monitoring the child's standard I/O for other purposes,
//! such as a stdio-based MCP server where fd 1 carries the JSON-RPC protocol
//! stream. Any output the wrapped [`ReleaseUpdate`] writes to file descriptor
//! 1 would corrupt that stream, so it is diverted for the duration of the
//! update and fd 1 is restored before returning.

use self_update::Status;
use self_update::errors::{Error, Result};
use self_update::update::{Release, ReleaseUpdate, UpdateStatus};

/// Where the wrapped update's standard output (fd 1) is sent while it runs.
///
/// The redirect is applied at the file-descriptor level and therefore also
/// captures output from child processes and native libraries, not just the
/// current process's buffered stdout.
///
/// Named `Sink` rather than "target" to avoid colliding with
/// [`ReleaseUpdate::target`], which reports the platform target triple.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sink {
    /// Fold stdout into the process's standard error (fd 2).
    ///
    /// Output is preserved as out-of-band diagnostics. This is the
    /// default.
    #[default]
    Stderr,
    /// Discard stdout entirely, redirecting it to the `/dev/null` device on
    /// Unix or the `NUL` device on Windows.
    ///
    /// Use this for a truly hidden update whose output should vanish.
    Null,
}

/// Builder for a [`silence`](crate::silence) [`Update`].
///
/// Configure the inner [`ReleaseUpdate`] backend and, optionally, the redirect
/// [`Sink`], then call [`build`](Self::build) to produce a
/// `Box<dyn ReleaseUpdate>` that diverts fd 1 while the wrapped update runs.
#[derive(Default)]
pub struct UpdateBuilder {
    release_update: Option<Box<dyn ReleaseUpdate>>,
    sink: Option<Sink>,
}

impl UpdateBuilder {
    /// Initialize a new builder.
    pub fn new() -> Self {
        Default::default()
    }

    /// Set the release update implementation to wrap. Required.
    pub fn release_update(&mut self, release_update: Box<dyn ReleaseUpdate>) -> &mut Self {
        self.release_update = Some(release_update);
        self
    }

    /// Set where fd 1 is redirected while the update runs. Defaults to
    /// [`Sink::Stderr`].
    pub fn sink(&mut self, sink: Sink) -> &mut Self {
        self.sink = Some(sink);
        self
    }

    /// Confirm config and create a ready-to-use `Update`.
    ///
    /// * Errors:
    ///     * Config - `release_update` was not provided
    pub fn build(&mut self) -> Result<Box<dyn ReleaseUpdate>> {
        let inner = self
            .release_update
            .take()
            .ok_or_else(|| Error::Config("`release_update` required".to_owned()))?;

        Ok(Box::new(Update {
            inner,
            sink: self.sink.unwrap_or_default(),
        }))
    }
}

/// Wraps a [`ReleaseUpdate`] and redirects file descriptor 1 while it runs.
///
/// Before delegating to the inner update, fd 1 is pointed at the configured
/// [`Sink`]; once the update returns (successfully or not) the original fd 1
/// is restored. Because the redirect happens at the descriptor level, output
/// from spawned child processes and native code is diverted too.
///
/// # Platform behavior
///
/// - **Unix**: fd 1 is duplicated and swapped via `dup`/`dup2`.
/// - **Windows**: the process's standard-output handle is swapped via
///   `GetStdHandle`/`SetStdHandle` (pointed at the `NUL` device or the
///   standard-error handle). Rust's `std` re-queries `GetStdHandle` on every
///   write, so its output honors the swap for the duration of the update.
/// - **Other platforms**: redirection is unsupported, so the wrapped update
///   runs unchanged (no redirect is applied).
///
/// # Metadata
///
/// All metadata methods (including `no_confirm`, `show_output`, and
/// `show_download_progress`) delegate to the wrapped update. Because a backend
/// reads its own configuration when it runs, overriding those on the wrapper
/// would not change the behavior of an arbitrarily deep composition, so this
/// wrapper relies solely on the fd redirect to keep the update quiet.
pub struct Update {
    inner: Box<dyn ReleaseUpdate>,
    sink: Sink,
}

impl Update {
    /// Initialize a new `Update` builder.
    pub fn configure() -> UpdateBuilder {
        UpdateBuilder::new()
    }

    /// The [`Sink`] fd 1 is redirected to while the wrapped update runs.
    pub fn sink(&self) -> Sink {
        self.sink
    }
}

impl ReleaseUpdate for Update {
    fn get_latest_release(&self) -> Result<Release> {
        self.inner.get_latest_release()
    }

    fn get_latest_releases(&self, current_version: &str) -> Result<Vec<Release>> {
        self.inner.get_latest_releases(current_version)
    }

    fn get_release_version(&self, ver: &str) -> Result<Release> {
        self.inner.get_release_version(ver)
    }

    fn current_version(&self) -> String {
        self.inner.current_version()
    }

    fn target(&self) -> String {
        self.inner.target()
    }

    fn target_version(&self) -> Option<String> {
        self.inner.target_version()
    }

    fn bin_name(&self) -> String {
        self.inner.bin_name()
    }

    fn bin_install_path(&self) -> std::path::PathBuf {
        self.inner.bin_install_path()
    }

    fn bin_path_in_archive(&self) -> String {
        self.inner.bin_path_in_archive()
    }

    fn show_download_progress(&self) -> bool {
        self.inner.show_download_progress()
    }

    fn show_output(&self) -> bool {
        self.inner.show_output()
    }

    fn no_confirm(&self) -> bool {
        self.inner.no_confirm()
    }

    fn progress_template(&self) -> String {
        self.inner.progress_template()
    }

    fn progress_chars(&self) -> String {
        self.inner.progress_chars()
    }

    fn auth_token(&self) -> Option<String> {
        self.inner.auth_token()
    }

    fn update(&self) -> Result<Status> {
        let current_version = self.current_version();
        self.update_extended()
            .map(|s| s.into_status(current_version))
    }

    fn update_extended(&self) -> Result<UpdateStatus> {
        // The guard restores stdout when it drops at the end of this scope,
        // covering every early return from the inner update.
        #[cfg(any(unix, windows))]
        let _redirect = StdoutRedirect::new(self.sink)?;
        #[cfg(not(any(unix, windows)))]
        let _ = self.sink;

        self.inner.update_extended()
    }
}

/// RAII guard that redirects fd 1 on construction and restores it on drop.
#[cfg(unix)]
struct StdoutRedirect {
    saved: std::os::fd::OwnedFd,
}

#[cfg(unix)]
impl StdoutRedirect {
    fn new(sink: Sink) -> Result<Self> {
        use std::io::Write;
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

        // Flush buffered stdout so pending output reaches the real stdout
        // before fd 1 is diverted.
        let _ = std::io::stdout().flush();

        // Save the current stdout so it can be restored on drop.
        // SAFETY: `dup` returns a fresh descriptor we take exclusive ownership
        // of, or a negative value on error (handled below).
        let saved = unsafe {
            let fd = libc::dup(libc::STDOUT_FILENO);
            if fd < 0 {
                return Err(os_error("duplicating stdout"));
            }
            OwnedFd::from_raw_fd(fd)
        };

        let result = match sink {
            // SAFETY: fd 1 and fd 2 are valid; `dup2` points fd 1 at fd 2's
            // open file description.
            Sink::Stderr => unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) },
            Sink::Null => {
                // SAFETY: opening "/dev/null" yields an owned descriptor that
                // is closed as soon as fd 1 has been pointed at it.
                let null = unsafe {
                    let fd = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
                    if fd < 0 {
                        return Err(os_error("opening /dev/null"));
                    }
                    OwnedFd::from_raw_fd(fd)
                };
                // SAFETY: both descriptors are valid for the duration of the call.
                unsafe { libc::dup2(null.as_raw_fd(), libc::STDOUT_FILENO) }
            }
        };

        if result < 0 {
            return Err(os_error("redirecting stdout"));
        }

        Ok(Self { saved })
    }
}

#[cfg(unix)]
impl Drop for StdoutRedirect {
    fn drop(&mut self) {
        use std::io::Write;
        use std::os::fd::AsRawFd;

        // Flush anything the wrapped update buffered into the diverted stdout,
        // then restore fd 1 to its original destination.
        let _ = std::io::stdout().flush();
        // SAFETY: `saved` is a valid descriptor referring to the original stdout.
        unsafe {
            libc::dup2(self.saved.as_raw_fd(), libc::STDOUT_FILENO);
        }
    }
}

/// Minimal Win32 bindings for swapping the standard-output handle. Declared
/// locally (mirroring the crate's use of raw `libc` on Unix) to avoid pulling
/// in a Windows API crate for a handful of stable calls.
#[cfg(windows)]
#[allow(non_snake_case)]
mod win {
    use std::ffi::c_void;

    pub type Handle = *mut c_void;

    pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    pub const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    pub const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    pub const GENERIC_WRITE: u32 = 0x4000_0000;
    pub const FILE_SHARE_READ: u32 = 0x0000_0001;
    pub const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    pub const OPEN_EXISTING: u32 = 3;
    pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;

    unsafe extern "system" {
        pub fn GetStdHandle(nStdHandle: u32) -> Handle;
        pub fn SetStdHandle(nStdHandle: u32, hHandle: Handle) -> i32;
        pub fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: u32,
            dwShareMode: u32,
            lpSecurityAttributes: *mut c_void,
            dwCreationDisposition: u32,
            dwFlagsAndAttributes: u32,
            hTemplateFile: Handle,
        ) -> Handle;
        pub fn CloseHandle(hObject: Handle) -> i32;
    }
}

/// RAII guard that swaps the standard-output handle on construction and
/// restores it on drop.
#[cfg(windows)]
struct StdoutRedirect {
    saved: win::Handle,
    /// A handle we opened ourselves (the `NUL` device) that must be closed on
    /// drop, or null when the sink borrows an existing handle (stderr).
    opened: win::Handle,
}

#[cfg(windows)]
impl StdoutRedirect {
    fn new(sink: Sink) -> Result<Self> {
        use std::io::Write;

        // Flush buffered stdout so pending output reaches the real stdout
        // before the handle is swapped.
        let _ = std::io::stdout().flush();

        // SAFETY: FFI call returning the current stdout handle.
        let saved = unsafe { win::GetStdHandle(win::STD_OUTPUT_HANDLE) };
        if saved == win::INVALID_HANDLE_VALUE {
            return Err(os_error("querying stdout handle"));
        }

        let (target, opened) = match sink {
            Sink::Stderr => {
                // SAFETY: FFI call returning the current stderr handle.
                let err = unsafe { win::GetStdHandle(win::STD_ERROR_HANDLE) };
                if err == win::INVALID_HANDLE_VALUE {
                    return Err(os_error("querying stderr handle"));
                }
                (err, std::ptr::null_mut())
            }
            Sink::Null => {
                // "NUL" as a NUL-terminated wide string.
                let name = [b'N' as u16, b'U' as u16, b'L' as u16, 0];
                // SAFETY: opens the null device for writing; validated below.
                let handle = unsafe {
                    win::CreateFileW(
                        name.as_ptr(),
                        win::GENERIC_WRITE,
                        win::FILE_SHARE_READ | win::FILE_SHARE_WRITE,
                        std::ptr::null_mut(),
                        win::OPEN_EXISTING,
                        win::FILE_ATTRIBUTE_NORMAL,
                        std::ptr::null_mut(),
                    )
                };
                if handle == win::INVALID_HANDLE_VALUE {
                    return Err(os_error("opening NUL device"));
                }
                (handle, handle)
            }
        };

        // SAFETY: redirects the process's stdout handle to `target`.
        if unsafe { win::SetStdHandle(win::STD_OUTPUT_HANDLE, target) } == 0 {
            if !opened.is_null() {
                // SAFETY: release the handle we opened before erroring out.
                unsafe { win::CloseHandle(opened) };
            }
            return Err(os_error("redirecting stdout handle"));
        }

        Ok(Self { saved, opened })
    }
}

#[cfg(windows)]
impl Drop for StdoutRedirect {
    fn drop(&mut self) {
        use std::io::Write;

        // Flush anything the wrapped update buffered into the diverted stdout,
        // then restore the original handle.
        let _ = std::io::stdout().flush();
        // SAFETY: `saved` is the valid original stdout handle; `opened`, if
        // non-null, is the `NUL` handle we allocated in `new`.
        unsafe {
            win::SetStdHandle(win::STD_OUTPUT_HANDLE, self.saved);
            if !self.opened.is_null() {
                win::CloseHandle(self.opened);
            }
        }
    }
}

#[cfg(any(unix, windows))]
fn os_error(context: &str) -> Error {
    Error::Release(format!("{context}: {}", std::io::Error::last_os_error()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockRelease;

    /// Build a concrete `Update` for white-box tests. `build()` returns a
    /// `Box<dyn ReleaseUpdate>`, which hides the concrete type and its fields.
    fn concrete(mock: MockRelease, sink: Sink) -> Update {
        Update {
            inner: Box::new(mock),
            sink,
        }
    }

    #[test]
    fn build_requires_a_release_update() {
        assert!(Update::configure().build().is_err());
    }

    #[test]
    fn sink_defaults_to_stderr() {
        assert_eq!(Sink::default(), Sink::Stderr);
        let updater = concrete(MockRelease::new("mock-silence-default"), Sink::default());
        assert_eq!(updater.sink(), Sink::Stderr);
    }

    #[test]
    fn sink_getter_reports_the_configured_sink() {
        let updater = concrete(MockRelease::new("mock-silence-getter"), Sink::Null);
        assert_eq!(updater.sink(), Sink::Null);
    }

    #[test]
    fn builder_sets_the_sink() {
        // `build` erases the concrete type, so exercise the builder plumbing
        // by driving an update and confirming the inner backend still runs.
        let mock = MockRelease::new("mock-silence-builder");
        let calls = mock.call_counter();
        let updater = Update::configure()
            .release_update(Box::new(mock))
            .sink(Sink::Null)
            .build()
            .unwrap();

        let status = updater.update().unwrap();

        assert!(matches!(status, Status::UpToDate(_)));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn metadata_methods_delegate_to_inner() {
        let updater = concrete(MockRelease::new("mock-silence-forward"), Sink::Stderr);

        assert_eq!(updater.current_version(), "1.0.0");
        assert_eq!(updater.target(), "test-target");
        assert_eq!(updater.target_version(), None);
        assert_eq!(updater.bin_name(), "mock-silence-forward");
        assert_eq!(
            updater.bin_install_path(),
            std::env::temp_dir().join("mock-silence-forward")
        );
        assert_eq!(updater.bin_path_in_archive(), "mock-silence-forward");
        assert!(!updater.show_download_progress());
        assert!(!updater.show_output());
        assert!(updater.no_confirm());
        assert_eq!(updater.progress_template(), "");
        assert_eq!(updater.progress_chars(), "");
        assert_eq!(updater.auth_token(), None);
        assert!(updater.get_latest_release().is_ok());
        assert!(updater.get_latest_releases("1.0.0").is_ok());
        assert!(updater.get_release_version("1.0.0").is_ok());
    }

    // The descriptor-level tests below replace the process's fd 1 (and fd 2)
    // with observation files, so they must run serially and always restore the
    // saved descriptors before asserting to avoid leaving stdout broken.
    #[cfg(unix)]
    mod fd {
        use super::*;
        use std::io::{Read, Seek, SeekFrom};
        use std::os::fd::AsRawFd;

        fn scratch(name: &str) -> (std::fs::File, std::path::PathBuf) {
            let path = std::env::temp_dir().join(name);
            let _ = std::fs::remove_file(&path);
            let file = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            (file, path)
        }

        fn read_all(file: &mut std::fs::File) -> String {
            file.seek(SeekFrom::Start(0)).unwrap();
            let mut buf = String::new();
            file.read_to_string(&mut buf).unwrap();
            buf
        }

        // These tests swap the process-global fd 1, so the test harness's own
        // `test ... ok` progress lines can interleave into the observation
        // files. Assertions therefore check for the presence/absence of the
        // mock's distinctive markers rather than exact file contents.

        #[test]
        #[serial_test::serial(silence_stdout_fd)]
        fn null_sink_discards_stdout() {
            let (mut out, out_path) = scratch("mock-silence-null.out");

            // SAFETY: standard descriptor juggling; `saved` is restored below.
            let saved = unsafe { libc::dup(libc::STDOUT_FILENO) };
            assert!(saved >= 0);
            unsafe { libc::dup2(out.as_raw_fd(), libc::STDOUT_FILENO) };

            let updater = concrete(
                MockRelease::new("mock-silence-null").print_on_update("LEAK"),
                Sink::Null,
            );
            let _ = updater.update();

            unsafe {
                libc::dup2(saved, libc::STDOUT_FILENO);
                libc::close(saved);
            }

            assert!(
                !read_all(&mut out).contains("LEAK"),
                "stdout should have been discarded to /dev/null"
            );
            let _ = std::fs::remove_file(&out_path);
        }

        #[test]
        #[serial_test::serial(silence_stdout_fd)]
        fn stderr_sink_folds_stdout_into_stderr() {
            let (mut out, out_path) = scratch("mock-silence-stderr.out");
            let (mut err, err_path) = scratch("mock-silence-stderr.err");

            let saved_out = unsafe { libc::dup(libc::STDOUT_FILENO) };
            let saved_err = unsafe { libc::dup(libc::STDERR_FILENO) };
            assert!(saved_out >= 0 && saved_err >= 0);
            unsafe {
                libc::dup2(out.as_raw_fd(), libc::STDOUT_FILENO);
                libc::dup2(err.as_raw_fd(), libc::STDERR_FILENO);
            }

            let updater = concrete(
                MockRelease::new("mock-silence-stderr").print_on_update("HELLO"),
                Sink::Stderr,
            );
            let _ = updater.update();

            unsafe {
                libc::dup2(saved_out, libc::STDOUT_FILENO);
                libc::dup2(saved_err, libc::STDERR_FILENO);
                libc::close(saved_out);
                libc::close(saved_err);
            }

            assert!(
                !read_all(&mut out).contains("HELLO"),
                "stdout should not have carried the wrapped update's output"
            );
            assert!(
                read_all(&mut err).contains("HELLO"),
                "stdout should have been folded into stderr"
            );
            let _ = std::fs::remove_file(&out_path);
            let _ = std::fs::remove_file(&err_path);
        }

        #[test]
        #[serial_test::serial(silence_stdout_fd)]
        fn stdout_is_restored_after_update() {
            let (mut out, out_path) = scratch("mock-silence-restore.out");

            let saved = unsafe { libc::dup(libc::STDOUT_FILENO) };
            assert!(saved >= 0);
            unsafe { libc::dup2(out.as_raw_fd(), libc::STDOUT_FILENO) };

            let updater = concrete(
                MockRelease::new("mock-silence-restore").print_on_update("DURING"),
                Sink::Null,
            );
            let _ = updater.update();

            // Writing after the update must land in the original destination,
            // proving fd 1 was restored.
            let marker = b"AFTER";
            unsafe {
                libc::write(
                    libc::STDOUT_FILENO,
                    marker.as_ptr() as *const libc::c_void,
                    marker.len(),
                );
                libc::dup2(saved, libc::STDOUT_FILENO);
                libc::close(saved);
            }

            let observed = read_all(&mut out);
            assert!(
                observed.contains("AFTER"),
                "fd 1 must be restored so later writes reach the original stdout"
            );
            assert!(
                !observed.contains("DURING"),
                "the update's output must not have reached the original stdout"
            );
            let _ = std::fs::remove_file(&out_path);
        }
    }

    // The Windows counterparts swap the process-global standard-output (and
    // standard-error) handles, so they must run serially and restore the saved
    // handles before asserting. As with the Unix tests, the harness's progress
    // lines can interleave into the observation files, so assertions check for
    // the presence/absence of the mock's markers rather than exact contents.
    #[cfg(windows)]
    mod handle {
        use super::*;
        use crate::silence::win;
        use std::io::{Read, Seek, SeekFrom};
        use std::os::windows::io::AsRawHandle;

        fn scratch(name: &str) -> (std::fs::File, std::path::PathBuf) {
            let path = std::env::temp_dir().join(name);
            let _ = std::fs::remove_file(&path);
            let file = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            (file, path)
        }

        fn read_all(file: &mut std::fs::File) -> String {
            file.seek(SeekFrom::Start(0)).unwrap();
            let mut buf = String::new();
            file.read_to_string(&mut buf).unwrap();
            buf
        }

        #[test]
        #[serial_test::serial(silence_stdout_fd)]
        fn null_sink_discards_stdout() {
            let (mut out, out_path) = scratch("mock-silence-null-win.out");

            // SAFETY: standard handle juggling; `saved` is restored below.
            let saved = unsafe { win::GetStdHandle(win::STD_OUTPUT_HANDLE) };
            unsafe {
                win::SetStdHandle(win::STD_OUTPUT_HANDLE, out.as_raw_handle() as win::Handle)
            };

            let updater = concrete(
                MockRelease::new("mock-silence-null-win").print_on_update("LEAK"),
                Sink::Null,
            );
            let _ = updater.update();

            unsafe { win::SetStdHandle(win::STD_OUTPUT_HANDLE, saved) };

            assert!(
                !read_all(&mut out).contains("LEAK"),
                "stdout should have been discarded to the NUL device"
            );
            let _ = std::fs::remove_file(&out_path);
        }

        #[test]
        #[serial_test::serial(silence_stdout_fd)]
        fn stderr_sink_folds_stdout_into_stderr() {
            let (mut out, out_path) = scratch("mock-silence-stderr-win.out");
            let (mut err, err_path) = scratch("mock-silence-stderr-win.err");

            let saved_out = unsafe { win::GetStdHandle(win::STD_OUTPUT_HANDLE) };
            let saved_err = unsafe { win::GetStdHandle(win::STD_ERROR_HANDLE) };
            unsafe {
                win::SetStdHandle(win::STD_OUTPUT_HANDLE, out.as_raw_handle() as win::Handle);
                win::SetStdHandle(win::STD_ERROR_HANDLE, err.as_raw_handle() as win::Handle);
            }

            let updater = concrete(
                MockRelease::new("mock-silence-stderr-win").print_on_update("HELLO"),
                Sink::Stderr,
            );
            let _ = updater.update();

            unsafe {
                win::SetStdHandle(win::STD_OUTPUT_HANDLE, saved_out);
                win::SetStdHandle(win::STD_ERROR_HANDLE, saved_err);
            }

            assert!(
                !read_all(&mut out).contains("HELLO"),
                "stdout should not have carried the wrapped update's output"
            );
            assert!(
                read_all(&mut err).contains("HELLO"),
                "stdout should have been folded into stderr"
            );
            let _ = std::fs::remove_file(&out_path);
            let _ = std::fs::remove_file(&err_path);
        }

        #[test]
        #[serial_test::serial(silence_stdout_fd)]
        fn stdout_is_restored_after_update() {
            let (mut out, out_path) = scratch("mock-silence-restore-win.out");

            let saved = unsafe { win::GetStdHandle(win::STD_OUTPUT_HANDLE) };
            let observation = out.as_raw_handle() as win::Handle;
            unsafe { win::SetStdHandle(win::STD_OUTPUT_HANDLE, observation) };

            let updater = concrete(
                MockRelease::new("mock-silence-restore-win").print_on_update("DURING"),
                Sink::Null,
            );
            let _ = updater.update();

            // The wrapper must have restored our observation handle as stdout.
            let after = unsafe { win::GetStdHandle(win::STD_OUTPUT_HANDLE) };
            unsafe { win::SetStdHandle(win::STD_OUTPUT_HANDLE, saved) };

            assert_eq!(
                after, observation,
                "the standard-output handle must be restored after the update"
            );
            assert!(
                !read_all(&mut out).contains("DURING"),
                "the update's output must not have reached the original stdout"
            );
            let _ = std::fs::remove_file(&out_path);
        }
    }
}
