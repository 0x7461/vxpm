use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

pub struct GccInfo {
    pub system_version: Option<(u32, u32, u32)>,
    pub requirements: HashMap<String, (u32, u32)>,
}

impl GccInfo {
    pub fn detect() -> Self {
        let system_version = detect_gcc_version();
        let requirements = load_requirements();
        GccInfo {
            system_version,
            requirements,
        }
    }

    /// Returns true if the package requires a newer GCC than what's installed.
    pub fn is_blocked(&self, pkg_name: &str) -> bool {
        let system = match self.system_version {
            Some(v) => v,
            None => return false, // Can't determine — don't block
        };
        match self.requirements.get(pkg_name) {
            Some(&required) => !is_gcc_sufficient(system, required),
            None => false,
        }
    }

    pub fn version_string(&self) -> String {
        match self.system_version {
            Some((maj, min, patch)) => format!("{}.{}.{}", maj, min, patch),
            None => "unknown".to_string(),
        }
    }

    pub fn required_version(&self, pkg_name: &str) -> Option<String> {
        self.requirements
            .get(pkg_name)
            .map(|(maj, min)| format!("{}.{}", maj, min))
    }
}

fn detect_gcc_version() -> Option<(u32, u32, u32)> {
    let output = Command::new("gcc").arg("-dumpversion").output().ok()?;
    let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_version_tuple(&version_str)
}

fn parse_version_tuple(s: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<&str> = s.split('.').collect();
    let major = parts.first()?.parse().ok()?;
    let minor = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

fn is_gcc_sufficient(system: (u32, u32, u32), required: (u32, u32)) -> bool {
    if system.0 > required.0 {
        return true;
    }
    if system.0 == required.0 && system.1 >= required.1 {
        return true;
    }
    false
}

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".config/vpm/gcc_requirements.toml")
}

fn load_requirements() -> HashMap<String, (u32, u32)> {
    let path = config_path();

    // Bootstrap config if it doesn't exist
    if !path.exists() {
        bootstrap_config(&path);
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };

    let table: toml::Table = match content.parse() {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };

    let mut map = HashMap::new();

    if let Some(reqs) = table.get("requirements").and_then(|v| v.as_table()) {
        for (pkg, val) in reqs {
            if let Some(ver_str) = val.as_str() {
                if let Some(ver) = parse_major_minor(ver_str) {
                    map.insert(pkg.clone(), ver);
                }
            }
        }
    }

    map
}

fn parse_major_minor(s: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = s.split('.').collect();
    let major = parts.first()?.parse().ok()?;
    let minor = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

fn bootstrap_config(path: &PathBuf) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let default = r#"# GCC version requirements for packages
# Format: package_name = "major.minor"
# Packages listed here will show a warning badge if the system GCC
# is older than the required version, and builds will be blocked.

[requirements]
# hyprland = "15.0"
# hyprland-qt-support = "15.0"
"#;

    let _ = std::fs::write(path, default);
}
