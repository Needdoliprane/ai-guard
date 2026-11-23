//! Per-context operation catalogue served with each offered human context.
//!
//! This is a presentation contract computed by the control from the context's
//! mode, its effective capabilities and server facts. It never authorizes a
//! request: every route revalidates the membership, the OIDC ceiling and each
//! resource permission. Permission (from the membership), availability (from
//! this build, the source profile and the store marker) and the outcome of a
//! read (429/503 at request time) stay three separate facts.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Closed operation identifiers. Static class and resource-ACL facts of each
/// identifier are part of the documented contract, not of this wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanOperationV1 {
    ResourcesRead,
    EventsReadMetadata,
    EventsReadContent,
    EventsExport,
    EventsStream,
    SummaryRead,
    BrokersRead,
    WorkersRead,
    FederationRead,
    QuotaRead,
    UsageRead,
    MembershipsRead,
    ConfigurationRead,
    MembershipsManage,
    GrantsManage,
    LaunchApprove,
    SessionLaunchPrepare,
    SessionLaunch,
    SessionMessageSend,
    SessionMessagesRead,
    SessionAttributionRead,
    EgressPoliciesRead,
    EgressAttemptsRead,
    EgressGrantsRead,
    EgressCoverageRead,
    EgressSummaryRead,
    EgressGrantApprove,
    EgressGrantRevoke,
}
impl HumanOperationV1 {
    pub const ALL: [Self; 28] = [
        Self::ResourcesRead,
        Self::EventsReadMetadata,
        Self::EventsReadContent,
        Self::EventsExport,
        Self::EventsStream,
        Self::SummaryRead,
        Self::BrokersRead,
        Self::WorkersRead,
        Self::FederationRead,
        Self::QuotaRead,
        Self::UsageRead,
        Self::MembershipsRead,
        Self::ConfigurationRead,
        Self::MembershipsManage,
        Self::GrantsManage,
        Self::LaunchApprove,
        Self::SessionLaunchPrepare,
        Self::SessionLaunch,
        Self::SessionMessageSend,
        Self::SessionMessagesRead,
        Self::SessionAttributionRead,
        Self::EgressPoliciesRead,
        Self::EgressAttemptsRead,
        Self::EgressGrantsRead,
        Self::EgressCoverageRead,
        Self::EgressSummaryRead,
        Self::EgressGrantApprove,
        Self::EgressGrantRevoke,
    ];
}

