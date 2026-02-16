use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub revision: u32,
    pub short_desc: String,
    pub homepage: String,
    pub build_style: String,
    pub makedepends: Vec<String>,
    pub hostmakedepends: Vec<String>,
    pub depends: Vec<String>,
    pub distfiles: String,
    pub changelog: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageState {
    pub package: Package,
    pub installed: Option<String>,
    pub built: Option<String>,
    pub latest: Option<String>,
    pub status: Status,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum Status {
    Ok,
    NotInstalled,
    BuildReady,
    NeedsBuild,
    UpdateAvailable,
    BuildFailed,
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::NotInstalled => "NOT INSTALLED",
            Status::BuildReady => "BUILD READY",
            Status::NeedsBuild => "NEEDS BUILD",
            Status::UpdateAvailable => "UPDATE AVAIL",
            Status::BuildFailed => "BUILD FAILED",
        }
    }

    #[allow(dead_code)]
    pub fn priority(&self) -> u8 {
        match self {
            Status::BuildFailed => 0,
            Status::NotInstalled => 1,
            Status::UpdateAvailable => 2,
            Status::NeedsBuild => 3,
            Status::BuildReady => 4,
            Status::Ok => 5,
        }
    }
}

impl PackageState {
    pub fn compute_status(
        package: &Package,
        installed: &Option<String>,
        built: &Option<String>,
        latest: &Option<String>,
    ) -> Status {
        let template_ver = format!("{}_{}",  package.version, package.revision);

        // Not installed at all
        if installed.is_none() {
            // If a built .xbps exists, it's ready to install
            if built.is_some() {
                return Status::BuildReady;
            }
            return Status::NotInstalled;
        }

        let inst_ver = installed.as_ref().unwrap();

        // Check if upstream has a newer version than the template
        if let Some(latest_ver) = latest {
            if version_newer(latest_ver, &package.version) {
                return Status::UpdateAvailable;
            }
        }

        // Template is newer than what's built
        if let Some(built_ver) = built {
            if *built_ver != template_ver {
                return Status::NeedsBuild;
            }
            // Built version exists and is newer than installed
            let inst_short = installed_to_ver_rev(inst_ver);
            if *built_ver != inst_short {
                return Status::BuildReady;
            }
        } else {
            // No built .xbps at all — check if template matches installed
            let inst_short = installed_to_ver_rev(inst_ver);
            if inst_short != template_ver {
                return Status::NeedsBuild;
            }
        }

        Status::Ok
    }

    pub fn description(&self) -> String {
        let p = &self.package;
        let template_ver = format!("{}_{}", p.version, p.revision);
        match &self.status {
            Status::NotInstalled => {
                "Template exists but package is not installed on the system.".to_string()
            }
            Status::UpdateAvailable => {
                let latest = self.latest.as_deref().unwrap_or("?");
                format!(
                    "Upstream has released v{}, but template defines v{}. Update the template to build the new version.",
                    latest, p.version
                )
            }
            Status::NeedsBuild => {
                format!(
                    "Template defines v{} but no matching .xbps has been built. Run `xbps-src pkg {}` to build.",
                    template_ver, p.name
                )
            }
            Status::BuildReady => {
                let built = self.built.as_deref().unwrap_or("?");
                let inst = self.installed.as_deref().unwrap_or("?");
                format!(
                    "v{} is built and ready to install. Installed: {}. Run `xi {}` to update.",
                    built, inst, p.name
                )
            }
            Status::Ok => {
                let inst = self.installed.as_deref().unwrap_or("?");
                format!(
                    "Installed {} matches the template. No upstream updates detected.",
                    inst
                )
            }
            Status::BuildFailed => {
                format!(
                    "Build of {} failed. Check the build log for details.",
                    p.name
                )
            }
        }
    }
}

/// Compare two version strings. Returns true if `a` is newer than `b`.
/// Simple comparison: split on '.', compare numeric parts left to right.
fn version_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    for i in 0..va.len().max(vb.len()) {
        let pa = va.get(i).copied().unwrap_or(0);
        let pb = vb.get(i).copied().unwrap_or(0);
        if pa > pb {
            return true;
        }
        if pa < pb {
            return false;
        }
    }
    false
}

/// Extract "version_revision" from xbps-query pkgver like "hyprutils-0.11.0_1"
fn installed_to_ver_rev(pkgver: &str) -> String {
    // Format: name-version_revision
    // Find the last '-' which separates name from version
    if let Some(idx) = pkgver.rfind('-') {
        pkgver[idx + 1..].to_string()
    } else {
        pkgver.to_string()
    }
}

