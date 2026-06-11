//! `vectorcode upgrade` — self-update the binary.
//!
//! Checks GitHub Releases for new versions, downloads the appropriate
//! platform binary, and atomically replaces the running executable.

use anyhow::{Context, Result};
use clap::Args;

/// Arguments for `vectorcode upgrade`.
#[derive(Args, Debug)]
pub struct UpgradeArgs {
    /// Check for updates without installing.
    #[arg(long)]
    pub check: bool,

    /// Install a specific version instead of latest.
    #[arg(long)]
    pub version: Option<String>,
}

/// Execute the `upgrade` command.
///
/// If `--check` is passed, only reports whether an update is available.
/// If `--version <VER>` is passed, installs that specific version.
/// Otherwise, checks for latest and performs self-update if available.
pub async fn execute(args: &UpgradeArgs) -> Result<()> {
    let current = current_version();

    // Determine target version
    let target_version = if let Some(ref v) = args.version {
        parse_version_tag(v).to_string()
    } else {
        let json = fetch_github_release_json().await?;
        parse_version_from_json(&json).context("Failed to parse GitHub release response")?
    };

    if args.check {
        let update_available = is_update_available(current, &target_version)?;
        println!("Current version: {current}");
        println!("Latest version: {target_version}");
        if update_available {
            println!("Update available! Run `vectorcode upgrade` to install.");
        } else {
            println!("Already up to date.");
        }
        return Ok(());
    }

    // Check if update needed (skip for explicit --version)
    if args.version.is_none() {
        let update_available = is_update_available(current, &target_version)?;
        if !update_available {
            println!("Already up to date (v{current}).");
            return Ok(());
        }
    }

    println!("Updating to v{target_version}...");
    let temp_dir = download_binary(&target_version).await?;
    let binary_path = temp_dir.path().join("vectorcode");

    // Sanity check: binary is not empty
    let metadata = std::fs::metadata(&binary_path).context("Extracted binary not found")?;
    if metadata.len() == 0 {
        anyhow::bail!("Downloaded binary is empty — release may be malformed");
    }

    // Atomic self-replace
    self_replace::self_replace(&binary_path).context("Failed to replace running binary")?;

    // chmod +x on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let current_exe = std::env::current_exe()?;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&current_exe, perms)?;
    }

    println!("Successfully updated to v{target_version}!");
    println!(
        "{} installed to {}.",
        env!("CARGO_PKG_NAME"),
        std::env::current_exe().unwrap_or_default().display()
    );
    #[cfg(target_os = "macos")]
    eprintln!(
        "NOTE: On macOS, restart your terminal or run 'exec $SHELL -l' to use the new version."
    );
    Ok(())
}

// ─── Internal helpers (async I/O) ────────────────────────────────────────────

/// Fetch the latest release JSON from GitHub Releases API.
async fn fetch_github_release_json() -> Result<String> {
    let client = reqwest::Client::builder()
        .user_agent("vectorcode-upgrade")
        .build()?;
    let resp = client
        .get("https://api.github.com/repos/alejandro-technology/vectorcode/releases/latest")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .context("Failed to reach GitHub Releases API")?;

    if !resp.status().is_success() {
        anyhow::bail!("GitHub API returned HTTP {}", resp.status());
    }

    resp.text()
        .await
        .context("Failed to read GitHub response body")
}

/// Download the release tarball for a given version and extract the binary.
///
/// Returns the temp directory containing the extracted `vectorcode` binary.
/// The caller is responsible for the directory lifetime.
///
/// If a `SHA256SUMS` file exists in the release, the tarball is verified.
/// If not, a warning is logged but the download proceeds.
async fn download_binary(version: &str) -> Result<tempfile::TempDir> {
    let target = release_target();
    let url = release_download_url(version, &target);

    eprintln!("Downloading from {url}...");

    let client = reqwest::Client::builder()
        .user_agent("vectorcode-upgrade")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;

    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to download release")?;

    if !response.status().is_success() {
        anyhow::bail!("Download failed: HTTP {}", response.status());
    }

    let bytes = response.bytes().await?;

    // Attempt SHA256 checksum verification
    verify_sha256_if_available(&client, version, &target, &bytes).await?;

    let temp_dir = tempfile::tempdir()?;
    extract_binary_from_tarball(&bytes, temp_dir.path())?;

    Ok(temp_dir)
}

