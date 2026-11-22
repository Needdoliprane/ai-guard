//! Wire of the administration memberships read and of the delegated
//! membership and share mutations of the 360° views.
use super::*;
use crate::redaction::Redactor;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Credential metadata of a local account; never a digest or a secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanMemberCredentialV1 {
    pub credential_id: String,
    pub active: bool,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanMembershipItemV1 {
    pub membership_id: String,
    pub principal: PrincipalRefV1,
    pub display_name: String,
    pub account_active: bool,
    pub organization_scope: String,
    pub capabilities: Vec<HumanCapabilityV1>,
    pub revision: String,
    pub active: bool,
    /// Local credentials of the account, at most 16; OIDC accounts have none.
    pub credentials: Vec<HumanMemberCredentialV1>,
    pub credentials_complete: bool,
    /// The reader's own membership: the delegated route refuses to change it.
    pub own: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanMembershipPageV1 {
    pub version: u32,
    pub context_id: String,
    pub authorization_version: String,
    pub source: String,
    pub consistency: String,
    pub evaluated_at: String,
    pub items: Vec<HumanMembershipItemV1>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub read_receipt: HumanReadReceiptV1,
}

impl HumanMembershipItemV1 {
    pub(crate) fn require_unredacted_metadata(&self, redactor: &Redactor) -> anyhow::Result<()> {
        check_membership_item(serde_json::to_value(self)?, redactor)
    }
}

impl HumanMembershipPageV1 {
    pub(crate) fn require_unredacted_metadata(&self, redactor: &Redactor) -> anyhow::Result<()> {
        let items = check_fields_except(serde_json::to_value(self)?, "items", redactor)?;
        let Value::Array(items) = items else {
            anyhow::bail!("invalid membership metadata page");
        };
        for item in items {
            check_membership_item(item, redactor)?;
        }
        Ok(())
    }
}

fn check_membership_item(item: Value, redactor: &Redactor) -> anyhow::Result<()> {
    // The closed DTO's `credentials` field holds credential metadata. Check the
    // collection itself without applying the generic secret-key rule to its name.
    let credentials = check_fields_except(item, "credentials", redactor)?;
    anyhow::ensure!(
        credentials.is_array() && redactor.value(&credentials) == credentials,
        "membership metadata requires redaction"
    );
    Ok(())
}

fn check_fields_except(mut body: Value, field: &str, redactor: &Redactor) -> anyhow::Result<Value> {
    let contents = body
        .as_object_mut()
        .and_then(|map| map.remove(field))
        .context("invalid membership metadata body")?;
    anyhow::ensure!(
        redactor.text(field) == field && redactor.value(&body) == body,
        "membership metadata requires redaction"
    );
    Ok(contents)
}

/// Delegated change of one existing membership of the reader's organization.
/// Principal, tenant and organization are taken from the stored membership,
/// never from this body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanMembershipChangeV1 {
    pub operation_id: String,
    pub expected_revision: String,
    pub capabilities: Vec<HumanCapabilityV1>,
    pub active: bool,
}

/// Durable outcome of a delegated membership change. Revisions and the audit
/// position are decimal strings, like every human count.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanMembershipChangeReceiptV1 {
    pub version: u32,
    pub operation_id: String,
    pub membership_id: String,
    pub membership_revision: String,
    pub authorization_revision: String,
    pub audit_seq: String,
}

/// Bound of a delegated change body: identifiers and at most every capability.
pub const MEMBERSHIP_CHANGE_BODY_MAX_BYTES: usize = 4096;

pub const MEMBER_CREDENTIALS_MAX: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_metadata_checks_wire_fields_without_masking_the_collection() {
        let credential_id = "00000000-0000-4000-8000-000000000004";
        let expires_at = "2027-01-01T00:00:00Z";
        let item = HumanMembershipItemV1 {
            membership_id: "00000000-0000-4000-8000-000000000001".into(),
            principal: PrincipalRefV1::HumanLocal {
                installation_id: "00000000-0000-4000-8000-000000000002".into(),
                account_id: "00000000-0000-4000-8000-000000000003".into(),
            },
            display_name: "Synthetic admin".into(),
            account_active: true,
            organization_scope: "synthetic-org".into(),
            capabilities: vec![HumanCapabilityV1::ReadMemberships],
            revision: "1".into(),
            active: true,
            credentials: vec![HumanMemberCredentialV1 {
                credential_id: credential_id.into(),
                active: true,
                expires_at: Some(expires_at.into()),
            }],
            credentials_complete: true,
            own: true,
        };
        let page = HumanMembershipPageV1 {
            version: 1,
            context_id: "a".repeat(64),
            authorization_version: "b".repeat(64),
            source: "tenant_identity_memberships".into(),
            consistency: "snapshot_per_page".into(),
            evaluated_at: "2026-11-22T10:00:00Z".into(),
            items: vec![item],
            has_more: false,
            next_cursor: None,
            read_receipt: HumanReadReceiptV1 {
                request_id: "00000000-0000-4000-8000-000000000005".into(),
                audit_event_id: "00000000-0000-4000-8000-000000000006".into(),
                received_at: "2026-11-22T10:00:00Z".into(),
            },
        };
        let redactor = Redactor::new(vec![]);
        let wire = serde_json::to_value(&page).unwrap();
        assert_eq!(
            redactor.value(&wire)["items"][0]["credentials"],
            "[REDACTED]"
        );
        page.items[0]
            .require_unredacted_metadata(&redactor)
            .unwrap();
        page.require_unredacted_metadata(&redactor).unwrap();
        assert_eq!(
            wire["items"][0]["credentials"],
            serde_json::json!([{"credential_id": credential_id, "active": true, "expires_at": expires_at}])
        );

        for literal in [credential_id, expires_at, "Synthetic admin", "credentials"] {
            let redactor = Redactor::new(vec![literal.into()]);
            assert!(
                page.items[0]
                    .require_unredacted_metadata(&redactor)
                    .is_err()
            );
            assert!(page.require_unredacted_metadata(&redactor).is_err());
        }
        let redactor = Redactor::new(vec!["snapshot_per_page".into()]);
        assert!(page.require_unredacted_metadata(&redactor).is_err());
    }
}
