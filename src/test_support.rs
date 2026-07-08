//! Shared test doubles used by the crate's unit tests.

use self_update::Status;
use self_update::errors::Result;
use self_update::update::{Release, ReleaseUpdate, UpdateStatus};
use std::cell::Cell;
use std::rc::Rc;

/// Minimal Win32 bindings for writing straight to the stdout handle in tests.
#[cfg(windows)]
#[allow(non_snake_case)]
mod win {
    use std::ffi::c_void;

    pub type Handle = *mut c_void;

    pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;

    unsafe extern "system" {
        pub fn GetStdHandle(nStdHandle: u32) -> Handle;
        pub fn WriteFile(
            hFile: Handle,
            lpBuffer: *const c_void,
            nNumberOfBytesToWrite: u32,
            lpNumberOfBytesWritten: *mut u32,
            lpOverlapped: *mut c_void,
        ) -> i32;
    }
}

/// A minimal [`ReleaseUpdate`] used to observe how the wrappers drive the
/// underlying backend without performing any real network or file work.
pub(crate) struct MockRelease {
    bin_name: String,
    update_calls: Rc<Cell<usize>>,
    report_updated: bool,
    stdout_message: Option<String>,
}

impl MockRelease {
    pub(crate) fn new(bin_name: &str) -> Self {
        Self {
            bin_name: bin_name.to_string(),
            update_calls: Rc::new(Cell::new(0)),
            report_updated: false,
            stdout_message: None,
        }
    }

    /// Configure whether `update` reports `Updated` instead of `UpToDate`.
    pub(crate) fn report_updated(mut self, yes: bool) -> Self {
        self.report_updated = yes;
        self
    }

    /// Emit `message` on file descriptor 1 whenever `update`/`update_extended`
    /// runs. The write goes straight to the OS descriptor (bypassing the test
    /// harness's `print!` capture) so it models output from a real backend and
    /// is observable by the `silence` wrapper's descriptor-level redirect.
    pub(crate) fn print_on_update(mut self, message: &str) -> Self {
        self.stdout_message = Some(message.to_string());
        self
    }

    /// Write the configured message, if any, directly to file descriptor 1.
    fn emit(&self) {
        let Some(message) = &self.stdout_message else {
            return;
        };
        #[cfg(unix)]
        // SAFETY: writing bytes to the raw stdout descriptor; the pointer and
        // length describe a live `str` and the call has no aliasing concerns.
        unsafe {
            libc::write(
                libc::STDOUT_FILENO,
                message.as_ptr() as *const libc::c_void,
                message.len(),
            );
        }
        #[cfg(windows)]
        // SAFETY: writing bytes directly to the current stdout handle, which
        // bypasses the test harness's `print!` capture just as the raw
        // descriptor write does on Unix.
        unsafe {
            let handle = win::GetStdHandle(win::STD_OUTPUT_HANDLE);
            let mut written: u32 = 0;
            win::WriteFile(
                handle,
                message.as_ptr() as *const std::ffi::c_void,
                message.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            );
        }
        #[cfg(not(any(unix, windows)))]
        {
            use std::io::Write;
            print!("{message}");
            let _ = std::io::stdout().flush();
        }
    }

    /// A shared handle to the update-call counter, which remains observable
    /// after the mock has been boxed and moved into a wrapper.
    pub(crate) fn call_counter(&self) -> Rc<Cell<usize>> {
        Rc::clone(&self.update_calls)
    }
}

impl ReleaseUpdate for MockRelease {
    fn get_latest_release(&self) -> Result<Release> {
        Ok(Release::default())
    }

    fn get_latest_releases(&self, _current_version: &str) -> Result<Vec<Release>> {
        Ok(Vec::new())
    }

    fn get_release_version(&self, _ver: &str) -> Result<Release> {
        Ok(Release::default())
    }

    fn current_version(&self) -> String {
        "1.0.0".to_string()
    }

    fn target(&self) -> String {
        "test-target".to_string()
    }

    fn target_version(&self) -> Option<String> {
        None
    }

    fn bin_name(&self) -> String {
        self.bin_name.clone()
    }

    fn bin_install_path(&self) -> std::path::PathBuf {
        std::env::temp_dir().join(&self.bin_name)
    }

    fn bin_path_in_archive(&self) -> String {
        self.bin_name.clone()
    }

    fn show_download_progress(&self) -> bool {
        false
    }

    fn show_output(&self) -> bool {
        false
    }

    fn no_confirm(&self) -> bool {
        true
    }

    fn progress_template(&self) -> String {
        String::new()
    }

    fn progress_chars(&self) -> String {
        String::new()
    }

    fn auth_token(&self) -> Option<String> {
        None
    }

    fn update(&self) -> Result<Status> {
        self.update_calls.set(self.update_calls.get() + 1);
        self.emit();
        if self.report_updated {
            Ok(Status::Updated("2.0.0".to_string()))
        } else {
            Ok(Status::UpToDate(self.current_version()))
        }
    }

    fn update_extended(&self) -> Result<UpdateStatus> {
        self.update_calls.set(self.update_calls.get() + 1);
        self.emit();
        Ok(UpdateStatus::UpToDate)
    }
}
