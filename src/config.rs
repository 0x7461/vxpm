use std::path::PathBuf;

pub struct Config {
    pub void_packages: PathBuf,
}

pub fn load() -> Config {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let config_path = PathBuf::from(&home).join(".config/vpm/config.toml");

    if !config_path.exists() {
        bootstrap(&config_path, &home);
    }

    let void_packages = match std::fs::read_to_string(&config_path) {
        Ok(content) => {
            let table: toml::Table = content.parse().unwrap_or_default();
            table
                .get("void_packages")
                .and_then(|v| v.as_str())
                .map(|s| expand_tilde(s, &home))
                .unwrap_or_else(|| PathBuf::from(&home).join("void-packages"))
        }
        Err(_) => PathBuf::from(&home).join("void-packages"),
    };

    Config { void_packages }
}

fn expand_tilde(path: &str, home: &str) -> PathBuf {
    if path.starts_with("~/") {
        PathBuf::from(home).join(&path[2..])
    } else if path == "~" {
        PathBuf::from(home)
    } else {
        PathBuf::from(path)
    }
}

fn bootstrap(path: &PathBuf, home: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let default = format!(
        r#"# VPM configuration
void_packages = "{}/void-packages"
"#,
        home
    );

    let _ = std::fs::write(path, default);
}
