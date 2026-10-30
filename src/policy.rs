use crate::model::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Serialize, Deserialize)]
pub struct Policy {
    pub allowed_argv: Vec<Vec<String>>,
    pub denied_executables: Vec<String>,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            allowed_argv: [
                vec!["/usr/bin/id"],
                vec!["id"],
                vec!["/bin/pwd"],
                vec!["pwd"],
                vec!["/bin/echo", "sandbox-ok"],
                vec!["echo", "sandbox-ok"],
                vec!["/usr/bin/printf", "sandbox-ok\\n"],
            ]
            .into_iter()
            .map(|a| a.into_iter().map(str::to_string).collect())
            .collect(),
            denied_executables: [
                "rm", "dd", "mkfs", "mount", "umount", "sudo", "su", "docker", "podman", "nsenter",
                "unshare", "reboot", "shutdown",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        }
    }
}
pub fn request_hash(req: &DecisionRequest) -> String {
    let mut request = req.clone();
    request.human_approved = false;
    request.approval_hash = None;
    hex::encode(Sha256::digest(
        serde_json::to_vec(&request).expect("serializable request"),
    ))
}
pub fn evaluate(policy: &Policy, req: &DecisionRequest) -> Decision {
    let deny = |rule, reason| Decision::new(Outcome::Deny, rule, reason);
    if !valid_provider_context(req) {
        return deny(
            "invalid_provider_context",
            "Choix du client, de l'authentification ou de l'invocation incohérent.",
        );
    }
    if uuid::Uuid::parse_str(&req.request_id).is_err() {
        return deny("invalid_request_id", "Identifiant de demande invalide.");
    }
    if !["strict", "review", "observe"].contains(&req.profile.as_str()) {
        return deny("unknown_profile", "Profil inconnu : refus par défaut.");
    }
    if req.command.cwd != "/workspace" {
        return deny(
            "workspace_boundary",
            "Le répertoire autorisé est /workspace uniquement.",
        );
    }
    let argv = &req.command.argv;
    if argv.is_empty()
        || argv.len() > 256
        || argv.iter().map(|a| a.len()).sum::<usize>() > 32_768
        || argv.iter().any(|a| a.contains('\0'))
    {
        return deny(
            "invalid_argv",
            "Commande vide, trop volumineuse ou contenant NUL.",
        );
    }
    let executable = argv[0].as_str();
    if executable.is_empty()
        || executable.starts_with('-')
        || executable.chars().any(char::is_control)
        || executable.contains("..")
        || (executable.contains('/')
            && !executable.starts_with("/usr/bin/")
            && !executable.starts_with("/bin/")
            && !executable.starts_with("/usr/local/bin/"))
    {
        return deny(
            "invalid_executable",
            "Exécutable hors des chemins système autorisés.",
        );
    }
    let base = Path::new(executable)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if policy.denied_executables.iter().any(|b| b == base) {
        return deny(
            "denied_executable",
            "Exécutable explicitement interdit par la politique locale.",
        );
    }
    if req.human_approved && req.approval_hash.as_deref() != Some(request_hash(req).as_str()) {
        return deny(
            "invalid_approval",
            "L'approbation ne correspond pas au condensat de cette demande.",
        );
    }
    if policy.allowed_argv.iter().any(|allowed| allowed == argv) {
        return Decision::new(
            Outcome::Allow,
            "exact_allowlist",
            "Invocation exacte présente dans la politique locale.",
        );
    }
    match req.profile.as_str() {
        "strict" => deny(
            "strict_default_deny",
            "Invocation absente de l'allowlist exacte ; aucun lancement.",
        ),
        "review" if req.human_approved => Decision::new(
            Outcome::Allow,
            "human_approval",
            "Approbation humaine liée au condensat exact de la demande.",
        ),
        "review" => Decision::new(
            Outcome::Review,
            "human_review_required",
            "Confirmation humaine liée à la commande requise.",
        ),
        "observe" => Decision::new(
            Outcome::Allow,
            "observation_only",
            "Lancement journalisé ; aucune garantie de contrôle des commandes transitives.",
        ),
        _ => unreachable!(),
    }
}

pub(crate) fn valid_provider_context(req: &DecisionRequest) -> bool {
    match &req.provider_resolution {
        Some(resolution) => {
            resolution.validate().is_ok()
                && resolution.validate_argv(&req.command.argv).is_ok()
                && resolution.transport.is_interactive() == req.execution_mode.is_some()
                && req.profile == "observe"
        }
        None => req
            .command
            .argv
            .first()
            .is_none_or(|program| program != crate::provider::BOOTSTRAP_PROGRAM),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn request(argv: &[&str], profile: &str) -> DecisionRequest {
        DecisionRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            profile: profile.into(),
            command: CommandSpec {
                argv: argv.iter().map(|s| s.to_string()).collect(),
                cwd: "/workspace".into(),
            },
            project_id: None,
            sandbox_id: None,
            agent_id: None,
            session_id: None,
            correlation_id: None,
            trace_id: None,
            human_approved: false,
            approval_hash: None,
            execution_mode: None,
            provider_resolution: None,
        }
    }
    #[test]
    fn strict_denies_unknown_and_shell() {
        for argv in [
            vec!["sh", "-c", "touch marker"],
            vec!["node", "-e", "process.exit(0)"],
            vec!["rm", "-rf", "/workspace"],
        ] {
            assert_eq!(
                evaluate(&Policy::default(), &request(&argv, "strict")).outcome,
                Outcome::Deny
            );
        }
    }
    #[test]
    fn review_requires_exact_bound_hash() {
        let mut r = request(&["cargo", "test"], "review");
        assert_eq!(evaluate(&Policy::default(), &r).outcome, Outcome::Review);
        r.human_approved = true;
        r.approval_hash = Some("wrong".into());
        assert_eq!(evaluate(&Policy::default(), &r).outcome, Outcome::Deny);
        r.approval_hash = Some(request_hash(&r));
        assert_eq!(evaluate(&Policy::default(), &r).outcome, Outcome::Allow);
        r.command.argv.push("--changed".into());
        assert_eq!(evaluate(&Policy::default(), &r).outcome, Outcome::Deny);
    }
    #[test]
    fn cwd_escape_and_unknown_profile_fail_closed() {
        let mut r = request(&["/usr/bin/id"], "strict");
        r.command.cwd = "/workspace/../../".into();
        assert_eq!(evaluate(&Policy::default(), &r).outcome, Outcome::Deny);
        r.command.cwd = "/workspace".into();
        r.profile = "loose".into();
        assert_eq!(evaluate(&Policy::default(), &r).outcome, Outcome::Deny);
    }
    #[test]
    fn known_exact_command_allowed_no_added_args() {
        assert_eq!(
            evaluate(&Policy::default(), &request(&["/usr/bin/id"], "strict")).outcome,
            Outcome::Allow
        );
        assert_eq!(
            evaluate(
                &Policy::default(),
                &request(&["/usr/bin/id", ";touch", "/tmp/a"], "strict")
            )
            .outcome,
            Outcome::Deny
        );
    }
}
