use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub struct BumpResult {
    pub old_version: String,
    pub new_version: String,
}

/// Bump a template to a new version: rewrite version, reset revision, update checksum.
pub fn bump_template(void_pkgs: &Path, name: &str, new_version: &str) -> Result<BumpResult> {
    let template_path = void_pkgs.join("srcpkgs").join(name).join("template");
    let content = fs::read_to_string(&template_path)
        .with_context(|| format!("reading template: {}", template_path.display()))?;

    // Parse variables from template to resolve distfiles URL
    let vars = parse_template_vars(&content);
    let old_version = vars.get("version").cloned().unwrap_or_default();

    // Get the raw distfiles line (with ${version} unexpanded)
    let raw_distfiles = vars.get("distfiles").cloned().unwrap_or_default();

    // Resolve the download URL with the new version
    let download_url = resolve_distfiles_url(&raw_distfiles, &vars, new_version);

    if download_url.is_empty() {
        bail!("Could not resolve distfiles URL for {}", name);
    }

    // Download tarball and compute SHA256
    let new_checksum = download_and_checksum(&download_url)
        .with_context(|| format!("downloading {}", download_url))?;

    // Rewrite template file line by line
    let new_content = rewrite_template(&content, new_version, &new_checksum);
    fs::write(&template_path, &new_content)
        .with_context(|| format!("writing template: {}", template_path.display()))?;

    Ok(BumpResult {
        old_version,
        new_version: new_version.to_string(),
    })
}

/// Parse shell-style variable assignments from a template, keeping values unexpanded.
fn parse_template_vars(content: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let mut in_multiline: Option<String> = None;
    let mut multiline_buf = String::new();

    for line in content.lines() {
        if let Some(ref varname) = in_multiline {
            if line.contains('"') {
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
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some(idx) = trimmed.find('=') {
            let varname = trimmed[..idx].trim();
            if !varname.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let rest = trimmed[idx + 1..].trim();

            // Multiline
            if rest.starts_with('"') && rest.len() > 1 && !rest[1..].contains('"') {
                in_multiline = Some(varname.to_string());
                multiline_buf = rest[1..].trim().to_string();
                continue;
            }
            if rest == "\"" {
                in_multiline = Some(varname.to_string());
                multiline_buf.clear();
                continue;
            }

            let val = rest.trim_matches('"').to_string();
            vars.insert(varname.to_string(), val);
        }
    }

    vars
}

/// Resolve distfiles URL by substituting template variables with the new version.
fn resolve_distfiles_url(raw: &str, vars: &HashMap<String, String>, new_version: &str) -> String {
    let mut url = raw.to_string();

    // Substitute ${version} with new version
    url = url.replace("${version}", new_version);

    // Substitute other known variables. Sort longest key first so that a key
    // which is a prefix of another (e.g. $foo vs $foobar) doesn't corrupt the
    // longer match before it gets a chance to be replaced.
    let mut sorted_keys: Vec<&String> = vars.keys().collect();
    sorted_keys.sort_by(|a, b| b.len().cmp(&a.len()));
    for key in sorted_keys {
        if key == "version" {
            continue; // already handled
        }
        let resolved_val = vars[key].replace("${version}", new_version);
        url = url.replace(&format!("${{{}}}", key), &resolved_val);
        url = url.replace(&format!("${}", key), &resolved_val);
    }

    // distfiles can have ">filename" suffix — strip it
    if let Some(idx) = url.rfind('>') {
        url = url[..idx].to_string();
    }

    url.trim().to_string()
}

/// Download a URL and return its SHA256 hex digest.
fn download_and_checksum(url: &str) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("vpm/0.1")
        .build()?;

    let response = client.get(url).send()?.error_for_status()?;
    let bytes = response.bytes()?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
}

/// Rewrite template content: update version, reset revision to 1, update checksum.
fn rewrite_template(content: &str, new_version: &str, new_checksum: &str) -> String {
    let mut lines: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("version=") {
            // Preserve indentation
            let indent = &line[..line.len() - trimmed.len()];
            lines.push(format!("{}version={}", indent, new_version));
        } else if trimmed.starts_with("revision=") {
            let indent = &line[..line.len() - trimmed.len()];
            lines.push(format!("{}revision=1", indent));
        } else if trimmed.starts_with("checksum=") {
            let indent = &line[..line.len() - trimmed.len()];
            lines.push(format!("{}checksum={}", indent, new_checksum));
        } else {
            lines.push(line.to_string());
        }
    }

    let mut result = lines.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    result
}
