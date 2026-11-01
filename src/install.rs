use crate::printer::get_render_config;
use anyhow::{Result, anyhow};
use colored::Colorize;
use inquire::Select;
use serde_json::Value;
use std::{
    fmt::Display,
    fs,
    path::Path,
    process::{Command, Stdio},
};

const PACKAGE_MANAGERS: [&str; 4] = ["npm", "yarn", "pnpm", "bun"];

#[derive(Debug, PartialEq)]
enum PackageManager {
    Npm,
    Yarn,
    Pnpm,
    Bun,
}

impl PackageManager {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "npm" => Some(PackageManager::Npm),
            "yarn" => Some(PackageManager::Yarn),
            "pnpm" => Some(PackageManager::Pnpm),
            "bun" => Some(PackageManager::Bun),
            _ => None,
        }
    }

    /// Reads the package manager declared in a `package.json` through the
    /// `packageManager` field (e.g. `pnpm@10.0.0`) or `devEngines.packageManager`.
    fn from_package_json(content: &str) -> Option<Self> {
        let json: Value = serde_json::from_str(content).ok()?;

        if let Some(field) = json.get("packageManager").and_then(Value::as_str) {
            let name = field.split('@').next().unwrap_or(field);
            if let Some(package_manager) = Self::from_name(name) {
                return Some(package_manager);
            }
        }

        match json.get("devEngines")?.get("packageManager")? {
            Value::Array(entries) => entries.iter().find_map(Self::from_dev_engine),
            entry => Self::from_dev_engine(entry),
        }
    }

    fn from_dev_engine(entry: &Value) -> Option<Self> {
        Self::from_name(entry.get("name")?.as_str()?)
    }

    fn from_lockfile(dir: &Path) -> Option<Self> {
        let exists = |name: &str| fs::metadata(dir.join(name)).is_ok();

        if exists("package-lock.json") {
            Some(PackageManager::Npm)
        } else if exists("bun.lockb") || exists("bun.lock") {
            Some(PackageManager::Bun)
        } else if exists("yarn.lock") {
            Some(PackageManager::Yarn)
        } else if exists("pnpm-lock.yaml") {
            Some(PackageManager::Pnpm)
        } else {
            None
        }
    }

    /// Name of the executable to spawn. Rust only resolves `.exe` programs on
    /// Windows, while npm, yarn and pnpm (and their corepack shims) are `.cmd` files.
    fn program(&self, windows: bool) -> String {
        match self {
            PackageManager::Bun => self.to_string(),
            _ if windows => format!("{self}.cmd"),
            _ => self.to_string(),
        }
    }

    fn resolve_in(dir: &Path) -> Result<Self> {
        let declared = fs::read_to_string(dir.join("package.json"))
            .ok()
            .and_then(|content| Self::from_package_json(&content));

        if let Some(package_manager) = declared.or_else(|| Self::from_lockfile(dir)) {
            return Ok(package_manager);
        }

        let package_manager = Select::new("Select a package manager to use", PACKAGE_MANAGERS.to_vec())
            .with_render_config(get_render_config())
            .with_vim_mode(true)
            .without_filtering()
            .with_help_message("Enter to select")
            .prompt();

        package_manager
            .ok()
            .and_then(Self::from_name)
            .ok_or_else(|| anyhow!("No package manager selected"))
    }
}

impl Display for PackageManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageManager::Npm => write!(f, "npm"),
            PackageManager::Yarn => write!(f, "yarn"),
            PackageManager::Pnpm => write!(f, "pnpm"),
            PackageManager::Bun => write!(f, "bun"),
        }
    }
}

