//! CLI fixture only: the actual runtime remains unqualified by this process.
use std::{fs, io::Write};
const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn fixture(key: &str) -> String {
    fs::read_to_string(
        std::env::current_exe()
            .unwrap()
            .with_extension("fixture")
            .join(key),
    )
    .unwrap()
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let start = args
        .iter()
        .position(|a| {
            matches!(
                a.as_str(),
                "info" | "volume" | "create" | "start" | "inspect" | "kill" | "rm" | "image" | "ps"
            )
        })
        .unwrap();
    let args = &args[start..];
    let root = std::path::PathBuf::from(fixture("AI_GUARD_MANAGED_ROOT"));
    let case = fixture("AI_GUARD_MANAGED_CHILD");
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("log"))
        .unwrap();
    if args[0] == "volume" {
        writeln!(log, "volume {} {}", args[1], args.last().unwrap()).unwrap();
    } else {
        writeln!(log, "{} {}", args[0], args.last().unwrap()).unwrap();
    }
    match args[0].as_str() {
        "volume" => {
            if !args.last().unwrap().starts_with("aig-authority-") {
                if args[1] == "create" {
                    println!("{}", args.last().unwrap());
                    return;
                }
                let owner = if case == "volume_collision" {
                    "other"
                } else {
                    "expected"
                };
                let options = if case == "volume_bind" {
                    r#"{"type":"none","device":"/outside","o":"bind"}"#
                } else {
                    "{}"
                };
                println!(
                    r#"{{"Name":"aig-fixture-worker-work","Driver":"local","Scope":"local","Labels":{{"agentsandbox.owner":"{owner}"}},"Options":{options}}}"#
                );
                return;
            }
            if args[1] == "create" {
                let authority = args
                    .iter()
                    .find_map(|a| a.strip_prefix("agentsandbox.authority="))
                    .unwrap();
                let nonce = args
                    .iter()
                    .find_map(|a| a.strip_prefix("agentsandbox.binding="))
                    .unwrap();
                fs::write(root.join("marker_authority"), authority).unwrap();
                fs::write(root.join("marker_nonce"), nonce).unwrap();
                fs::write(root.join("marker_name"), args.last().unwrap()).unwrap();
                if root.join("marker_create_error").exists() {
                    eprintln!("private marker {} / {}", args.last().unwrap(), nonce);
                    std::process::exit(125);
                }
                println!("{}", args.last().unwrap());
            } else {
                if root.join("marker_missing").exists() {
                    std::process::exit(125);
                }
                let name = fs::read_to_string(root.join("marker_name")).unwrap();
                let nonce = fs::read_to_string(root.join("marker_nonce")).unwrap();
                let authority = fs::read_to_string(root.join("marker_authority")).unwrap();
                println!(
                    r#"{{"Name":"{name}","Labels":{{"agentsandbox.authority":"{authority}","agentsandbox.binding":"{nonce}"}}}}"#
                );
            }
        }
        "info" => {
            if root.join("pause_info_for_create").is_file() {
                fs::write(root.join("info_waiting_for_create"), "").unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !root.join("release_info_for_create").is_file() {
                    if std::time::Instant::now() >= deadline {
                        std::process::exit(125);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            if case == "podman_authority" {
                let rootless = !root.join("podman_rootful").exists();
                println!(
                    r#"{{"host":{{"security":{{"rootless":{rootless}}}}},"store":{{"graphRoot":"/fixture/graph","runRoot":"/fixture/run"}}}}"#
                );
                return;
            }

            let changed = case == "engine_changed" && root.join("state").exists();
            println!(
                "{{\"ID\":\"{}\"}}",
                if changed {
                    "other-engine"
                } else {
                    "fixture-engine"
                }
            );
        }
        "ps" => {
            if root.join("inventory_unavailable").exists() {
                std::process::exit(125);
            }
            if root.join("state").exists() {
                println!("{ID}");
            }
        }
        "image" => {
            println!("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        }
        "create" => {
            if case == "create_failed" {
                std::process::exit(125);
            }
            let name = &args[args.iter().position(|a| a == "--name").unwrap() + 1];
            assert_eq!(
                args[args.iter().position(|a| a == "--log-driver").unwrap() + 1],
                "none"
            );
            for pair in args.windows(2) {
                if pair[0] == "--label" {
                    let (key, value) = pair[1].split_once('=').unwrap();
                    fs::write(root.join(format!("label_{}", key.replace('.', "_"))), value)
                        .unwrap();
                }
            }
            let mount = &args[args.iter().position(|a| a == "--mount").unwrap() + 1];
            let volume = mount
                .split("source=")
                .nth(1)
                .unwrap()
                .split(',')
                .next()
                .unwrap();
            fs::write(root.join("volume"), volume).unwrap();
            fs::write(root.join("name"), name).unwrap();
            let interactive = args.iter().any(|arg| arg == "--interactive");
            fs::write(root.join("interactive"), interactive.to_string()).unwrap();
            fs::write(root.join("state"), "created").unwrap();
            if case == "create_bad" {
                println!("invalid reference!")
            } else {
                println!("{ID}")
            }
        }
        "start" => {
            assert_eq!(args.last().unwrap(), ID);
            fs::write(
                root.join("state"),
                if case == "timeout" || case == "start_callback_error" {
                    "running"
                } else {
                    "exited"
                },
            )
            .unwrap();
            if case == "timeout" || case == "start_callback_error" {
                while fs::read_to_string(root.join("state")).unwrap() == "running" {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
        "inspect" => {
            assert_eq!(args.last().unwrap(), ID);
            let state = fs::read_to_string(root.join("state")).unwrap();
            let running = state == "running";
            let started = if state == "created" {
                "0001-01-01T00:00:00Z"
            } else {
                "2026-10-29T12:00:00Z"
            };
            let finished = if state == "exited" {
                "2026-10-29T12:00:01Z"
            } else {
                "0001-01-01T00:00:00Z"
            };
            let mut name = fs::read_to_string(root.join("name")).unwrap();
            if case == "name_swap" && root.join("terminal_ack").exists() {
                name = "renamed-owned-instance".into();
                fs::write(root.join("other_instance"), "running").unwrap();
            }
            let volume = fs::read_to_string(root.join("volume")).unwrap();
            let mut session = fs::read_to_string(root.join("label_agentsandbox_session")).unwrap();
            let mut operation =
                fs::read_to_string(root.join("label_agentsandbox_operation")).unwrap();
            let mut purpose = fs::read_to_string(root.join("label_agentsandbox_purpose")).unwrap();
            if root.join("lookup_foreign_labels").exists() {
                session = "99999999-9999-4999-8999-999999999999".into();
                operation = "88888888-8888-4888-8888-888888888888".into();
                purpose = "export".into();
            }
            let state_json = format!(
                r#"{{"Status":"{state}","Running":{running},"StartedAt":"{started}","FinishedAt":"{finished}","ExitCode":0,"OOMKilled":false}}"#
            );
            let state_json = if root.join("omit_started_at").exists() {
                state_json.replace(&format!("\"StartedAt\":\"{started}\","), "")
            } else if root.join("contradict_finished_at").exists() {
                state_json.replace(
                    &format!("\"FinishedAt\":\"{finished}\""),
                    "\"FinishedAt\":\"2026-10-29T12:00:01Z\"",
                )
            } else {
                state_json
            };
            let state_json = if case == "missing_oom" {
                state_json.replace(",\"OOMKilled\":false", "")
            } else {
                state_json
            };
            if args.iter().any(|a| a == "{{json .State}}") {
                println!("{state_json}");
                return;
            }
            let interactive = fs::read_to_string(root.join("interactive")).unwrap();
            let mut json = format!(
                r#"{{"Id":"{ID}","Name":"/{name}","Image":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","RestartCount":0,"State":{state_json},"HostConfig":{{"LogConfig":{{"Type":"none","Config":{{}}}},"NanoCpus":2000000000,"CpuQuota":0,"CpuPeriod":0,"Memory":1073741824,"PidsLimit":256,"ReadonlyRootfs":true,"Privileged":false,"AutoRemove":false,"Tmpfs":{{"/tmp":"rw,nosuid,nodev,noexec,size=256m,mode=1777","/home/agent":"rw,nosuid,nodev,size=64m,uid=1000,gid=1000,mode=700"}},"NetworkMode":"none","CapDrop":["ALL"],"SecurityOpt":["no-new-privileges"],"RestartPolicy":{{"Name":"no"}}}},"Config":{{"Tty":false,"OpenStdin":{interactive},"StdinOnce":{interactive},"Labels":{{"agentsandbox.session":"{session}","agentsandbox.operation":"{operation}","agentsandbox.purpose":"{purpose}"}},"User":"1000:1000","WorkingDir":"/workspace","Entrypoint":["/usr/bin/id"],"Cmd":[],"Env":["PRIVATE=not-a-public-observation"]}},"Mounts":[{{"Type":"volume","Name":"{volume}","Destination":"/workspace","RW":true}}]}}"#
            );
            if case == "bad_log_driver"
                || case == "legacy_observe"
                || case == "logging_changed" && state != "created"
            {
                json = json.replace("\"Type\":\"none\"", "\"Type\":\"local\"");
            }
            if case == "missing_log_driver" {
                json = json.replace("\"Type\":\"none\",", "");
            }
            if case == "bad_log_options" {
                json = json.replace(
                    "\"Type\":\"none\",\"Config\":{}",
                    "\"Type\":\"none\",\"Config\":{\"max-size\":\"1m\"}",
                );
            }
            for (case_name, field) in [
                ("bad_tty", "Tty"),
                ("bad_open_stdin", "OpenStdin"),
                ("bad_stdin_once", "StdinOnce"),
            ] {
                if case == case_name {
                    json =
                        json.replace(&format!("\"{field}\":false"), &format!("\"{field}\":true"));
                }
            }
            if case == "bad_tmpfs" {
                json = json.replace("noexec", "exec");
            }
            if case == "bad_mount" {
                json = json.replace("aig-fixture-worker-work", "other-work");
            }
            if case == "bad_privileged" || case == "config_changed" && state != "created" {
                json = json.replace("\"Privileged\":false", "\"Privileged\":true");
            }
            if case == "bad_command" {
                json = json.replace("/usr/bin/id", "/usr/bin/other");
            }
            if case == "bad_image" {
                json = json.replace(
                    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                );
            }
            println!("{json}");
        }
        "kill" => {
            assert_eq!(args.last().unwrap(), ID);
            fs::write(root.join("state"), "exited").unwrap();
        }
        "rm" => {
            assert_eq!(args.last().unwrap(), ID);
            assert!(root.join("terminal_ack").exists());
            fs::remove_file(root.join("state")).unwrap();
        }
        _ => std::process::exit(125),
    }
}
