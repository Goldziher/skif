//! What a measurement was made from and on: the source commit, the CPU and the load of the machine.

use std::fs;
use std::path::Path;
use std::process::Command;

/// The commit of the skif checkout the harness runs from (the tree the mise tasks build `target/release/skif` from),
/// with `-dirty` when tracked files differ from it, or `unknown` outside a git checkout.
pub fn source_rev() -> String {
    rev_of(Path::new(env!("CARGO_MANIFEST_DIR")))
}

fn rev_of(dir: &Path) -> String {
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let Some(rev) = git(&["rev-parse", "--short=12", "HEAD"]) else {
        return "unknown".to_owned();
    };
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|status| !status.is_empty());
    if dirty { format!("{rev}-dirty") } else { rev }
}

/// The CPU model: `model name` of `/proc/cpuinfo` on Linux, `machdep.cpu.brand_string` on macOS, else `unknown`.
pub fn cpu() -> String {
    fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|info| cpu_model(&info))
        .or_else(|| sysctl("machdep.cpu.brand_string"))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// The logical CPUs the process may use.
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// The 1, 5 and 15 minute load averages: `/proc/loadavg` on Linux, `vm.loadavg` on macOS.
pub fn load_average() -> Option<[f64; 3]> {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .or_else(|| sysctl("vm.loadavg"))
        .and_then(|text| parse_load(&text))
}

fn sysctl(name: &str) -> Option<String> {
    Command::new("sysctl")
        .args(["-n", name])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// The first `model name` of a `/proc/cpuinfo`.
fn cpu_model(cpuinfo: &str) -> Option<String> {
    cpuinfo
        .lines()
        .filter(|line| line.starts_with("model name"))
        .find_map(|line| line.split_once(':'))
        .map(|(_, model)| model.trim().to_owned())
        .filter(|model| !model.is_empty())
}

/// The first three numbers of `/proc/loadavg` (`0.52 0.58 0.59 1/389 12345`) or of macOS's `vm.loadavg`
/// (`{ 1.23 1.45 1.67 }`).
fn parse_load(text: &str) -> Option<[f64; 3]> {
    let mut numbers = text.split_whitespace().filter_map(|token| token.parse::<f64>().ok());
    Some([numbers.next()?, numbers.next()?, numbers.next()?])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_cpu_model_and_both_load_formats() {
        let cpuinfo = "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: Intel(R) Xeon(R) CPU @ 2.10GHz\n\nprocessor\t: 1\nmodel name\t: Intel(R) Xeon(R) CPU @ 2.10GHz\n";
        assert_eq!(cpu_model(cpuinfo).as_deref(), Some("Intel(R) Xeon(R) CPU @ 2.10GHz"));
        assert_eq!(cpu_model("processor\t: 0\n"), None);
        assert_eq!(parse_load("0.52 0.58 0.59 1/389 12345\n"), Some([0.52, 0.58, 0.59]));
        assert_eq!(parse_load("{ 1.23 1.45 1.67 }"), Some([1.23, 1.45, 1.67]));
        assert_eq!(parse_load("{ 1.23 }"), None);
    }

    #[test]
    fn the_rev_names_the_commit_and_a_dirty_tree() {
        let dir = std::env::temp_dir().join(format!("skif-bench-provenance-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.com",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap()
                .status;
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "--quiet"]);
        fs::write(dir.join("a.txt"), "1").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "--quiet", "-m", "one"]);
        let clean = rev_of(&dir);
        assert_eq!(clean.len(), 12, "{clean}");
        assert!(clean.bytes().all(|b| b.is_ascii_hexdigit()), "{clean}");
        fs::write(dir.join("untracked.txt"), "x").unwrap();
        assert_eq!(rev_of(&dir), clean, "untracked files do not make the tree dirty");
        fs::write(dir.join("a.txt"), "2").unwrap();
        assert_eq!(rev_of(&dir), format!("{clean}-dirty"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
