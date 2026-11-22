//! Store facts of the administration configuration read: format, marker and
//! the runtime kinds declared by registered brokers, recorded under the human
//! read action `configuration` in the same checked transaction.
use super::*;
use crate::identity::*;

pub(crate) struct HumanConfigurationStoreFactsV1 {
    pub store_format: i64,
    pub human_rights_marker: bool,
    /// Sorted, distinct runtime kinds of the registered broker authorities.
    pub declared_runtime_kinds: Vec<String>,
    pub read_receipt: HumanReadReceiptV1,
}

impl Store {
    pub(crate) fn human_configuration_facts(
        &self,
        scope: &AuthorizedScopeV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanConfigurationStoreFactsV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let facts = self.human_configuration_facts_checked(&tx, scope, transient)?;
        self.commit_checked(tx)?;
        Ok(facts)
    }
    /// The same facts on an owned 14/15 source, inside its scoped transaction.
    pub(crate) fn modern_human_configuration_facts(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanConfigurationStoreFactsV1> {
        self.with_modern_read(access, |tx| {
            self.human_configuration_facts_checked(tx, scope, transient)
        })
    }
    fn human_configuration_facts_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        scope: &AuthorizedScopeV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanConfigurationStoreFactsV1> {
        identity::require_human_read(tx)?;
        let current = identity::revalidate_scope_checked(tx, scope, &self.tenant_id)?;
        if current.mode() != ContextModeV1::Administration
            || !current.has_capability(HumanCapabilityV1::ReadConfiguration)
        {
            return Err(IdentityFailureV1::PermissionDenied.into());
        }
        let human_rights_marker = super::human_rights::installed(tx)?;
        if !human_rights_marker {
            return Err(IdentityFailureV1::MigrationRequired.into());
        }
        let store_format: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let declared_runtime_kinds = if (6..=15).contains(&store_format) {
            let mut statement = tx.prepare(
                "SELECT DISTINCT runtime_kind FROM broker_registry_authorities ORDER BY runtime_kind",
            )?;
            let kinds = statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for kind in &kinds {
                anyhow::ensure!(
                    matches!(kind.as_str(), "docker" | "podman" | "kubernetes"),
                    "invalid registered runtime kind"
                );
            }
            kinds
        } else {
            Vec::new()
        };
        let read_receipt = self.record_human_read_checked(
            tx,
            current.principal(),
            Some(&current),
            "configuration",
            1,
            transient,
        )?;
        Ok(HumanConfigurationStoreFactsV1 {
            store_format,
            human_rights_marker,
            declared_runtime_kinds,
            read_receipt,
        })
    }
}
