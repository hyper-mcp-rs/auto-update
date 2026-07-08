//! Shared test doubles used by the crate's unit tests.

use self_update::Status;
use self_update::errors::Result;
use self_update::update::{Release, ReleaseUpdate, UpdateStatus};
use std::cell::Cell;
use std::rc::Rc;

/// A minimal [`ReleaseUpdate`] used to observe how the wrappers drive the
/// underlying backend without performing any real network or file work.
pub(crate) struct MockRelease {
    bin_name: String,
    update_calls: Rc<Cell<usize>>,
    report_updated: bool,
}

impl MockRelease {
    pub(crate) fn new(bin_name: &str) -> Self {
        Self {
            bin_name: bin_name.to_string(),
            update_calls: Rc::new(Cell::new(0)),
            report_updated: false,
        }
    }

    /// Configure whether `update` reports `Updated` instead of `UpToDate`.
    pub(crate) fn report_updated(mut self, yes: bool) -> Self {
        self.report_updated = yes;
        self
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
        if self.report_updated {
            Ok(Status::Updated("2.0.0".to_string()))
        } else {
            Ok(Status::UpToDate(self.current_version()))
        }
    }

    fn update_extended(&self) -> Result<UpdateStatus> {
        self.update_calls.set(self.update_calls.get() + 1);
        Ok(UpdateStatus::UpToDate)
    }
}