/// Try to fetch `SHA256SUMS` from the release and verify the tarball.
///
/// If the sums file doesn't exist (404), logs a warning and returns Ok.
/// If verification fails, returns an error.
async fn verify_sha256_if_available(
    client: &reqwest::Client,
    version: &str,
    target: &str,
    tarball_bytes: &[u8],
) -> Result<()> {
    use sha2::Digest;

    let sums_url = release_sums_url(version);
    let resp = client
        .get(&sums_url)
        .send()
        .await
        .context("Failed to fetch SHA256SUMS")?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        tracing::warn!(
            "No SHA256SUMS found for this release — skipping checksum verification. \
             Consider adding checksums to your release artifacts."
        );
        return Ok(());
    }

    if !resp.status().is_success() {
        tracing::warn!(
            "SHA256SUMS fetch returned HTTP {} — skipping checksum verification.",
            resp.status()
        );
        return Ok(());
    }

    let sums_content = resp.text().await.context("Failed to read SHA256SUMS")?;
    let tarball_filename = format!("vectorcode-{target}.tar.gz");

    // Parse the SHA256SUMS file: each line is "<hash>  <filename>" or "<hash> <filename>"
    let expected_hash = sums_content.lines().find_map(|line| {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[1] == tarball_filename {
            Some(parts[0].to_string())
        } else {
            None
        }
    });

    let expected_hash = match expected_hash {
        Some(h) => h,
        None => {
            tracing::warn!(
                "SHA256SUMS does not contain an entry for {tarball_filename} — skipping verification."
            );
            return Ok(());
        }
    };

    // Compute actual SHA256
    let mut hasher = sha2::Sha256::new();
    hasher.update(tarball_bytes);
    let actual_hash = format!("{:x}", hasher.finalize());

    if actual_hash != expected_hash {
        anyhow::bail!(
            "SHA256 checksum mismatch!\n  Expected: {expected_hash}\n  Actual:   {actual_hash}\n\
             The downloaded release may be corrupted or tampered with. Aborting."
        );
    }

    eprintln!("SHA256 checksum verified.");
    Ok(())
}

// ─── Pure functions (easily testable, no I/O) ────────────────────────────────

/// Strip the leading 'v' or 'V' prefix from a version tag.
///
/// `parse_version_tag("v0.2.0")` → `"0.2.0"`
/// `parse_version_tag("0.2.0")` → `"0.2.0"`
fn parse_version_tag(tag: &str) -> &str {
    tag.strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag)
}

/// Parse the `tag_name` field from a GitHub release JSON response.
///
/// Returns the version string with any 'v' prefix stripped.
fn parse_version_from_json(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = value.get("tag_name")?.as_str()?;
    Some(parse_version_tag(tag).to_string())
}

/// Compare two semver strings. Returns `true` when `latest` is strictly
/// newer than `current`.
fn is_update_available(current: &str, latest: &str) -> Result<bool> {
    let current_ver = semver::Version::parse(current).context("Invalid current version format")?;
    let latest_ver = semver::Version::parse(latest).context("Invalid latest version format")?;
    Ok(latest_ver > current_ver)
}

/// Map the running platform to the release asset target triple suffix.
///
/// Examples: `"aarch64-apple-darwin"`, `"x86_64-unknown-linux-gnu"`.
fn release_target() -> String {
    format!("{}-{}", std::env::consts::ARCH, rustc_target_os())
}

/// Map `std::env::consts::OS` to the Rust/LLVM target OS component.
fn rustc_target_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        "windows" => "pc-windows-msvc",
        other => other,
    }
}

/// Build the full download URL for a release tarball.
fn release_download_url(version: &str, target: &str) -> String {
    format!(
        "https://github.com/alejandro-technology/vectorcode/releases/download/v{version}/vectorcode-{target}.tar.gz"
    )
}

/// Build the URL for the SHA256SUMS file in a release.
fn release_sums_url(version: &str) -> String {
    format!(
        "https://github.com/alejandro-technology/vectorcode/releases/download/v{version}/SHA256SUMS"
    )
}

/// Extract the `vectorcode` binary from a `.tar.gz` byte buffer into `dest_dir`.
fn extract_binary_from_tarball(data: &[u8], dest_dir: &std::path::Path) -> Result<()> {
    use flate2::read::GzDecoder;
    use tar::Archive;

    let decoder = GzDecoder::new(data);
    let mut archive = Archive::new(decoder);

    for entry in archive
        .entries()
        .context("Failed to read tarball entries")?
    {
        let mut entry = entry.context("Failed to read tar entry")?;
        let path = entry.path().context("Failed to read entry path")?;

        // Match the binary by file name (last component)
        let is_target = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == "vectorcode")
            .unwrap_or(false);

        if is_target {
            let dest = dest_dir.join("vectorcode");
            entry
                .unpack(&dest)
                .context("Failed to extract binary from tarball")?;
            return Ok(());
        }
    }

    anyhow::bail!("Binary 'vectorcode' not found in release tarball")
}

