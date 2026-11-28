//! Standard-library-only engine fixture; never runs a business command or engine.
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
const IMAGE: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
fn q(value: &str) -> String {
    format!("{value:?}")
}
fn field(args: &[String], flag: &str) -> String {
    args[args.iter().position(|a| a == flag).unwrap() + 1].clone()
}
fn log(root: &Path, line: &str) {
    // One append write per record: start and inspect are separate processes.
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("calls"))
        .unwrap()
        .write_all(format!("{line}\n").as_bytes())
        .unwrap();
}
fn set_state(instance: &Path, value: &str) {
    let temporary = instance.join(format!("state-{}", std::process::id()));
    fs::write(&temporary, value).unwrap();
    fs::rename(temporary, instance.join("state")).unwrap();
}
fn labels(args: &[String]) -> String {
    let values = args
        .windows(2)
        .filter(|p| p[0] == "--label")
        .map(|p| {
            let (k, v) = p[1].split_once('=').unwrap();
            format!("{}:{}", q(k), q(v))
        })
        .collect::<Vec<_>>();
    format!("{{{}}}", values.join(","))
}
fn container(root: &Path, target: &str) -> PathBuf {
    let direct = root.join("containers").join(target);
    if direct.is_dir() {
        return direct;
    }
    for entry in fs::read_dir(root.join("containers"))
        .unwrap()
        .filter_map(Result::ok)
    {
        if fs::read_to_string(entry.path().join("name")).unwrap() == target {
            return entry.path();
        }
    }
    eprintln!("fixture instance not found");
    std::process::exit(125)
}
fn main() {
    let original = std::env::args().skip(1).collect::<Vec<_>>();
    let start = original
        .iter()
        .position(|a| {
            matches!(
                a.as_str(),
                "info" | "image" | "volume" | "create" | "start" | "inspect" | "kill" | "rm" | "ps"
            )
        })
        .unwrap();
    let args = &original[start..];
    let metadata = std::env::current_exe().unwrap().with_extension("fixture");
    let root = PathBuf::from(fs::read_to_string(metadata.join("root")).unwrap());
    let case = fs::read_to_string(metadata.join("case")).unwrap();
    fs::create_dir_all(root.join("containers")).unwrap();
    fs::create_dir_all(root.join("volumes")).unwrap();
    let target = args.last().unwrap();
    match args[0].as_str() {
        "info" => {
            log(&root, "info");
            println!(
                r#"{{"ID":"fixture-engine","SecurityOptions":["name=rootless"],"ServerVersion":"fixture"}}"#
            );
        }
        "image" => {
            log(&root, "image");
            println!("{IMAGE}");
        }
        "volume" => {
            log(&root, &format!("volume {} {target}", args[1]));
            let volume = root.join("volumes").join(target);
            if args[1] == "create" {
                fs::create_dir_all(&volume).unwrap();
                fs::write(volume.join("labels"), labels(args)).unwrap();
                if case == "volume-response-lost" && !root.join("volume_response_lost").exists() {
                    fs::write(root.join("volume_response_lost"), "once").unwrap();
                    eprintln!("fixture volume acknowledgement lost after creation");
                    std::process::exit(125);
                }
                println!("{target}");
            } else {
                if !volume.is_dir() {
                    std::process::exit(125);
                }
                let labels = fs::read_to_string(volume.join("labels")).unwrap();
                println!(
                    r#"{{"Name":{},"Driver":"local","Scope":"local","Options":{{}},"Labels":{labels}}}"#,
                    q(target)
                );
            }
        }
        "create" => {
            let name = field(args, "--name");
            let purpose = args
                .iter()
                .find_map(|a| a.strip_prefix("agentsandbox.purpose="))
                .unwrap_or_else(|| {
                    if name.contains("-import-") {
                        "import"
                    } else if name.contains("-export-") {
                        "export"
                    } else {
                        "run"
                    }
                });
            if purpose == "run"
                && matches!(
                    case.as_str(),
                    "runtime-error-secret" | "runtime-error-capture-secret"
                )
            {
                log(&root, "create_rejected run");
                let secret = fs::read_to_string(root.join("config/secrets/runner")).unwrap();
                if case == "runtime-error-capture-secret" {
                    std::io::stderr()
                        .write_all(
                            format!("useful control report\n{}", secret.repeat(131)).as_bytes(),
                        )
                        .unwrap();
                } else {
                    eprintln!("fixture runtime error: {}", secret.trim());
                }
                std::process::exit(125);
            }
            assert!(args.iter().any(|a| a == "--read-only"));
            for (flag, value) in [
                ("--cpus", "2"),
                ("--memory", "1g"),
                ("--pids-limit", "256"),
                ("--cap-drop", "ALL"),
                ("--security-opt", "no-new-privileges"),
                ("--user", "1000:1000"),
                ("--workdir", "/workspace"),
                ("--log-driver", "none"),
            ] {
                assert_eq!(field(args, flag), value);
            }
            let network = field(args, "--network");
            assert_eq!(
                args.iter().any(|arg| arg == "--interactive"),
                purpose == "import"
            );
            assert!(!args.iter().any(|arg| arg == "--tty" || arg == "-t"));
            assert!(matches!(network.as_str(), "none" | "bridge"));
            if purpose != "run" {
                assert_eq!(network, "none");
            }
            let sequence = fs::read_to_string(root.join("sequence"))
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
                + 1;
            fs::write(root.join("sequence"), sequence.to_string()).unwrap();
            let id = format!("{sequence:064x}");
            let instance = root.join("containers").join(&id);
            fs::create_dir(&instance).unwrap();
            let mount = field(args, "--mount");
            let volume = mount
                .split("source=")
                .nth(1)
                .unwrap()
                .split(',')
                .next()
                .unwrap();
            let separator = args.iter().position(|a| a == "--").unwrap();
            if purpose == "import_verification" {
                assert_eq!(
                    mount,
                    format!("type=volume,source={volume},target=/workspace,readonly")
                );
            }
            let cmd = args[separator + 2..]
                .iter()
                .map(|a| q(a))
                .collect::<Vec<_>>()
                .join(",");
            for (file, value) in [
                ("name", name.as_str()),
                ("volume", volume),
                ("purpose", purpose),
                ("state", "created"),
                ("cmd", cmd.as_str()),
                ("network", network.as_str()),
            ] {
                fs::write(instance.join(file), value).unwrap();
            }
            fs::write(instance.join("entry"), field(args, "--entrypoint")).unwrap();
            fs::write(instance.join("labels"), labels(args)).unwrap();
            if purpose == "run" {
                fs::write(root.join("run_id"), &id).unwrap();
                fs::write(root.join("run_name"), &name).unwrap();
            }
            log(&root, &format!("create {id} {purpose}"));
            if purpose == "run"
                && case == "create-response-lost"
                && !root.join("create_response_lost").exists()
            {
                fs::write(root.join("create_response_lost"), "once").unwrap();
                eprintln!("fixture create acknowledgement lost after creation");
                std::process::exit(125);
            }
            println!("{id}");
        }
        "start" => {
            let instance = container(&root, target);
            let purpose = fs::read_to_string(instance.join("purpose")).unwrap();
            log(
                &root,
                &format!(
                    "start {} {purpose}",
                    instance.file_name().unwrap().to_string_lossy()
                ),
            );
            if purpose == "run" && case == "broker-crash-running" {
                set_state(&instance, "running");
                while fs::read_to_string(instance.join("state")).unwrap() == "running" {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            } else {
                set_state(&instance, "exited");
            }
            if purpose == "import" {
                let mut archive = Vec::new();
                std::io::stdin().read_to_end(&mut archive).unwrap();
                fs::write(root.join("workspace_archive"), archive).unwrap();
            } else if purpose == "export" {
                let archive =
                    fs::read(root.join("workspace_archive")).unwrap_or_else(|_| vec![0; 1024]);
                std::io::stdout().write_all(&archive).unwrap();
            } else if case == "adapter-boundary-secret" {
                let secret = fs::read_to_string(root.join("config/secrets/runner")).unwrap();
                println!(
                    "{{\"type\":\"agentsandbox.telemetry\",\"version\":1,\"kind\":\"tool.result\",\"data\":{{\"message\":\"{}{}\"}}}}",
                    "x".repeat(4073),
                    secret.trim()
                );
            } else if case == "output-boundary-secret" {
                std::io::stdout().write_all(&vec![b'x'; 8180]).unwrap();
                std::io::stdout()
                    .write_all(
                        fs::read_to_string(root.join("config/secrets/runner"))
                            .unwrap()
                            .trim()
                            .as_bytes(),
                    )
                    .unwrap();
            } else if case == "capture-boundary-secret" {
                let secret = fs::read_to_string(root.join("config/secrets/runner")).unwrap();
                let output = format!("useful worker report\n{}", secret.repeat(522));
                std::io::stdout().write_all(output.as_bytes()).unwrap();
                std::io::stderr().write_all(output.as_bytes()).unwrap();
            }
        }
        "inspect" => {
            let instance = container(&root, target);
            let id = instance.file_name().unwrap().to_string_lossy();
            let name = fs::read_to_string(instance.join("name")).unwrap();
            let state = fs::read_to_string(instance.join("state")).unwrap();
            let volume = fs::read_to_string(instance.join("volume")).unwrap();
            let labels = fs::read_to_string(instance.join("labels")).unwrap();
            let entry = fs::read_to_string(instance.join("entry")).unwrap();
            let cmd = fs::read_to_string(instance.join("cmd")).unwrap();
            let network = fs::read_to_string(instance.join("network")).unwrap();
            let open_stdin = fs::read_to_string(instance.join("purpose")).unwrap() == "import";
            let workspace_rw =
                fs::read_to_string(instance.join("purpose")).unwrap() != "import_verification";
            if case == "named-verification-created-ack" && !workspace_rw {
                fs::write(root.join("verification-inspection-entered"), b"").unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while !root.join("verification-inspection-release").is_file() {
                    if std::time::Instant::now() >= deadline {
                        std::process::exit(125);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            let networks = if network == "bridge" {
                r#"{"bridge":{}}"#
            } else {
                "{}"
            };
            log(&root, &format!("inspect {id}"));
            let started = if state == "created" {
                "0001-01-01T00:00:00Z"
            } else {
                "2026-10-29T10:00:00Z"
            };
            let finished = if state == "exited" {
                "2026-10-29T10:00:01Z"
            } else {
                "0001-01-01T00:00:00Z"
            };
            let exit_code = if state == "exited"
                && fs::read_to_string(instance.join("purpose")).unwrap() == "run"
            {
                fs::read_to_string(root.join("run_exit_code"))
                    .ok()
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(0)
            } else {
                0
            };
            let process = format!(
                r#"{{"Status":"{state}","Running":{},"StartedAt":"{started}","FinishedAt":"{finished}","ExitCode":{exit_code},"OOMKilled":false}}"#,
                state == "running"
            );
            if args.iter().any(|a| a == "{{json .State}}") {
                println!("{process}");
                return;
            }
            println!(
                r#"{{"Id":"{id}","Name":{},"Image":"{IMAGE}","RestartCount":0,"State":{process},"HostConfig":{{"LogConfig":{{"Type":"none","Config":{{}}}},"NanoCpus":2000000000,"CpuQuota":0,"CpuPeriod":0,"Memory":1073741824,"PidsLimit":256,"ReadonlyRootfs":true,"Privileged":false,"AutoRemove":false,"Tmpfs":{{"/tmp":"rw,nosuid,nodev,noexec,size=256m,mode=1777","/home/agent":"rw,nosuid,nodev,size=64m,uid=1000,gid=1000,mode=700"}},"NetworkMode":"{network}","CapDrop":["ALL"],"SecurityOpt":["no-new-privileges"],"RestartPolicy":{{"Name":"no"}}}},"Config":{{"Tty":false,"OpenStdin":{open_stdin},"StdinOnce":{open_stdin},"User":"1000:1000","WorkingDir":"/workspace","Entrypoint":[{}],"Cmd":[{cmd}],"Labels":{labels},"Env":["PRIVATE=not-a-public-observation"]}},"Mounts":[{{"Type":"volume","Name":{},"Destination":"/workspace","RW":{workspace_rw}}}],"NetworkSettings":{{"Networks":{networks}}}}}"#,
                q(&format!("/{name}")),
                q(&entry),
                q(&volume)
            );
        }
        "rm" => {
            let instance = container(&root, target);
            let purpose = fs::read_to_string(instance.join("purpose")).unwrap();
            log(
                &root,
                &format!(
                    "rm {} {purpose}",
                    instance.file_name().unwrap().to_string_lossy()
                ),
            );
            if purpose == "run" {
                assert!(
                    root.join("terminal_ack").is_file(),
                    "cleanup happened before the real control terminal acknowledgement"
                );
                if case == "cleanup-response-lost" {
                    fs::remove_dir_all(&instance).unwrap();
                    eprintln!("fixture cleanup response lost after removal");
                    std::process::exit(125);
                }
                if case.starts_with("cleanup-") {
                    eprintln!("fixture cleanup refused");
                    std::process::exit(125);
                }
            }
            fs::remove_dir_all(instance).unwrap();
        }
        "kill" => {
            let instance = container(&root, target);
            log(
                &root,
                &format!("kill {}", instance.file_name().unwrap().to_string_lossy()),
            );
            set_state(&instance, "exited");
        }
        "ps" => {
            log(&root, "ps");
            let name = args
                .iter()
                .find_map(|a| a.strip_prefix("name=^"))
                .map(|s| s.trim_start_matches('/').trim_end_matches('$'));
            for entry in fs::read_dir(root.join("containers"))
                .unwrap()
                .filter_map(Result::ok)
            {
                let actual = fs::read_to_string(entry.path().join("name")).unwrap();
                if name.is_none_or(|expected| expected == actual) {
                    println!("{}", entry.file_name().to_string_lossy());
                }
            }
        }
        _ => std::process::exit(126),
    }
}
