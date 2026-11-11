//! What a measurement was made from and on: the source commit, the machine and its load, and the toolchain.
//!
//! Every fact is read by a small pure function from the text the system gives (`/etc/os-release`, `/proc/meminfo`,
//! `cpu.max`, the environment), so it can be tested on fixture text; a fact that cannot be read is `None` and never
//! fails a run.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use crate::proc::{capture, find_on_path};

/// How long one `--version` probe may take: a corepack shim may download its package manager first.
const VERSION_TIMEOUT: Duration = Duration::from_secs(120);

/// The commit of the skif checkout the harness runs from (the tree the mise tasks build `target/release/skif` from),
/// with `-dirty` when tracked files differ from it, or `unknown` outside a git checkout.
pub fn source_rev() -> String {
    rev_of(Path::new(env!("CARGO_MANIFEST_DIR")))
}

/// The commit of the git checkout at `dir`, with `-dirty` when tracked files differ from it, or `unknown` when `dir`
/// is not a git checkout.
pub fn rev_of(dir: &Path) -> String {
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

/// The machine a measurement ran on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Machine {
    /// `PRETTY_NAME` of `/etc/os-release` on Linux, `sw_vers`' product name and version on macOS.
    pub os: Option<String>,
    /// `uname -sr`.
    pub kernel: Option<String>,
    pub arch: String,
    /// `model name` of `/proc/cpuinfo`, else `lscpu`'s `Model name` (Arm CPUs name no model in `/proc/cpuinfo`), else
    /// `machdep.cpu.brand_string` on macOS.
    pub cpu: Option<String>,
    /// The logical CPUs of the machine: the processors of `/proc/cpuinfo`, `hw.logicalcpu` on macOS.
    pub logical_cpus: Option<usize>,
    /// The logical CPUs this process may use (its affinity mask and cgroup quota), as the standard library reports.
    pub available_cpus: usize,
    /// Distinct (package, core) pairs of `/proc/cpuinfo`, else of `/sys/devices/system/cpu/*/topology`, else
    /// `hw.physicalcpu` on macOS.
    pub physical_cores: Option<usize>,
    /// `MemTotal` of `/proc/meminfo`, `hw.memsize` on macOS.
    pub memory_bytes: Option<u64>,
    /// The CPUs the process's cgroup may use (`cpu.max` quota over period); `None` without a limit.
    pub cgroup_cpu_limit: Option<f64>,
    /// The hypervisor: `lscpu`'s `Hypervisor vendor`, else `systemd-detect-virt`; `virtual machine` on macOS when
    /// `kern.hv_vmm_present` is set.
    pub virtualization: Option<String>,
    /// The GitHub Actions runner, when the run is a workflow job.
    pub github: Option<GithubRunner>,
}

/// What a GitHub Actions job knows about its runner, from the variables the runner sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubRunner {
    /// `ImageOS`, such as `ubuntu24`.
    pub image_os: Option<String>,
    /// `ImageVersion`, the runner image release.
    pub image_version: Option<String>,
    /// `RUNNER_OS`.
    pub runner_os: Option<String>,
    /// `RUNNER_ARCH`.
    pub runner_arch: Option<String>,
    /// `RUNNER_ENVIRONMENT`: `github-hosted` or `self-hosted`.
    pub runner_environment: Option<String>,
    /// `RUNNER_NAME`.
    pub runner_name: Option<String>,
    /// `GITHUB_SERVER_URL/GITHUB_REPOSITORY/actions/runs/GITHUB_RUN_ID`.
    pub run_url: Option<String>,
}

/// The versions of the toolchain around the measurement: the compiler that built skif and the JavaScript runtime and
/// package managers that install the comparison tools and the corpus. Each is the first line of `--version` of the
/// program on `PATH`; `rustc` is the one `RUSTUP_TOOLCHAIN` selects when it is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Toolchain {
    pub rustc: Option<String>,
    pub node: Option<String>,
    pub npm: Option<String>,
    pub pnpm: Option<String>,
    pub yarn: Option<String>,
    pub bun: Option<String>,
}