/// Return the version of the running binary.
fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;
    use flate2::write::GzEncoder;

    // ── Helpers ──────────────────────────────────────────────────────────

    /// Create a `.tar.gz` byte buffer containing a single file at `path`
    /// with `content`.
    fn create_test_tarball(file_path: &str, content: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let enc = GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut tar = tar::Builder::new(enc);
            let mut header = tar::Header::new_gnu();
            header.set_path(file_path).unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append(&header, content).unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        buf
    }

    // ── CLI argument parsing ─────────────────────────────────────────────

    #[test]
    fn upgrade_args_parse_defaults() {
        let cli = Cli::parse_from(["vectorcode", "upgrade"]);
        match cli.command {
            crate::cli::Commands::Upgrade(args) => {
                assert!(!args.check);
                assert!(args.version.is_none());
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn upgrade_args_parse_check_flag() {
        let cli = Cli::parse_from(["vectorcode", "upgrade", "--check"]);
        match cli.command {
            crate::cli::Commands::Upgrade(args) => {
                assert!(args.check);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn upgrade_args_parse_version_flag() {
        let cli = Cli::parse_from(["vectorcode", "upgrade", "--version", "0.3.0"]);
        match cli.command {
            crate::cli::Commands::Upgrade(args) => {
                assert_eq!(args.version.as_deref(), Some("0.3.0"));
                assert!(!args.check);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn upgrade_args_parse_check_and_version_together() {
        let cli = Cli::parse_from(["vectorcode", "upgrade", "--check", "--version", "0.2.0"]);
        match cli.command {
            crate::cli::Commands::Upgrade(args) => {
                assert!(args.check);
                assert_eq!(args.version.as_deref(), Some("0.2.0"));
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    // ── parse_version_tag ────────────────────────────────────────────────

    #[test]
    fn parse_version_tag_strips_lowercase_v() {
        assert_eq!(parse_version_tag("v0.2.0"), "0.2.0");
    }

    #[test]
    fn parse_version_tag_strips_uppercase_v() {
        assert_eq!(parse_version_tag("V1.0.0"), "1.0.0");
    }

    #[test]
    fn parse_version_tag_no_prefix_returns_unchanged() {
        assert_eq!(parse_version_tag("0.2.0"), "0.2.0");
    }

    #[test]
    fn parse_version_tag_preserves_prerelease_suffix() {
        assert_eq!(parse_version_tag("v1.0.0-rc.1"), "1.0.0-rc.1");
    }

    #[test]
    fn parse_version_tag_empty_string_returns_empty() {
        assert_eq!(parse_version_tag(""), "");
    }

    // ── parse_version_from_json ──────────────────────────────────────────

    #[test]
    fn parse_version_from_json_extracts_tag_name() {
        let json = r#"{"tag_name": "v0.2.0", "name": "Release 0.2.0"}"#;
        assert_eq!(parse_version_from_json(json), Some("0.2.0".to_string()));
    }

    #[test]
    fn parse_version_from_json_strips_v_prefix() {
        let json = r#"{"tag_name": "v1.3.5"}"#;
        assert_eq!(parse_version_from_json(json), Some("1.3.5".to_string()));
    }

    #[test]
    fn parse_version_from_json_no_v_prefix() {
        let json = r#"{"tag_name": "0.1.0"}"#;
        assert_eq!(parse_version_from_json(json), Some("0.1.0".to_string()));
    }

    #[test]
    fn parse_version_from_json_missing_tag_returns_none() {
        let json = r#"{"name": "no tag here"}"#;
        assert_eq!(parse_version_from_json(json), None);
    }

    #[test]
    fn parse_version_from_json_invalid_json_returns_none() {
        assert_eq!(parse_version_from_json("not json"), None);
    }

    #[test]
    fn parse_version_from_json_tag_not_string_returns_none() {
        let json = r#"{"tag_name": 42}"#;
        assert_eq!(parse_version_from_json(json), None);
    }

    // ── is_update_available ──────────────────────────────────────────────

    #[test]
    fn is_update_available_true_when_latest_is_newer_major() {
        assert!(is_update_available("0.1.0", "1.0.0").unwrap());
    }

    #[test]
    fn is_update_available_true_when_latest_is_newer_minor() {
        assert!(is_update_available("0.1.0", "0.2.0").unwrap());
    }

    #[test]
    fn is_update_available_true_when_latest_is_newer_patch() {
        assert!(is_update_available("0.1.0", "0.1.1").unwrap());
    }

    #[test]
    fn is_update_available_false_when_same_version() {
        assert!(!is_update_available("0.1.0", "0.1.0").unwrap());
    }

    #[test]
    fn is_update_available_false_when_current_is_newer() {
        assert!(!is_update_available("0.2.0", "0.1.0").unwrap());
    }

    #[test]
    fn is_update_available_false_when_current_is_prerelease_ahead() {
        // 1.0.0-rc.1 < 1.0.0 per semver, so current=1.0.0 is already newer
        assert!(!is_update_available("1.0.0", "1.0.0-rc.1").unwrap());
    }

    #[test]
    fn is_update_available_errors_on_invalid_current() {
        assert!(is_update_available("not-a-version", "0.2.0").is_err());
    }

    #[test]
    fn is_update_available_errors_on_invalid_latest() {
        assert!(is_update_available("0.1.0", "garbage").is_err());
    }

    // ── release_target ───────────────────────────────────────────────────

    #[test]
    fn release_target_has_arch_os_format() {
        let target = release_target();
        let parts: Vec<&str> = target.split('-').collect();
        // All Rust target triples have at least 2 components: arch-os
        assert!(
            parts.len() >= 2,
            "Expected at least arch-os in '{target}', got {parts:?}"
        );
        assert!(!parts[0].is_empty(), "arch component must not be empty");
    }

    #[test]
    fn release_target_matches_known_platform() {
        let target = release_target();
        let known = [
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-pc-windows-msvc",
        ];
        assert!(
            known.contains(&target.as_str()),
            "Unexpected target triple: {target}. If this is a new platform, add it to the known list."
        );
    }

    // ── release_download_url ─────────────────────────────────────────────

    #[test]
    fn release_download_url_contains_version_and_target() {
        let url = release_download_url("0.2.0", "aarch64-apple-darwin");
        assert_eq!(
            url,
            "https://github.com/alejandro-technology/vectorcode/releases/download/v0.2.0/vectorcode-aarch64-apple-darwin.tar.gz"
        );
    }

    #[test]
    fn release_download_url_linux_target() {
        let url = release_download_url("1.0.0", "x86_64-unknown-linux-gnu");
        assert!(url.contains("v1.0.0"));
        assert!(url.contains("x86_64-unknown-linux-gnu"));
        assert!(url.ends_with(".tar.gz"));
    }

    #[test]
    fn release_download_url_windows_target() {
        let url = release_download_url("0.3.0", "x86_64-pc-windows-msvc");
        assert!(url.contains("v0.3.0"));
        assert!(url.contains("x86_64-pc-windows-msvc"));
    }

    // ── current_version ──────────────────────────────────────────────────

    #[test]
    fn current_version_matches_cargo_toml() {
        assert_eq!(current_version(), "0.1.0");
    }

    #[test]
    fn current_version_is_valid_semver() {
        assert!(semver::Version::parse(current_version()).is_ok());
    }

    // ── extract_binary_from_tarball ──────────────────────────────────────

    #[test]
    fn extract_binary_from_tarball_finds_vectorcode_binary() {
        let tarball = create_test_tarball("vectorcode", b"fake-binary-content");
        let temp = tempfile::tempdir().unwrap();
        extract_binary_from_tarball(&tarball, temp.path()).unwrap();
        let extracted = std::fs::read(temp.path().join("vectorcode")).unwrap();
        assert_eq!(extracted, b"fake-binary-content");
    }

    #[test]
    fn extract_binary_from_tarball_finds_nested_binary() {
        let tarball = create_test_tarball("release/vectorcode", b"nested-binary");
        let temp = tempfile::tempdir().unwrap();
        extract_binary_from_tarball(&tarball, temp.path()).unwrap();
        let extracted = std::fs::read(temp.path().join("vectorcode")).unwrap();
        assert_eq!(extracted, b"nested-binary");
    }

    #[test]
    fn extract_binary_from_tarball_errors_when_binary_missing() {
        let tarball = create_test_tarball("other-file.txt", b"not the binary");
        let temp = tempfile::tempdir().unwrap();
        let result = extract_binary_from_tarball(&tarball, temp.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not found"),
            "Error should mention binary not found, got: {err_msg}"
        );
    }

    #[test]
    fn extract_binary_from_tarball_errors_on_empty_data() {
        let temp = tempfile::tempdir().unwrap();
        let result = extract_binary_from_tarball(&[], temp.path());
        assert!(result.is_err());
    }

    #[test]
    fn extract_binary_from_tarball_errors_on_invalid_gzip() {
        let temp = tempfile::tempdir().unwrap();
        let result = extract_binary_from_tarball(b"this is not gzip data", temp.path());
        assert!(result.is_err());
    }

    #[test]
    fn extract_binary_from_tarball_preserves_content_exactly() {
        // Binary content with all byte values 0..=255
        let content: Vec<u8> = (0..=255).collect();
        let tarball = create_test_tarball("vectorcode", &content);
        let temp = tempfile::tempdir().unwrap();
        extract_binary_from_tarball(&tarball, temp.path()).unwrap();
        let extracted = std::fs::read(temp.path().join("vectorcode")).unwrap();
        assert_eq!(extracted, content);
    }
}
