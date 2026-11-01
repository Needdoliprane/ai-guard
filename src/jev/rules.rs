use super::{JevError, transport::bounded_json};
use crate::{
    model::{DecisionRequest, Outcome},
    redaction::Redactor,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogDocument {
    version: u32,
    allow_argv: Vec<Vec<String>>,
    #[serde(default)]
    deny_argv: Vec<Vec<String>>,
}

/// Trusted exact top-level argv only. This is not a shell or semantic analyser.
#[derive(Clone)]
pub struct RuleCatalog {
    document: CatalogDocument,
    digest: String,
    #[cfg(unix)]
    file: Option<crate::control_fs::PrivateFile>,
}
impl Default for RuleCatalog {
    fn default() -> Self {
        let document = CatalogDocument {
            version: 1,
            allow_argv: ["/usr/bin/id", "/usr/bin/true", "/bin/pwd"]
                .into_iter()
                .map(|program| vec![program.into()])
                .collect(),
            deny_argv: Vec::new(),
        };
        let digest = hex::encode(Sha256::digest(
            bounded_json(&document).expect("bounded built-in catalogue"),
        ));
        Self {
            document,
            digest,
            #[cfg(unix)]
            file: None,
        }
    }
}
impl RuleCatalog {
    /// Existing private file, retained and revalidated; no creation or repair.
    pub fn from_private_file(path: &std::path::Path) -> Result<Self, JevError> {
        #[cfg(unix)]
        {
            let path =
                crate::control_fs::absolute_path(path).map_err(|_| JevError::InvalidCatalog)?;
            let file = crate::control_fs::PrivateFile::open(&path, 65_536)
                .map_err(|_| JevError::InvalidCatalog)?;
            let document: CatalogDocument =
                serde_json::from_slice(file.bytes()).map_err(|_| JevError::InvalidCatalog)?;
            Self::validate(&document)?;
            let digest = file.digest().to_owned();
            Ok(Self {
                document,
                digest,
                file: Some(file),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(JevError::InvalidCatalog)
        }
    }
    fn validate(document: &CatalogDocument) -> Result<(), JevError> {
        let mut unique = std::collections::HashSet::new();
        if document.version != 1 || document.allow_argv.len() + document.deny_argv.len() > 128 {
            return Err(JevError::InvalidCatalog);
        }
        for argv in document.allow_argv.iter().chain(&document.deny_argv) {
            if argv.is_empty()
                || argv.len() > 32
                || !argv[0].starts_with('/')
                || argv
                    .iter()
                    .any(|arg| arg.len() > 4096 || arg.chars().any(char::is_control))
                || argv.iter().map(String::len).sum::<usize>() > 8192
                || !unique.insert(argv)
            {
                return Err(JevError::InvalidCatalog);
            }
        }
        Ok(())
    }
    pub(super) fn configuration(&self) -> Value {
        json!({"catalogue_version":1,"catalogue_sha256":self.digest,"source":if self.is_builtin(){"builtin"}else{"private_file"},"coverage":"exact_top_level_argv_only"})
    }
    fn is_builtin(&self) -> bool {
        #[cfg(unix)]
        {
            self.file.is_none()
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
    pub(super) fn validate_redaction(&self, redactor: &Redactor) -> Result<(), JevError> {
        let document =
            serde_json::to_value(&self.document).map_err(|_| JevError::InvalidCatalog)?;
        if redactor.value_unclipped(&document) != document {
            return Err(JevError::InvalidCatalog);
        }
        Ok(())
    }
    pub(super) fn evaluate(
        &self,
        request: &DecisionRequest,
    ) -> Result<(Outcome, &'static str), JevError> {
        #[cfg(unix)]
        if let Some(file) = &self.file {
            file.revalidate().map_err(|_| JevError::InvalidCatalog)?;
        }
        Ok(if self.document.deny_argv.contains(&request.command.argv) {
            (Outcome::Deny, "exact_catalogue_deny")
        } else if self.document.allow_argv.contains(&request.command.argv) {
            (Outcome::Allow, "exact_catalogue_allow_top_level_only")
        } else if request.provider_resolution.is_some()
            && request
                .command
                .argv
                .first()
                .is_some_and(|program| program == crate::provider::BOOTSTRAP_PROGRAM)
            && crate::policy::valid_provider_context(request)
        {
            (Outcome::Allow, "validated_managed_bootstrap_structure_only")
        } else {
            (Outcome::Review, "outside_incomplete_exact_catalogue")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalogue_is_versioned_exact_and_rejects_conflicting_entries() {
        let mut document = RuleCatalog::default().document;
        assert!(RuleCatalog::validate(&document).is_ok());
        document.deny_argv.push(document.allow_argv[0].clone());
        assert!(RuleCatalog::validate(&document).is_err());
        document.deny_argv.clear();
        document.version = 2;
        assert!(RuleCatalog::validate(&document).is_err());
    }
}