/// Reads the facts of the machine this process runs on.
pub fn machine() -> Machine {
    let cpuinfo = read("/proc/cpuinfo");
    let lscpu = stdout_of(Command::new("lscpu").env("LC_ALL", "C"));
    Machine {
        os: read("/etc/os-release")
            .and_then(|text| os_release_name(&text))
            .or_else(macos_name),
        kernel: stdout_of(Command::new("uname").arg("-sr")),
        arch: env::consts::ARCH.to_owned(),
        cpu: cpuinfo
            .as_deref()
            .and_then(cpu_model)
            .or_else(|| lscpu.as_deref().and_then(|text| lscpu_field(text, "Model name")))
            .or_else(|| sysctl("machdep.cpu.brand_string")),
        logical_cpus: cpuinfo
            .as_deref()
            .and_then(logical_cpus)
            .or_else(|| sysctl("hw.logicalcpu").and_then(|n| n.parse().ok())),
        available_cpus: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
        physical_cores: cpuinfo
            .as_deref()
            .and_then(cpuinfo_cores)
            .or_else(sysfs_cores)
            .or_else(|| sysctl("hw.physicalcpu").and_then(|n| n.parse().ok())),
        memory_bytes: read("/proc/meminfo")
            .and_then(|text| mem_total(&text))
            .or_else(|| sysctl("hw.memsize").and_then(|n| n.parse().ok())),
        cgroup_cpu_limit: cgroup_cpu_limit(),
        virtualization: lscpu
            .as_deref()
            .and_then(|text| lscpu_field(text, "Hypervisor vendor"))
            .or_else(|| stdout_of(&mut Command::new("systemd-detect-virt")).filter(|virt| virt != "none"))
            .or_else(|| (sysctl("kern.hv_vmm_present").as_deref() == Some("1")).then(|| "virtual machine".to_owned())),
        github: github_runner(|name| env::var(name).ok()),
    }
}

/// Reads the toolchain versions, running each probe in `scratch` (a directory without a `package.json`, so a corepack
/// shim reports its default version rather than a project's).
pub fn toolchain(scratch: &Path) -> Toolchain {
    let version = |program: &str| version_in(program, scratch, scratch);
    Toolchain {
        rustc: version("rustc"),
        node: version("node"),
        npm: version("npm"),
        pnpm: version("pnpm"),
        yarn: version("yarn"),
        bun: version("bun"),
    }
}

/// The first line of `program --version` run in `dir` (where a corepack shim resolves the project's `packageManager`),
/// with its output captured in `scratch`; `None` when the program is not on `PATH` or fails.
pub fn version_in(program: &str, dir: &Path, scratch: &Path) -> Option<String> {
    find_on_path(program)?;
    let run = capture(
        Command::new(program)
            .arg("--version")
            .current_dir(dir)
            .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
        scratch,
        &format!("version-{program}"),
        VERSION_TIMEOUT,
    )
    .ok()?;
    if run.timed_out || run.code != Some(0) {
        return None;
    }
    first_line(&String::from_utf8_lossy(&run.stdout))
}

/// The 1, 5 and 15 minute load averages: `/proc/loadavg` on Linux, `vm.loadavg` on macOS.
pub fn load_average() -> Option<[f64; 3]> {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .or_else(|| sysctl("vm.loadavg"))
        .and_then(|text| parse_load(&text))
}

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// The trimmed standard output of a command that succeeds and prints something.
fn stdout_of(command: &mut Command) -> Option<String> {
    command
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn sysctl(name: &str) -> Option<String> {
    stdout_of(Command::new("sysctl").args(["-n", name]))
}

/// `macOS 26.1` from `sw_vers`.
fn macos_name() -> Option<String> {
    let name = stdout_of(Command::new("sw_vers").arg("-productName"))?;
    let version = stdout_of(Command::new("sw_vers").arg("-productVersion"))?;
    Some(format!("{name} {version}"))
}

/// The CPU limit of the cgroup (v2) this process belongs to, else of the root cgroup, else of the cgroup v1 `cpu`
/// controller.
fn cgroup_cpu_limit() -> Option<f64> {
    let own = read("/proc/self/cgroup")
        .and_then(|text| cgroup_v2_path(&text))
        .map(|path| format!("/sys/fs/cgroup{}/cpu.max", path.trim_end_matches('/')));
    let v1 = || {
        let quota = read("/sys/fs/cgroup/cpu/cpu.cfs_quota_us")?;
        let period = read("/sys/fs/cgroup/cpu/cpu.cfs_period_us")?;
        cpu_max_limit(&format!("{} {}", quota.trim(), period.trim()))
    };
    own.iter()
        .map(String::as_str)
        .chain(["/sys/fs/cgroup/cpu.max"])
        .find_map(|path| read(path).and_then(|text| cpu_max_limit(&text)))
        .or_else(v1)
}

/// The physical cores of `/sys/devices/system/cpu/cpu*/topology`, for CPUs whose `/proc/cpuinfo` names no core ids.
fn sysfs_cores() -> Option<usize> {
    let cpus = fs::read_dir("/sys/devices/system/cpu").ok()?;
    let pairs: Vec<(String, String)> = cpus
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("cpu"))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .filter_map(|entry| {
            let topology = entry.path().join("topology");
            let field = |name: &str| {
                fs::read_to_string(topology.join(name))
                    .ok()
                    .map(|v| v.trim().to_owned())
            };
            Some((field("physical_package_id")?, field("core_id")?))
        })
        .collect();
    distinct_cores(pairs)
}