/// Parse a void-packages template file into a Package.
pub fn parse_template(path: &Path) -> Result<Package> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading template: {}", path.display()))?;

    let mut vars: HashMap<String, String> = HashMap::new();
    let mut in_multiline: Option<String> = None;
    let mut multiline_buf = String::new();

    for line in content.lines() {
        // If we're accumulating a multiline value
        if let Some(ref varname) = in_multiline {
            if line.contains('"') {
                // Closing quote found
                let before_quote = line.split('"').next().unwrap_or("");
                multiline_buf.push(' ');
                multiline_buf.push_str(before_quote.trim());
                let varname = varname.clone();
                vars.insert(varname, multiline_buf.trim().to_string());
                multiline_buf.clear();
                in_multiline = None;
            } else {
                multiline_buf.push(' ');
                multiline_buf.push_str(line.trim());
            }
            continue;
        }

        let trimmed = line.trim();

        // Skip comments, empty lines, functions, conditionals
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("if ")
            || trimmed.starts_with("fi")
            || trimmed.starts_with("then")
            || trimmed.starts_with("else")
            || trimmed.ends_with("() {")
            || trimmed == "}"
            || trimmed.starts_with("vmove")
            || trimmed.starts_with("vlicense")
            || trimmed.starts_with("vinstall")
            || trimmed.starts_with("vbin")
            || trimmed.starts_with("vman")
            || trimmed.starts_with("vsed")
            || trimmed.starts_with("vcompletion")
            || trimmed.starts_with("vcopy")
            || trimmed.starts_with("ln ")
            || trimmed.starts_with("sed ")
            || trimmed.starts_with("chmod")
            || trimmed.starts_with("mkdir")
            || trimmed.starts_with("cat ")
            || trimmed.starts_with("install ")
            || trimmed.starts_with("rm ")
            || trimmed.starts_with("local ")
            || trimmed.starts_with("pkg_install")
            || trimmed.starts_with("depends=")
                && trimmed.contains("sourcepkg")
        {
            continue;
        }

        // Handle append: var+=" value"
        if let Some(idx) = trimmed.find("+=") {
            let varname = trimmed[..idx].trim().to_string();
            let rest = trimmed[idx + 2..].trim();
            let val = unquote(rest);
            let existing = vars.entry(varname).or_default();
            if !existing.is_empty() {
                existing.push(' ');
            }
            // Handle multiline append
            if rest.starts_with('"') && !rest[1..].contains('"') {
                // Opening quote but no closing — multiline
                let varkey = trimmed[..idx].trim();
                let existing_val = vars.get(varkey).cloned().unwrap_or_default();
                multiline_buf = format!("{} {}", existing_val, val);
                in_multiline = Some(varkey.to_string());
            } else {
                existing.push_str(&val);
            }
            continue;
        }

        // Handle assignment: var=value or var="value" or var=$othervar
        if let Some(idx) = trimmed.find('=') {
            // Make sure this isn't inside a function or conditional
            let varname = trimmed[..idx].trim();
            if !varname.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let rest = trimmed[idx + 1..].trim();

            // Variable reference: var=$other
            if rest.starts_with('$') && !rest.contains('{') {
                let ref_name = rest.trim_start_matches('$');
                if let Some(ref_val) = vars.get(ref_name) {
                    vars.insert(varname.to_string(), ref_val.clone());
                }
                continue;
            }

            // Multiline: starts with " but no closing "
            let unq = rest.trim_matches('"');
            if rest.starts_with('"') && rest.len() > 1 && !rest[1..].contains('"') {
                in_multiline = Some(varname.to_string());
                multiline_buf = unq.trim().to_string();
                continue;
            }

            // Check for opening quote only (like just `"`)
            if rest == "\"" {
                in_multiline = Some(varname.to_string());
                multiline_buf.clear();
                continue;
            }

            vars.insert(varname.to_string(), unquote(rest));
        }
    }

    let version = vars.get("version").cloned().unwrap_or_default();

    // Resolve ${version} in distfiles
    let distfiles = vars
        .get("distfiles")
        .cloned()
        .unwrap_or_default()
        .replace("${version}", &version);

    Ok(Package {
        name: vars.get("pkgname").cloned().unwrap_or_default(),
        version,
        revision: vars
            .get("revision")
            .and_then(|r| r.parse().ok())
            .unwrap_or(1),
        short_desc: vars.get("short_desc").cloned().unwrap_or_default(),
        homepage: vars.get("homepage").cloned().unwrap_or_default(),
        build_style: vars.get("build_style").cloned().unwrap_or_default(),
        makedepends: split_deps(vars.get("makedepends").map(|s| s.as_str()).unwrap_or("")),
        hostmakedepends: split_deps(
            vars.get("hostmakedepends").map(|s| s.as_str()).unwrap_or(""),
        ),
        depends: split_deps(vars.get("depends").map(|s| s.as_str()).unwrap_or("")),
        distfiles,
        changelog: vars.get("changelog").cloned().unwrap_or_default(),
    })
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn split_deps(s: &str) -> Vec<String> {
    s.split_whitespace()
        .filter(|d| !d.is_empty())
        .map(|d| d.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_newer() {
        assert!(version_newer("0.12.0", "0.11.0"));
        assert!(version_newer("1.0.0", "0.99.99"));
        assert!(!version_newer("0.11.0", "0.11.0"));
        assert!(!version_newer("0.10.0", "0.11.0"));
    }

    #[test]
    fn test_installed_to_ver_rev() {
        assert_eq!(installed_to_ver_rev("hyprutils-0.11.0_1"), "0.11.0_1");
        assert_eq!(
            installed_to_ver_rev("google-chrome-133.0.6943.53_1"),
            "133.0.6943.53_1"
        );
    }
}
