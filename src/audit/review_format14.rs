//! Closed extension of tenant13. This replay proves descriptive consistency;
//! it never constructs ownership, an epoch, an incarnation or launch consent.
use super::review_authority::ReviewAuthorityFailure;
use super::*;

/// The parent composes these with every exact object from format13.
pub(super) fn schema_objects()
-> impl Iterator<Item = &'static (&'static str, &'static str, &'static str)> {
    review_authority::SCHEMA
        .iter()
        .chain(review_binding::SCHEMA.iter())
}

/// SQLite's automatic indexes are part of the exact new14 profile as well.
pub(super) fn auto_index_counts() -> impl Iterator<Item = &'static (&'static str, u32)> {
    review_authority::AUTO_INDEX_COUNTS
        .iter()
        .chain(review_binding::AUTO_INDEX_COUNTS.iter())
}

fn exact(value: bool) -> anyhow::Result<()> {
    if value {
        Ok(())
    } else {
        Err(ReviewAuthorityFailure::Mismatch.into())
    }
}

/// Validate the new histories and their shared source, namespace and tenant.
/// Exact SQLite DDL and every inherited replay remain the parent's job.
pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let format: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    // Exact15 inherits these unchanged14 source/binding facts; its additional
    // five objects and replay are separately closed by the parent and egress.
    exact(matches!(format, 14 | 15))?;
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    crate::tenancy::validate_tenant_id(&tenant)?;
    let authority = review_authority::replay(db)?;
    exact(authority.anchor.tenant_id == tenant)?;
    review_binding::validate(db, &tenant)?;
    validate_binding_source_history(db, &authority)
}

/// These fields are read only after both closed typed replays above succeeded.
/// They bind each historical binding to the incarnation that preceded it,
/// rather than comparing an old binding to the latest owner's incarnation.
fn validate_binding_source_history(
    db: &Connection,
    authority: &review_authority::AuthoritySnapshot,
) -> anyhow::Result<()> {
    let source_id = authority.anchor.source_id.to_string();
    let namespace = authority.anchor.durable_namespace_id.to_string();
    let mut incarnation: Option<Value> = None;
    let mut query = db.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.review_authority.*' OR kind GLOB 'control.review_binding.*' ORDER BY seq",
    )?;
    for event in query.query_map([], row_event)? {
        let stored = event?;
        match stored.event.kind.as_str() {
            review_authority::ANCHOR_KIND => {
                exact(stored.seq == authority.anchor_receipt.seq)?;
            }
            review_authority::INCARNATION_KIND => {
                // review_authority::replay already checked every typed record,
                // previous receipt and exact projection, including this value.
                incarnation = Some(stored.event.data["record"].clone());
            }
            "control.review_binding.provisioned" | "control.review_binding.revoked" => {
                let current = incarnation
                    .as_ref()
                    .ok_or(ReviewAuthorityFailure::Mismatch)?;
                let record = &stored.event.data;
                let binding = &record["snapshot"]["binding"];
                exact(
                    stored.seq > authority.anchor_receipt.seq
                        && record["authority_source_id"].as_str() == Some(source_id.as_str())
                        && record["owner_incarnation"].as_str()
                            == current["owner_incarnation"].as_str()
                        && record["owner_incarnation"].is_string()
                        && binding["tenant_id"].as_str()
                            == Some(authority.anchor.tenant_id.as_str())
                        && binding["control_namespace_uuid"].as_str() == Some(namespace.as_str())
                        && binding["closure_id"] == current["closure_id"]
                        && binding["closure_revision"] == current["closure_revision"]
                        && binding["closure_digest"] == current["closure_digest"]
                        && binding["control_config_public_digest"]
                            == current["control_config_public_digest"],
                )?;
            }
            _ => return Err(ReviewAuthorityFailure::Mismatch.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