/// Why the selected context does or does not carry an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationPermissionV1 {
    Granted,
    NotGranted,
    /// The membership carries the right, but only an administration context
    /// of the same membership offers it.
    RequiresAdministration,
    /// The membership carries the right, but only its personal context offers it.
    RequiresPersonal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationScopeV1 {
    /// Resources the principal owns or that are shared with it.
    OwnAndShared,
    /// Resources of the membership's organization in the tenant.
    Organization,
    /// The tenant's control registries.
    Tenant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationAvailabilityV1 {
    Available,
    /// The tenant store lacks the explicit marker this operation records under.
    MigrationRequired,
    /// The adopted source profile has no protocol for this operation.
    ProfileUnavailable,
    /// This build serves no route for this operation.
    NotInThisBuild,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanOperationAccessV1 {
    pub operation: HumanOperationV1,
    pub permission: OperationPermissionV1,
    /// Present exactly when the permission is granted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<OperationScopeV1>,
    pub availability: OperationAvailabilityV1,
}

/// Server facts that bound availability, independent of any permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationFactsV1 {
    /// The gateway serves the owned source profile (Store14/15 sources).
    pub owned_profile: bool,
    /// The tenant store carries the validated human rights marker.
    pub human_rights_marker: bool,
}

/// The complete closed catalogue for one offered context: one entry per
/// operation, in `HumanOperationV1::ALL` order.
pub fn context_operations(
    mode: ContextModeV1,
    context_capabilities: &BTreeSet<HumanCapabilityV1>,
    membership_capabilities: &BTreeSet<HumanCapabilityV1>,
    facts: OperationFactsV1,
) -> Vec<HumanOperationAccessV1> {
    HumanOperationV1::ALL
        .into_iter()
        .map(|operation| {
            let (permission, scope) = permission(
                operation,
                mode,
                context_capabilities,
                membership_capabilities,
            );
            HumanOperationAccessV1 {
                operation,
                permission,
                scope,
                availability: availability(operation, facts),
            }
        })
        .collect()
}

fn permission(
    operation: HumanOperationV1,
    mode: ContextModeV1,
    context: &BTreeSet<HumanCapabilityV1>,
    membership: &BTreeSet<HumanCapabilityV1>,
) -> (OperationPermissionV1, Option<OperationScopeV1>) {
    use HumanCapabilityV1 as C;
    use HumanOperationV1 as O;
    use OperationPermissionV1 as P;
    use OperationScopeV1 as S;
    let administration = mode == ContextModeV1::Administration;
    let has = |capability| context.contains(&capability);
    // A right the membership holds but this mode does not offer.
    let elsewhere = |required: &[C], personal_only: bool| {
        if required.iter().all(|c| membership.contains(c)) {
            if personal_only {
                P::RequiresPersonal
            } else {
                P::RequiresAdministration
            }
        } else {
            P::NotGranted
        }
    };
    let metadata_scope = if administration && has(C::ReadTenantMetadata) {
        Some(S::Organization)
    } else if has(C::ReadMetadata) {
        Some(S::OwnAndShared)
    } else {
        None
    };
    let granted = |scope| (P::Granted, Some(scope));
    let refused = |permission| (permission, None);
    match operation {
        O::ResourcesRead
        | O::EventsReadMetadata
        | O::EventsStream
        | O::SessionAttributionRead
        | O::EgressPoliciesRead
        | O::EgressAttemptsRead
        | O::EgressGrantsRead
        | O::EgressCoverageRead
        | O::EgressSummaryRead => match metadata_scope {
            Some(scope) => granted(scope),
            None => refused(P::NotGranted),
        },
        O::EventsReadContent | O::SessionMessagesRead
            if has(C::ReadContent) && metadata_scope.is_some() =>
        {
            granted(S::OwnAndShared)
        }
        O::EventsExport if has(C::Export) && metadata_scope.is_some() => granted(S::OwnAndShared),
        O::EventsReadContent | O::EventsExport | O::SessionMessagesRead => refused(P::NotGranted),
        O::SummaryRead => {
            if administration && context.iter().any(|c| c.is_administration()) {
                granted(S::Tenant)
            } else if !administration && has(C::ReadMetadata) {
                granted(S::OwnAndShared)
            } else {
                refused(P::NotGranted)
            }
        }
        O::BrokersRead => admin(administration, context, &[C::ReadBrokerRegistry], elsewhere),
        O::WorkersRead | O::QuotaRead | O::UsageRead => {
            admin(administration, context, &[C::ReadWorkerRegistry], elsewhere)
        }
        O::FederationRead => admin(
            administration,
            context,
            &[C::ReadWorkerRegistry, C::ReadBrokerRegistry],
            elsewhere,
        ),
        O::MembershipsRead => admin(administration, context, &[C::ReadMemberships], elsewhere),
        O::ConfigurationRead => admin(administration, context, &[C::ReadConfiguration], elsewhere),
        O::MembershipsManage => admin(
            administration,
            context,
            &[C::ManageMemberships, C::ReadMemberships],
            elsewhere,
        ),
        // The share list names organization members: a membership reader only.
        O::GrantsManage => admin(
            administration,
            context,
            &[C::ManageGrants, C::ReadMemberships],
            elsewhere,
        ),
        O::EgressGrantApprove | O::EgressGrantRevoke => {
            admin(administration, context, &[C::ApproveEgress], elsewhere)
        }
        O::LaunchApprove => personal(administration, context, &[C::ApproveLaunch], elsewhere),
        O::SessionLaunchPrepare | O::SessionLaunch => {
            personal(administration, context, &[C::LaunchSession], elsewhere)
        }
        O::SessionMessageSend => {
            personal(administration, context, &[C::SendSessionMessage], elsewhere)
        }
    }
}

fn admin(
    administration: bool,
    context: &BTreeSet<HumanCapabilityV1>,
    required: &[HumanCapabilityV1],
    elsewhere: impl Fn(&[HumanCapabilityV1], bool) -> OperationPermissionV1,
) -> (OperationPermissionV1, Option<OperationScopeV1>) {
    if administration && required.iter().all(|c| context.contains(c)) {
        (
            OperationPermissionV1::Granted,
            Some(OperationScopeV1::Tenant),
        )
    } else if administration {
        (OperationPermissionV1::NotGranted, None)
    } else {
        (elsewhere(required, false), None)
    }
}

fn personal(
    administration: bool,
    context: &BTreeSet<HumanCapabilityV1>,
    required: &[HumanCapabilityV1],
    elsewhere: impl Fn(&[HumanCapabilityV1], bool) -> OperationPermissionV1,
) -> (OperationPermissionV1, Option<OperationScopeV1>) {
    if !administration && required.iter().all(|c| context.contains(c)) {
        (
            OperationPermissionV1::Granted,
            Some(OperationScopeV1::OwnAndShared),
        )
    } else if !administration {
        (OperationPermissionV1::NotGranted, None)
    } else {
        (elsewhere(required, true), None)
    }
}

/// Whether this build serves a route for the operation. A slice that adds a
/// route flips its entry; nothing else in the catalogue changes.
const fn served(operation: HumanOperationV1) -> bool {
    use HumanOperationV1 as O;
    matches!(
        operation,
        O::ResourcesRead
            | O::EventsReadMetadata
            | O::EventsReadContent
            | O::EventsExport
            | O::EventsStream
            | O::BrokersRead
            | O::WorkersRead
            | O::FederationRead
            | O::SummaryRead
            | O::QuotaRead
            | O::UsageRead
            | O::MembershipsRead
            | O::ConfigurationRead
    )
}

/// Operations whose reads or effects are recorded under a marker-fenced audit
/// action or capability.
const fn requires_marker(operation: HumanOperationV1) -> bool {
    use HumanOperationV1 as O;
    matches!(
        operation,
        O::SummaryRead
            | O::QuotaRead
            | O::UsageRead
            | O::MembershipsRead
            | O::ConfigurationRead
            | O::MembershipsManage
            | O::GrantsManage
            | O::SessionLaunchPrepare
            | O::SessionLaunch
            | O::SessionMessageSend
            | O::SessionMessagesRead
            | O::SessionAttributionRead
            | O::EgressGrantApprove
            | O::EgressGrantRevoke
    )
}

fn availability(operation: HumanOperationV1, facts: OperationFactsV1) -> OperationAvailabilityV1 {
    use HumanOperationV1 as O;
    use OperationAvailabilityV1 as A;
    if !served(operation) {
        A::NotInThisBuild
    } else if facts.owned_profile
        && matches!(
            operation,
            O::EventsReadContent
                | O::EventsExport
                | O::EventsStream
                | O::FederationRead
                | O::QuotaRead
                | O::UsageRead
        )
    {
        // Owned sources serve the two registries' metadata as separate reads.
        A::ProfileUnavailable
    } else if requires_marker(operation) && !facts.human_rights_marker {
        A::MigrationRequired
    } else {
        A::Available
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        operations: &[HumanOperationAccessV1],
        operation: HumanOperationV1,
    ) -> &HumanOperationAccessV1 {
        operations
            .iter()
            .find(|entry| entry.operation == operation)
            .unwrap()
    }

    #[test]
    fn availability_follows_the_source_profile_and_the_store_marker() {
        use HumanCapabilityV1 as C;
        let capabilities: BTreeSet<_> = [
            C::ReadWorkerRegistry,
            C::ReadBrokerRegistry,
            C::ReadMemberships,
        ]
        .into_iter()
        .collect();
        let owned = context_operations(
            ContextModeV1::Administration,
            &capabilities,
            &capabilities,
            OperationFactsV1 {
                owned_profile: true,
                human_rights_marker: true,
            },
        );
        for operation in [
            HumanOperationV1::FederationRead,
            HumanOperationV1::QuotaRead,
            HumanOperationV1::UsageRead,
        ] {
            assert_eq!(
                entry(&owned, operation).availability,
                OperationAvailabilityV1::ProfileUnavailable,
                "{operation:?} on the owned profile"
            );
        }
        for operation in [
            HumanOperationV1::BrokersRead,
            HumanOperationV1::WorkersRead,
            HumanOperationV1::SummaryRead,
            HumanOperationV1::MembershipsRead,
        ] {
            assert_eq!(
                entry(&owned, operation).availability,
                OperationAvailabilityV1::Available,
                "{operation:?} is served on the owned profile"
            );
        }
        let unmarked = context_operations(
            ContextModeV1::Administration,
            &capabilities,
            &capabilities,
            OperationFactsV1 {
                owned_profile: false,
                human_rights_marker: false,
            },
        );
        assert_eq!(
            entry(&unmarked, HumanOperationV1::MembershipsRead).availability,
            OperationAvailabilityV1::MigrationRequired
        );
        assert_eq!(
            entry(&unmarked, HumanOperationV1::WorkersRead).availability,
            OperationAvailabilityV1::Available
        );
        assert_eq!(unmarked.len(), HumanOperationV1::ALL.len());
    }

    #[test]
    fn owned_event_availability_preserves_granted_permissions() {
        use HumanCapabilityV1 as C;
        use HumanOperationV1 as O;
        let capabilities = [C::ReadMetadata, C::ReadContent, C::Export]
            .into_iter()
            .collect();
        let catalogue = |owned_profile| {
            context_operations(
                ContextModeV1::Personal,
                &capabilities,
                &capabilities,
                OperationFactsV1 {
                    owned_profile,
                    human_rights_marker: true,
                },
            )
        };
        let owned = catalogue(true);
        let legacy = catalogue(false);
        assert_eq!(
            entry(&owned, O::EventsReadMetadata).availability,
            OperationAvailabilityV1::Available
        );
        for operation in [O::EventsReadContent, O::EventsExport, O::EventsStream] {
            let owned = entry(&owned, operation);
            let legacy = entry(&legacy, operation);
            assert_eq!(owned.permission, OperationPermissionV1::Granted);
            assert_eq!(owned.scope, legacy.scope);
            assert_eq!(
                owned.availability,
                OperationAvailabilityV1::ProfileUnavailable,
                "{operation:?} on the owned profile"
            );
            assert_eq!(legacy.availability, OperationAvailabilityV1::Available);
        }
    }

    #[test]
    fn a_right_held_by_the_other_mode_is_named_not_granted_here() {
        use HumanCapabilityV1 as C;
        let membership: BTreeSet<_> = [C::ReadMetadata, C::ReadBrokerRegistry, C::LaunchSession]
            .into_iter()
            .collect();
        let personal: BTreeSet<_> = [C::ReadMetadata, C::LaunchSession].into_iter().collect();
        let facts = OperationFactsV1 {
            owned_profile: false,
            human_rights_marker: true,
        };
        let operations = context_operations(ContextModeV1::Personal, &personal, &membership, facts);
        assert_eq!(
            entry(&operations, HumanOperationV1::BrokersRead).permission,
            OperationPermissionV1::RequiresAdministration
        );
        assert_eq!(
            entry(&operations, HumanOperationV1::SessionLaunch).permission,
            OperationPermissionV1::Granted
        );
        let administration = context_operations(
            ContextModeV1::Administration,
            &membership,
            &membership,
            facts,
        );
        assert_eq!(
            entry(&administration, HumanOperationV1::SessionLaunch).permission,
            OperationPermissionV1::RequiresPersonal
        );
        assert_eq!(
            entry(&administration, HumanOperationV1::EgressGrantApprove).permission,
            OperationPermissionV1::NotGranted
        );
    }
}
