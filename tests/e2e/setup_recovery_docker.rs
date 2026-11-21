//! Local Docker SetupV2 recovery recipe. Compile with `rustc --edition=2024`.
//! The dedicated Docker volume and its private proof files are retained.

use std::{
    env, fs,
    io::{Read, Write},
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::{Command, ExitCode, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "setup-recovery-docker --docker /absolute/docker --endpoint unix:///absolute/docker.sock --image sha256:<64 lowercase hex> --manifest /absolute/public-setup-v2.json";
const LABEL: &str = "qa.ai-guard.local/setup-recovery";
const DATA: &str = "/data/installation";
const CLI: &str = "/usr/local/bin/agentsandbox";
const LIMIT: usize = 262_144;
const CHECK_FRESH_DATA: &str = r#"
test "$(id -u):$(id -g)" = 1000:1000
test "$(stat -c '%u:%g:%a' /data)" = 1000:1000:700
entries=$(find /data -mindepth 1 -maxdepth 1 -print -quit)
test -z "$entries"
"#;

// The scripts use only fixed recipe paths inside the owned volume. File
// fingerprints and content comparisons stay inside that private volume.
const CHECK_FILES: &str = r#"
umask 077
export LC_ALL=C
root=/data/installation
proof=/data/.qa-setup-recovery
test "$(stat -c '%u:%g:%a' /data)" = 1000:1000:700
test "$(stat -c '%u:%g:%a' "$root")" = 1000:1000:700
test "$(sed -n '2p' "$root/preparation.json")" = '  "version": 2,'

private_file() {
    test -f "$1" && test ! -L "$1" && test -s "$1" &&
        test "$(stat -c '%u:%g:%a:%h' "$1")" = 1000:1000:600:1
}

fingerprint() {
    set -- "$root/preparation.json" "$root/authentication-creation.json" \
        "$root/authentication-origin.json" "$root/oidc.json" "$root/tenants.json" \
        "$root/secret-material.json" "$root"/tenants/*/runner
    for file do
        private_file "$file" || return 1
        stat -c '%i:%u:%g:%a:%s' "$file" || return 1
        sha256sum "$file" || return 1
    done
    for runner in "$root"/tenants/*/runner; do
        viewer=${runner%/runner}/viewer
        private_file "$viewer" || return 1
        stat -c '%i:%u:%g:%a:%s' "$viewer" || return 1
        sha256sum "$viewer" || return 1
    done
}

records=$(fingerprint)
fingerprint=$(printf '%s\n' "$records" | sha256sum)
case "$1" in
    isolate-completion)
        private_file "$root/completion.json"
        mkdir -m 700 "$proof"
        printf '%s\n' "$fingerprint" > "$proof/private-fingerprint"
        mv --no-clobber "$root/completion.json" "$proof/original-completion.json"
        test ! -e "$root/completion.json"
        private_file "$proof/original-completion.json"
        sync "$proof/private-fingerprint" "$proof" "$root" /data
        ;;
    compare)
        test "$(stat -c '%u:%g:%a' "$proof")" = 1000:1000:700
        private_file "$root/completion.json"
        private_file "$proof/original-completion.json"
        private_file "$proof/private-fingerprint"
        cmp -s "$root/completion.json" "$proof/original-completion.json"
        test "$fingerprint" = "$(cat "$proof/private-fingerprint")"
        ;;
    *) exit 2 ;;
esac
"#;

fn require(condition: bool, failure: impl Into<Box<dyn std::error::Error>>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(failure.into())
    }
}

struct Recipe {
    docker: PathBuf,
    endpoint: String,
    image: String,
    manifest: Vec<u8>,
    volume: String,
    nonce: String,
}

