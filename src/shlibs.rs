use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct ShlibEntry {
    pub soname: String,
    #[allow(dead_code)]
    pub pkg_ver: String,
}

#[derive(Debug, Clone)]
pub struct SonameMismatch {
    pub registered: String,
    pub installed: String,
}

pub type ShlibMap = HashMap<String, Vec<ShlibEntry>>;

/// Parse common/shlibs from void-packages, returning a map of package name -> shlib entries.
pub fn parse_shlibs(void_pkgs: &Path) -> ShlibMap {
    let shlibs_path = void_pkgs.join("common/shlibs");
    let content = match std::fs::read_to_string(&shlibs_path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };

    let mut map: ShlibMap = HashMap::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Format: "libfoo.so.1 foo-1.2.3_1"
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        let soname = parts[0].to_string();
        let pkg_ver = parts[1].to_string();

        // Extract package name: everything before the last '-'
        let pkg_name = match pkg_ver.rfind('-') {
            Some(idx) => pkg_ver[..idx].to_string(),
            None => continue,
        };

        map.entry(pkg_name)
            .or_default()
            .push(ShlibEntry { soname, pkg_ver });
    }

    map
}

/// Get the actual SONAMEs installed on the system for a package.
pub fn get_installed_sonames(pkg_name: &str) -> Vec<String> {
    // Get list of .so files owned by the package
    let file_list = match Command::new("xbps-query")
        .args(["-f", pkg_name])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => return Vec::new(),
    };

    let so_files: Vec<&str> = file_list
        .lines()
        .filter(|l| l.contains(".so"))
        // Only actual .so files, not symlinks to versioned ones
        .filter(|l| {
            let path = l.trim().trim_start_matches(" - ");
            // We want files like /usr/lib/libfoo.so.1.2.3 (the actual SONAME carriers)
            path.contains(".so.")
                || path.ends_with(".so")
        })
        .collect();

    let mut sonames = Vec::new();

    for so_file in &so_files {
        let path = so_file.trim().trim_start_matches(" - ");
        if !Path::new(path).exists() {
            continue;
        }

        if let Ok(output) = Command::new("readelf").args(["-d", path]).output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("(SONAME)") {
                    // Format: 0x... (SONAME)  Library soname: [libfoo.so.1]
                    if let Some(start) = line.find('[') {
                        if let Some(end) = line.find(']') {
                            let soname = line[start + 1..end].to_string();
                            if !sonames.contains(&soname) {
                                sonames.push(soname);
                            }
                        }
                    }
                }
            }
        }
    }

    sonames
}

/// Compare registered SONAMEs against what's actually installed.
pub fn check_soname_mismatches(
    shlibs: &[ShlibEntry],
    pkg_name: &str,
) -> Vec<SonameMismatch> {
    if shlibs.is_empty() {
        return Vec::new();
    }

    let installed = get_installed_sonames(pkg_name);
    let mut mismatches = Vec::new();

    for entry in shlibs {
        let registered = &entry.soname;

        if installed.contains(registered) {
            continue;
        }

        // Find the closest match by base name (e.g. libfoo.so.*)
        let base = soname_base(registered);
        let found = installed
            .iter()
            .find(|s| soname_base(s) == base);

        let installed_str = match found {
            Some(s) => s.clone(),
            None if installed.is_empty() => "not found".to_string(),
            None => "not found".to_string(),
        };

        mismatches.push(SonameMismatch {
            registered: registered.clone(),
            installed: installed_str,
        });
    }

    mismatches
}

/// Update common/shlibs file with new SONAME entries.
/// Each tuple is (old_soname, new_soname, new_pkg_ver).
pub fn update_shlibs_file(void_pkgs: &Path, updates: &[(String, String, String)]) {
    let shlibs_path = void_pkgs.join("common/shlibs");
    let content = match std::fs::read_to_string(&shlibs_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    for (old_soname, new_soname, new_pkg_ver) in updates {
        // Skip "not found" entries — can't update what doesn't exist
        if new_soname == "not found" {
            continue;
        }
        for line in &mut lines {
            let trimmed = line.trim();
            if trimmed.starts_with(old_soname) {
                let after = &trimmed[old_soname.len()..];
                if after.starts_with(' ') || after.starts_with('\t') {
                    *line = format!("{} {}", new_soname, new_pkg_ver);
                }
            }
        }
    }

    let new_content = lines.join("\n");
    // Preserve trailing newline if original had one
    let final_content = if content.ends_with('\n') {
        format!("{}\n", new_content)
    } else {
        new_content
    };

    let _ = std::fs::write(&shlibs_path, final_content);
}

/// Extract base library name: "libfoo.so.4" -> "libfoo.so"
fn soname_base(soname: &str) -> &str {
    match soname.find(".so.") {
        Some(idx) => &soname[..idx + 3], // include ".so"
        None => soname,
    }
}
