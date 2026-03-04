//! Build script: optionally build BootCommander for release and copy next to the binary.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Only run BootCommander build for release
    if env::var("PROFILE").unwrap_or_default() != "release" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let target_dir = env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("target"))
        .join("release");

    let script = manifest_dir.join("scripts/build-bootcommander.sh");

    if !script.exists() {
        return;
    }

    // Run build script from project root, output to target/release
    let status = Command::new("bash")
        .arg(&script)
        .arg(&target_dir)
        .current_dir(&manifest_dir)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:warning=BootCommander built and copied to {}", target_dir.display());
        }
        Ok(s) => {
            // Non-zero exit - maybe cmake/libusb not installed, skip silently
            eprintln!("cargo:warning=BootCommander build failed (exit {}). Install cmake and libusb, or run ./scripts/build-bootcommander.sh manually.", s.code().unwrap_or(-1));
        }
        Err(e) => {
            eprintln!("cargo:warning=Could not run BootCommander build script: {}. Run ./scripts/build-bootcommander.sh manually.", e);
        }
    }
}