struct Output {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Recipe {
    fn read(args: &[String]) -> Result<Self> {
        let [
            docker_flag,
            docker_arg,
            endpoint_flag,
            endpoint,
            image_flag,
            image,
            manifest_flag,
            manifest,
        ] = args
        else {
            return Err(USAGE.into());
        };
        require(
            [
                docker_flag.as_str(),
                endpoint_flag,
                image_flag,
                manifest_flag,
            ] == ["--docker", "--endpoint", "--image", "--manifest"],
            USAGE,
        )?;
        let docker = Path::new(docker_arg);
        require(docker.is_absolute(), "Docker executable must be absolute")?;
        let docker = docker.canonicalize()?;
        let meta = docker.metadata()?;
        require(
            meta.is_file() && meta.permissions().mode() & 0o111 != 0,
            "Docker executable must be a regular executable file",
        )?;
        let socket = endpoint
            .strip_prefix("unix://")
            .ok_or("use an explicit Unix socket")?;
        let path = Path::new(socket);
        require(
            path.is_absolute()
                && path
                    .components()
                    .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
                && !socket.chars().any(|c| c.is_control() || c.is_whitespace()),
            "use an absolute Unix socket path without traversal or whitespace",
        )?;
        require(
            path.metadata()?.file_type().is_socket(),
            "the selected endpoint is not a Unix socket",
        )?;
        require(
            image.strip_prefix("sha256:").is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }),
            "select a cached image by its full lowercase SHA-256 ID",
        )?;
        let manifest = Path::new(manifest);
        require(
            manifest.is_absolute() && manifest.symlink_metadata()?.is_file(),
            "select an absolute, regular public manifest",
        )?;
        let mut input = Vec::new();
        fs::File::open(manifest)?
            .take((LIMIT + 1) as u64)
            .read_to_end(&mut input)?;
        require(
            (1..=LIMIT).contains(&input.len()),
            "public manifest exceeds its stdin bound or is empty",
        )?;
        let mut random = [0u8; 16];
        fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        let nonce = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(Self {
            docker,
            endpoint: endpoint.to_owned(),
            image: image.to_owned(),
            manifest: input,
            volume: format!("aig-qa-setup-{nonce}"),
            nonce,
        })
    }

    fn call(&self, stage: &str, args: &[&str], input: Option<&[u8]>) -> Result<Output> {
        let mut child = Command::new(&self.docker)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("DOCKER_CONFIG", format!("/nonexistent/{}", self.volume))
            .args(["--host", &self.endpoint])
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("missing Docker stdout pipe")?;
        let stderr = child.stderr.take().ok_or("missing Docker stderr pipe")?;
        thread::scope(|threads| {
            let writer = threads.spawn(move || {
                if let (Some(mut stdin), Some(input)) = (stdin, input) {
                    stdin.write_all(input)?;
                }
                Ok::<_, std::io::Error>(())
            });
            let stdout = threads.spawn(move || read_bounded(stdout));
            let stderr = threads.spawn(move || read_bounded(stderr));
            let deadline = Instant::now() + Duration::from_secs(50);
            let status = loop {
                if let Some(status) = child.try_wait()? {
                    break status;
                }
                if Instant::now() >= deadline {
                    child.kill()?;
                    let _ = child.wait()?;
                    return Err(format!("{stage}: Docker client timed out; inspect only the retained recipe resources").into());
                }
                thread::sleep(Duration::from_millis(25));
            };
            writer.join().map_err(|_| "Docker stdin task failed")??;
            Ok(Output {
                status,
                stdout: stdout.join().map_err(|_| "Docker stdout task failed")??,
                stderr: stderr.join().map_err(|_| "Docker stderr task failed")??,
            })
        })
    }

    fn checked(&self, stage: &str, args: &[&str], input: Option<&[u8]>) -> Result<Output> {
        let output = self.call(stage, args, input)?;
        require(
            output.status.success(),
            format!("{stage}: Docker command {}", output.status),
        )?;
        Ok(output)
    }

    fn owns_idle_volume(&self) -> Result<()> {
        let option = format!(
            r#"{{{{.Name}}}} {{{{.Driver}}}} {{{{.Scope}}}} {{{{with index . "Options"}}}}{{{{len .}}}}{{{{else}}}}0{{{{end}}}} {{{{index .Labels "{LABEL}"}}}}"#
        );
        let actual = self.checked(
            "volume identity",
            &["volume", "inspect", "--format", &option, &self.volume],
            None,
        )?;
        let expected = format!("{} local local 0 {}\n", self.volume, self.nonce);
        require(
            actual.stdout == expected.as_bytes(),
            "volume identity or local driver differs",
        )?;
        let consumer = format!("volume={}", self.volume);
        let active = self.checked(
            "volume consumers",
            &["ps", "--all", "--quiet", "--filter", &consumer],
            None,
        )?;
        require(
            active.stdout.is_empty(),
            "recipe volume has an existing container consumer",
        )
    }

    fn container(&self, phase: &str, command: &[&str], input: Option<&[u8]>) -> Result<Output> {
        self.owns_idle_volume()?;
        let mount = format!("type=volume,src={},dst=/data", self.volume);
        let name = format!("{}-{phase}", self.volume);
        let label = format!("{LABEL}={}", self.nonce);
        let mut args = vec![
            "run",
            "--rm",
            "--pull=never",
            "--name",
            &name,
            "--label",
            &label,
            "--network=none",
            "--ipc=none",
            "--user=1000:1000",
            "--read-only",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--memory=256m",
            "--memory-swap=256m",
            "--cpus=1",
            "--pids-limit=64",
            "--no-healthcheck",
            "--ulimit",
            "fsize=67108864:67108864",
            "--ulimit",
            "core=0:0",
            "--log-driver=none",
            "--mount",
            &mount,
            "--entrypoint",
            "/usr/bin/timeout",
        ];
        if input.is_some() {
            args.push("--interactive");
        }
        args.extend([
            self.image.as_str(),
            "--signal=TERM",
            "--kill-after=3s",
            "35s",
        ]);
        args.extend_from_slice(command);
        self.call(phase, &args, input)
    }

    fn check_volume(&self, phase: &str, action: &str) -> Result<()> {
        let output = self.container(
            phase,
            &["/bin/sh", "-ceu", CHECK_FILES, "setup-recovery", action],
            None,
        )?;
        require(
            output.status.success() && output.stdout.is_empty(),
            format!("{phase}: private recovery comparison failed"),
        )
    }

    fn run(&self) -> Result<()> {
        let image = self.checked(
            "image identity",
            &[
                "image",
                "inspect",
                "--format",
                r#"{{.Id}} {{.Os}} {{with index .Config "Volumes"}}{{len .}}{{else}}0{{end}}"#,
                &self.image,
            ],
            None,
        )?;
        require(
            image.stdout == format!("{} linux 0\n", self.image).as_bytes(),
            "image must be the selected Linux image with no implicit volumes",
        )?;
        let name_filter = format!("name=^{}$", self.volume);
        let existing = self.checked(
            "volume absence",
            &["volume", "ls", "--quiet", "--filter", &name_filter],
            None,
        )?;
        require(
            existing.stdout.is_empty(),
            "the selected random volume already exists",
        )?;
        let label = format!("{LABEL}={}", self.nonce);
        println!(
            "retained_volume={} image={} scope=setup_v2_recovery",
            self.volume, self.image
        );
        let created = self.checked(
            "create dedicated volume",
            &[
                "volume",
                "create",
                "--driver",
                "local",
                "--label",
                &label,
                &self.volume,
            ],
            None,
        )?;
        require(
            created.stdout == format!("{}\n", self.volume).as_bytes(),
            "Docker returned a different volume",
        )?;
        let empty = self.container(
            "volume-preflight",
            &["/bin/sh", "-ceu", CHECK_FRESH_DATA],
            None,
        )?;
        require(
            empty.status.success() && empty.stdout.is_empty(),
            "recipe volume was not empty and private for UID 1000",
        )?;

        let prepare = [
            CLI,
            "setup",
            "prepare",
            "--input",
            "-",
            "--output-dir",
            DATA,
        ];
        let initial = self.container("prepare", &prepare, Some(&self.manifest))?;
        let report = std::str::from_utf8(&initial.stdout)?;
        require(
            initial.status.success()
                && report.contains("\"status\": \"prepared\"")
                && report.contains("\"installation_dir\": \"/data/installation\""),
            "prepare did not report the expected installation",
        )?;
        self.check_volume("isolate-completion", "isolate-completion")?;
        for (resume, check) in [("resume-1", "compare-1"), ("resume-2", "compare-2")] {
            let output = self.container(
                resume,
                &[
                    CLI,
                    "setup",
                    "resume",
                    "--recovery-dir",
                    DATA,
                    "--confirm-control-stopped",
                ],
                None,
            )?;
            require(
                output.status.success() && output.stdout == initial.stdout,
                format!("{resume}: recovery report differed"),
            )?;
            self.check_volume(check, "compare")?;
        }
        let repeated = self.container("repeat-prepare", &prepare, Some(&self.manifest))?;
        require(
            repeated.status.code() == Some(1)
                && repeated.stdout.is_empty()
                && std::str::from_utf8(&repeated.stderr)?
                    .contains("exclusive final destination must be absent"),
            "repeated prepare did not refuse the existing destination",
        )?;
        self.check_volume("compare-final", "compare")?;
        self.owns_idle_volume()?;
        println!(
            "status=PASS prepare_v2=1 resumes=2 duplicate_prepare=refused private_recovery_state=identical retained_volume={}",
            self.volume
        );
        Ok(())
    }
}

fn read_bounded(reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take((LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > LIMIT {
        return Err(std::io::Error::other("Docker output exceeded its bound"));
    }
    Ok(bytes)
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.as_slice() == ["--help"] {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match Recipe::read(&args).and_then(|recipe| recipe.run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("setup recovery recipe failed: {error}");
            ExitCode::FAILURE
        }
    }
}