/// The value of `PRETTY_NAME` (else `NAME`) in an `os-release` file, without its quotes.
fn os_release_name(text: &str) -> Option<String> {
    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))
            .map(|value| value.trim().trim_matches(|c| c == '"' || c == '\'').to_owned())
            .filter(|value| !value.is_empty())
    };
    value("PRETTY_NAME").or_else(|| value("NAME"))
}

/// `MemTotal` of a `/proc/meminfo`, in bytes (the file counts kibibytes).
fn mem_total(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find_map(|line| line.strip_prefix("MemTotal:"))?;
    let mut words = line.split_whitespace();
    let kib: u64 = words.next()?.parse().ok()?;
    matches!(words.next(), Some("kB") | None).then_some(kib.saturating_mul(1024))
}

/// The CPUs a cgroup v2 `cpu.max` (`<quota> <period>`, or `max <period>` without a limit) allows; a cgroup v1 quota
/// of `-1` is no limit either.
fn cpu_max_limit(text: &str) -> Option<f64> {
    let mut words = text.split_whitespace();
    let quota: f64 = words.next()?.parse().ok()?;
    let period: f64 = words.next()?.parse().ok()?;
    (quota > 0.0 && period > 0.0).then_some(quota / period)
}

/// The cgroup v2 path of a `/proc/self/cgroup` (the `0::<path>` line).
fn cgroup_v2_path(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(|path| path.trim().to_owned())
        .filter(|path| path.starts_with('/'))
}

/// The value of a field of `lscpu`'s output (`Model name:   Neoverse-N2`).
fn lscpu_field(text: &str, field: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.trim_start().strip_prefix(field)?.trim_start().strip_prefix(':'))
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty() && value != "-")
}

/// The processors listed in a `/proc/cpuinfo`.
fn logical_cpus(cpuinfo: &str) -> Option<usize> {
    let count = cpuinfo
        .lines()
        .filter(|line| line.split(':').next().is_some_and(|key| key.trim() == "processor"))
        .count();
    (count > 0).then_some(count)
}

/// The distinct (`physical id`, `core id`) pairs of a `/proc/cpuinfo`; `None` when it lists no core ids, as on Arm.
fn cpuinfo_cores(cpuinfo: &str) -> Option<usize> {
    let mut pairs = Vec::new();
    for block in cpuinfo.split("\n\n") {
        let field = |key: &str| {
            block.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                (name.trim() == key).then(|| value.trim().to_owned())
            })
        };
        if let Some(core) = field("core id") {
            pairs.push((field("physical id").unwrap_or_default(), core));
        }
    }
    distinct_cores(pairs)
}

/// The number of distinct (package, core) pairs; `None` for none.
fn distinct_cores(pairs: Vec<(String, String)>) -> Option<usize> {
    let distinct: BTreeSet<(String, String)> = pairs.into_iter().collect();
    (!distinct.is_empty()).then_some(distinct.len())
}

