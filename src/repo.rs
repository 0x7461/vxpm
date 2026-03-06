use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::package::{self, Package, PackageState};

/// Discover custom packages by diffing master..custom branches.
pub fn discover_custom_packages(void_pkgs: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["log", "--name-only", "--pretty=format:", "master..custom", "--", "srcpkgs/"])
        .current_dir(void_pkgs)
        .output()
        .context("running git log")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut names: HashSet<String> = HashSet::new();

    for line in stdout.lines() {
        // Lines look like "srcpkgs/hyprutils/template"
        let parts: Vec<&str> = line.split('/').collect();
        if parts.len() >= 2 {
            let pkg_name = parts[1].to_string();
            names.insert(pkg_name);
        }
    }

    // Filter out symlink dirs (subpackages like hyprland-devel)
    let srcpkgs = void_pkgs.join("srcpkgs");
    let mut result: Vec<String> = names
        .into_iter()
        .filter(|name| {
            let pkg_dir = srcpkgs.join(name);
            // Real packages have a template file and are not symlinks
            !pkg_dir.is_symlink() && pkg_dir.join("template").exists()
        })
        .collect();

    result.sort();
    Ok(result)
}

/// Discover packages in srcpkgs/ that aren't committed to the custom branch yet.
/// Catches untracked (??) and staged-but-uncommitted (A ) templates.
pub fn discover_uncommitted_packages(void_pkgs: &Path, committed: &HashSet<String>) -> Vec<String> {
    let output = match Command::new("git")
        .args(["status", "--porcelain", "srcpkgs/"])
        .current_dir(void_pkgs)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let srcpkgs = void_pkgs.join("srcpkgs");
    let mut names: HashSet<String> = HashSet::new();

    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        let path = line[3..].trim();
        // Only care about untracked (??) and staged-new (A )
        if status != "??" && status != "A " {
            continue;
        }
        // Path looks like "srcpkgs/foo/" or "srcpkgs/foo/template"
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() < 2 || parts[0] != "srcpkgs" {
            continue;
        }
        let name = parts[1].to_string();
        if committed.contains(&name) {
            continue;
        }
        let pkg_dir = srcpkgs.join(&name);
        if !pkg_dir.is_symlink() && pkg_dir.join("template").exists() {
            names.insert(name);
        }
    }

    let mut result: Vec<String> = names.into_iter().collect();
    result.sort();
    result
}

/// Parse all discovered packages.
pub fn load_packages(void_pkgs: &Path, names: &[String]) -> Vec<Package> {
    names
        .iter()
        .filter_map(|name| {
            let template = void_pkgs.join("srcpkgs").join(name).join("template");
            match package::parse_template(&template) {
                Ok(pkg) => Some(pkg),
                Err(e) => {
                    eprintln!("Warning: failed to parse {}: {}", name, e);
                    None
                }
            }
        })
        .collect()
}

/// Query installed version via xbps-query.
pub fn query_installed(name: &str) -> Option<String> {
    let output = Command::new("xbps-query")
        .args(["-p", "pkgver", name])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let pkgver = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if pkgver.is_empty() {
        None
    } else {
        Some(pkgver)
    }
}

/// Scan hostdir/binpkgs for built .xbps files for a given package.
/// Returns the version_revision from the filename if found.
pub fn find_built_xbps(void_pkgs: &Path, name: &str) -> Option<String> {
    let dirs = [
        void_pkgs.join("hostdir/binpkgs"),
        void_pkgs.join("hostdir/binpkgs/custom"),
    ];

    let prefix = format!("{}-", name);
    let mut best: Option<String> = None;

    for dir in &dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                // Pattern: name-version_revision.arch.xbps
                if fname.starts_with(&prefix) && fname.ends_with(".xbps") {
                    // Extract version_revision from filename
                    // e.g., "hyprutils-0.11.0_1.x86_64.xbps" -> "0.11.0_1"
                    let after_name = &fname[prefix.len()..];
                    // Version always starts with a digit — skip subpackages
                    if !after_name.starts_with(|c: char| c.is_ascii_digit()) {
                        continue;
                    }
                    // Strip ".arch.xbps" from end: find last two dots
                    // Format: ver_rev.arch.xbps
                    let without_xbps = after_name.strip_suffix(".xbps").unwrap_or(after_name);
                    // Now strip .arch (e.g., ".x86_64" or ".noarch")
                    if let Some(dot_idx) = without_xbps.rfind('.') {
                        let ver_rev = &without_xbps[..dot_idx];
                        if !ver_rev.is_empty() {
                            if best.is_none() || best.as_deref().unwrap_or("") < ver_rev {
                                best = Some(ver_rev.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    best
}

/// Build full PackageState for all packages.
pub fn build_package_states(void_pkgs: &Path, packages: Vec<Package>, uncommitted: &HashSet<String>) -> Vec<PackageState> {
    packages
        .into_iter()
        .map(|pkg| {
            let installed = query_installed(&pkg.name);
            let built = find_built_xbps(void_pkgs, &pkg.name);
            let status = PackageState::compute_status(&pkg, &installed, &built, &None);
            let is_uncommitted = uncommitted.contains(&pkg.name);
            PackageState {
                package: pkg,
                installed,
                built,
                latest: None,
                status,
                uncommitted: is_uncommitted,
                shlibs: Vec::new(),
                soname_mismatches: Vec::new(),
                build_log: None,
            }
        })
        .collect()
}
