# auto-update

A minimal Rust library for wrapping self-updating logic with throttling and restart management. It accepts any type implementing `self_update::update::ReleaseUpdate` and adds automatic throttling (to limit check frequency) and restart handling (to prevent update loops and re-execute after updates).

## Purpose

This crate provides a thin wrapper around the `self_update` crate's `ReleaseUpdate` trait. It handles:

- **Throttling**: Limits update checks to a configurable time window (default: 15 minutes)
- **Restart guard**: Prevents update loops via environment variable
- **Process restart**: Re-executes with original arguments using Unix `exec()` semantics

The actual update source (GitHub, custom server, etc.) is provided by the caller through the `ReleaseUpdate` trait.

**Note on Windows**: The restart behavior is Unix-focused. Windows support can be enabled via `WindowsPolicy` but re-exec semantics differ.

## Usage

### Integration Example

```rust
use auto_update::{Updater, WindowsPolicy};
use self_update::update::ReleaseUpdate;
use std::time::Duration;

#[tokio::main]
async fn main() {
    // Implement ReleaseUpdate for your update source
    let release_update = MyReleaseUpdate::new(
        "hyper-mcp-rs",
        "hyper-mcp",
        "hyper-mcp"
    );

    let updater = Updater::new(release_update)
        .guard_env("HYPER_MCP_AUTO_UPDATED")  // Prevent restart loops
        .throttle_file("hyper-mcp-update-check")  // Throttle state file
        .throttle_window(Duration::from_secs(15 * 60))  // Check interval
        .windows_policy(WindowsPolicy::Disabled);  // Or Enabled

    if let Err(e) = updater.run().await {
        tracing::warn!(error = ?e, "Auto-update failed; continuing with the current version");
    }

    // Rest of your application...
}

// Your custom ReleaseUpdate implementation
struct MyReleaseUpdate {
    owner: String,
    repo: String,
    binary: String,
}

impl MyReleaseUpdate {
    fn new(owner: &str, repo: &str, binary: &str) -> Self {
        Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            binary: binary.to_string(),
        }
    }
}

impl ReleaseUpdate for MyReleaseUpdate {
    async fn update(&self) -> anyhow::Result<()> {
        // Use self_update's backends or your own logic
        let update = self_update::backends::github::Update::configure()
            .repo_owner(&self.owner)
            .repo_name(&self.repo)
            .bin_name(&self.binary)
            .current_version(self_update::cargo_crate_version!())
            .no_confirm(true)
            .show_download_progress(false)
            .target(get_target())
            .build()?;

        update.update()?;
        Ok(())
    }
}

fn get_target() -> &'static str {
    env!("BUILD_TARGET")
}
```

### Configuration

The `Updater` takes a user-provided `ReleaseUpdate` and adds throttling/restart behavior:

| Method | Description |
|--------|-------------|
| `guard_env(env)` | Environment variable to prevent restart loops (default: `"AUTO_UPDATE_GUARD"`) |
| `throttle_file(name)` | Throttle state file name (stored in `$TMPDIR`, default: `"auto-update-check"`) |
| `throttle_window(duration)` | Minimum interval between checks (default: 15 minutes) |
| `windows_policy(policy)` | Enable/disable auto-update on Windows (default: `Enabled`) |

The `ReleaseUpdate` trait requires implementing a single async method:

```rust
#[async_trait::async_trait]
pub trait ReleaseUpdate {
    async fn update(&self) -> Result<(), anyhow::Error>;
}
```

### Build Requirements

The crate requires the `BUILD_TARGET` environment variable to be set at build time (automatically provided by Cargo as `TARGET`). This ensures the correct target-specific asset is downloaded.

## License

Apache-2.0 — see [LICENSE](./LICENSE) for details.
