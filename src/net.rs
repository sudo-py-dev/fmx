//! Shared HTTPS client and atomic install of a downloaded executable.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// An HTTP client with Fmx's download timeouts.
pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_per_read(Some(READ_TIMEOUT))
        .build()
        .into()
}

/// Marks a finished temporary file executable and moves it to `target` in one
/// step, so a half-written program is never left in place.
pub(crate) fn persist_executable(
    mut tmp: tempfile::NamedTempFile,
    target: &Path,
) -> std::io::Result<()> {
    tmp.as_file_mut().flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o755))?;
    }
    tmp.persist(target).map_err(|e| e.error)?;
    Ok(())
}