/// Runs the package manager's `install` command in `root`. When `quiet`, nothing is written
/// to stdout (the note goes to stderr and the command's stdout is redirected there too).
pub fn install(root: &Path, quiet: bool) -> Result<()> {
    let package_manager = PackageManager::resolve_in(root)?;

    let note = format!(
        " {}",
        format!("Note: running install command using {package_manager}...").bright_black()
    );
    if quiet {
        eprintln!("{note}");
        eprintln!();
    } else {
        println!("{note}");
        println!();
    }

    let stdout = if quiet {
        Stdio::from(std::io::stderr())
    } else {
        Stdio::inherit()
    };

    let mut command = Command::new(package_manager.program(cfg!(windows)))
        .arg("install")
        .current_dir(root)
        .stdout(stdout)
        .stderr(Stdio::inherit())
        .spawn()?;

    let status = command.wait()?;
    if !status.success() {
        return Err(anyhow!("Install command failed"));
    }

    if !quiet {
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::collect::collect_packages;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("skif-install-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_detect_package_manager() {
        for (lockfile, expected) in [
            ("package-lock.json", PackageManager::Npm),
            ("bun.lockb", PackageManager::Bun),
            ("bun.lock", PackageManager::Bun),
            ("yarn.lock", PackageManager::Yarn),
            ("pnpm-lock.yaml", PackageManager::Pnpm),
        ] {
            let dir = temp_dir(lockfile);
            fs::File::create(dir.join(lockfile)).unwrap();
            assert_eq!(PackageManager::resolve_in(&dir).unwrap(), expected);
            fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn test_package_manager_field_beats_lockfile() {
        let dir = temp_dir("field");
        fs::File::create(dir.join("package-lock.json")).unwrap();
        fs::write(dir.join("package.json"), r#"{ "packageManager": "pnpm@10.0.0" }"#).unwrap();
        assert_eq!(PackageManager::resolve_in(&dir).unwrap(), PackageManager::Pnpm);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_package_json_without_package_manager_uses_lockfile() {
        let dir = temp_dir("nofield");
        fs::File::create(dir.join("yarn.lock")).unwrap();
        fs::write(dir.join("package.json"), r#"{ "name": "x" }"#).unwrap();
        assert_eq!(PackageManager::resolve_in(&dir).unwrap(), PackageManager::Yarn);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_from_package_json() {
        for (json, expected) in [
            (r#"{ "packageManager": "pnpm@10.0.0" }"#, Some(PackageManager::Pnpm)),
            (
                r#"{ "packageManager": "yarn@4.1.0+sha512.abc" }"#,
                Some(PackageManager::Yarn),
            ),
            (r#"{ "packageManager": "bun" }"#, Some(PackageManager::Bun)),
            (r#"{ "packageManager": "deno@2.0.0" }"#, None),
            (
                r#"{ "devEngines": { "packageManager": { "name": "npm" } } }"#,
                Some(PackageManager::Npm),
            ),
            (
                r#"{ "devEngines": { "packageManager": [{ "name": "deno" }, { "name": "pnpm" }] } }"#,
                Some(PackageManager::Pnpm),
            ),
            (
                r#"{ "packageManager": "yarn@4.0.0", "devEngines": { "packageManager": { "name": "npm" } } }"#,
                Some(PackageManager::Yarn),
            ),
            (r#"{ "name": "x" }"#, None),
            ("not json", None),
        ] {
            assert_eq!(PackageManager::from_package_json(json), expected, "{json}");
        }
    }

    #[test]
    fn test_program_name() {
        for (package_manager, program) in [
            (PackageManager::Npm, "npm.cmd"),
            (PackageManager::Yarn, "yarn.cmd"),
            (PackageManager::Pnpm, "pnpm.cmd"),
            (PackageManager::Bun, "bun"),
        ] {
            assert_eq!(package_manager.program(true), program);
        }

        for package_manager in [
            PackageManager::Npm,
            PackageManager::Yarn,
            PackageManager::Pnpm,
            PackageManager::Bun,
        ] {
            assert_eq!(package_manager.program(false), package_manager.to_string());
        }
    }

    #[test]
    fn test_install_run() {
        let root = Path::new("fixtures/install");
        let _ = collect_packages(root);

        super::install(root, false).unwrap();

        // The previously empty package-lock.json now contains the "install" name, so the install command ran in `root`.
        let file = fs::File::open(root.join("package-lock.json"));
        let json: Result<Value, serde_json::Error> = serde_json::from_reader(file.unwrap());
        assert_eq!(json.unwrap()["name"], "install");
    }
}