/// The runner facts of a GitHub Actions job from its environment (`lookup` reads one variable); `None` outside one.
fn github_runner(lookup: impl Fn(&str) -> Option<String>) -> Option<GithubRunner> {
    let var = |name: &str| {
        lookup(name)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    if var("GITHUB_ACTIONS").as_deref() != Some("true") {
        return None;
    }
    let run_url = match (var("GITHUB_SERVER_URL"), var("GITHUB_REPOSITORY"), var("GITHUB_RUN_ID")) {
        (Some(server), Some(repository), Some(run)) => Some(format!(
            "{}/{repository}/actions/runs/{run}",
            server.trim_end_matches('/')
        )),
        _ => None,
    };
    Some(GithubRunner {
        image_os: var("ImageOS"),
        image_version: var("ImageVersion"),
        runner_os: var("RUNNER_OS"),
        runner_arch: var("RUNNER_ARCH"),
        runner_environment: var("RUNNER_ENVIRONMENT"),
        runner_name: var("RUNNER_NAME"),
        run_url,
    })
}

/// The first non-empty line of a `--version` output, trimmed.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
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
    use std::collections::BTreeMap;

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
    fn reads_the_operating_system_name_from_os_release() {
        let ubuntu = "PRETTY_NAME=\"Ubuntu 24.04.3 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\n";
        assert_eq!(os_release_name(ubuntu).as_deref(), Some("Ubuntu 24.04.3 LTS"));
        assert_eq!(
            os_release_name("NAME='Alpine Linux'\nID=alpine\n").as_deref(),
            Some("Alpine Linux")
        );
        assert_eq!(
            os_release_name("PRETTY_NAME=\"\"\nNAME=Arch\n").as_deref(),
            Some("Arch")
        );
        assert_eq!(os_release_name("ID=x\n"), None);
        assert_eq!(os_release_name("PRETTY_NAMES=x\n"), None);
    }

    #[test]
    fn reads_the_total_memory_from_meminfo() {
        let meminfo = "MemTotal:       16374836 kB\nMemFree:         1048576 kB\nMemAvailable:   12000000 kB\n";
        assert_eq!(mem_total(meminfo), Some(16_374_836 * 1024));
        assert_eq!(mem_total("MemFree: 1 kB\n"), None);
        assert_eq!(mem_total("MemTotal: lots kB\n"), None);
        assert_eq!(mem_total("MemTotal: 1 MB\n"), None);
    }

    #[test]
    fn reads_the_cgroup_cpu_limit() {
        assert_eq!(cpu_max_limit("200000 100000\n"), Some(2.0));
        assert_eq!(cpu_max_limit("150000 100000"), Some(1.5));
        assert_eq!(cpu_max_limit("max 100000\n"), None);
        assert_eq!(cpu_max_limit("-1 100000"), None, "a cgroup v1 quota of -1 is no limit");
        assert_eq!(cpu_max_limit("100000"), None);
        assert_eq!(cpu_max_limit(""), None);
        assert_eq!(
            cgroup_v2_path("0::/system.slice/runner.service\n").as_deref(),
            Some("/system.slice/runner.service")
        );
        assert_eq!(cgroup_v2_path("12:cpu,cpuacct:/docker/abc\n"), None, "cgroup v1 only");
    }

    #[test]
    fn reads_the_cpu_topology_of_x86_and_arm() {
        let x86 = "processor\t: 0\nmodel name\t: AMD EPYC 7763 64-Core Processor\nphysical id\t: 0\ncore id\t\t: 0\n\n\
                   processor\t: 1\nmodel name\t: AMD EPYC 7763 64-Core Processor\nphysical id\t: 0\ncore id\t\t: 0\n\n\
                   processor\t: 2\nmodel name\t: AMD EPYC 7763 64-Core Processor\nphysical id\t: 0\ncore id\t\t: 1\n\n\
                   processor\t: 3\nmodel name\t: AMD EPYC 7763 64-Core Processor\nphysical id\t: 0\ncore id\t\t: 1\n";
        assert_eq!(logical_cpus(x86), Some(4));
        assert_eq!(cpuinfo_cores(x86), Some(2), "two hyperthreads per core");
        assert_eq!(cpu_model(x86).as_deref(), Some("AMD EPYC 7763 64-Core Processor"));
        let arm = "processor\t: 0\nBogoMIPS\t: 2000.00\nCPU implementer\t: 0x41\nCPU part\t: 0xd49\n\n\
                   processor\t: 1\nBogoMIPS\t: 2000.00\nCPU implementer\t: 0x41\nCPU part\t: 0xd49\n";
        assert_eq!(logical_cpus(arm), Some(2));
        assert_eq!(cpuinfo_cores(arm), None, "Arm lists no core ids");
        assert_eq!(cpu_model(arm), None);
        assert_eq!(logical_cpus(""), None);
        assert_eq!(
            distinct_cores(vec![
                ("0".into(), "0".into()),
                ("0".into(), "1".into()),
                ("1".into(), "0".into()),
                ("0".into(), "0".into()),
            ]),
            Some(3)
        );
        assert_eq!(distinct_cores(Vec::new()), None);
    }

    #[test]
    fn reads_the_cpu_model_and_hypervisor_from_lscpu() {
        let lscpu = "Architecture:             aarch64\n  CPU op-mode(s):         64-bit\nVendor ID:                ARM\n  \
                     Model name:             Neoverse-N2\n    Thread(s) per core:   1\nHypervisor vendor:        Microsoft\n\
                     Virtualization type:      full\nNUMA node(s):             -\n";
        assert_eq!(lscpu_field(lscpu, "Model name").as_deref(), Some("Neoverse-N2"));
        assert_eq!(lscpu_field(lscpu, "Hypervisor vendor").as_deref(), Some("Microsoft"));
        assert_eq!(lscpu_field(lscpu, "NUMA node(s)"), None, "a dash is no value");
        assert_eq!(lscpu_field(lscpu, "BIOS Model name"), None);
    }

    #[test]
    fn reads_the_github_runner_from_the_environment() {
        let env: BTreeMap<&str, &str> = BTreeMap::from([
            ("GITHUB_ACTIONS", "true"),
            ("ImageOS", "ubuntu24"),
            ("ImageVersion", "20261102.1.0"),
            ("RUNNER_OS", "Linux"),
            ("RUNNER_ARCH", "X64"),
            ("RUNNER_ENVIRONMENT", "github-hosted"),
            ("RUNNER_NAME", "GitHub Actions 1000012345"),
            ("GITHUB_SERVER_URL", "https://github.com/"),
            ("GITHUB_REPOSITORY", "Goldziher/skif"),
            ("GITHUB_RUN_ID", "19283746"),
        ]);
        let lookup = |env: &BTreeMap<&str, &str>, name: &str| env.get(name).map(|value| (*value).to_owned());
        assert_eq!(
            github_runner(|name| lookup(&env, name)),
            Some(GithubRunner {
                image_os: Some("ubuntu24".into()),
                image_version: Some("20261102.1.0".into()),
                runner_os: Some("Linux".into()),
                runner_arch: Some("X64".into()),
                runner_environment: Some("github-hosted".into()),
                runner_name: Some("GitHub Actions 1000012345".into()),
                run_url: Some("https://github.com/Goldziher/skif/actions/runs/19283746".into()),
            })
        );
        let mut partial = env.clone();
        partial.remove("GITHUB_RUN_ID");
        partial.insert("ImageOS", " ");
        let runner = github_runner(|name| lookup(&partial, name)).unwrap();
        assert_eq!(runner.run_url, None);
        assert_eq!(runner.image_os, None, "a blank variable is no value");
        let mut local = env;
        local.remove("GITHUB_ACTIONS");
        assert_eq!(github_runner(|name| lookup(&local, name)), None);
    }

    #[test]
    fn a_version_is_the_first_line_of_its_output() {
        assert_eq!(
            first_line("\nrustc 1.99.0 (0123abcd 2026-10-01)\nextra\n").as_deref(),
            Some("rustc 1.99.0 (0123abcd 2026-10-01)")
        );
        assert_eq!(first_line("  \n"), None);
    }

    #[test]
    fn the_machine_record_never_fails_and_names_the_architecture() {
        let machine = machine();
        assert_eq!(machine.arch, env::consts::ARCH);
        assert!(machine.available_cpus >= 1);
        assert!(machine.logical_cpus.is_none_or(|logical| logical >= 1), "{machine:?}");
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
