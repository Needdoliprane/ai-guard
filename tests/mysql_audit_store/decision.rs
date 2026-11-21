use agentsandbox::{
    model::{CommandSpec, Decision, DecisionRequest, Outcome, StoredEvent},
    mysql_storage::audit::MysqlAuditStore,
    policy::request_hash,
};
use serde_json::json;

pub(super) async fn exercise(
    first: &MysqlAuditStore,
    reopened: &MysqlAuditStore,
) -> anyhow::Result<()> {
    let start = first.verify().await?;
    anyhow::ensure!(start.seq == 4, "decision exercise expects append head 4");

    // A lookup without a proposal is a read and leaves the chain untouched.
    let absent = request("strict", &["/usr/bin/id"]);
    anyhow::ensure!(
        first.decision(&absent, None).await?.is_none(),
        "a missing decision without a proposal must remain absent"
    );
    anyhow::ensure!(
        first.verify().await?.seq == start.seq,
        "lookup changed audit head"
    );

    // Concurrent writers with the same identity and content converge on one receipt.
    let concurrent = request("strict", &["/usr/bin/id"]);
    let proposal = (
        decision(Outcome::Allow, "oracle_allow"),
        json!({"oracle": "same"}),
    );
    let (left, right) = tokio::join!(
        first.decision(&concurrent, Some(proposal.clone())),
        reopened.decision(&concurrent, Some(proposal)),
    );
    let left = left?.expect("first concurrent writer returns a receipt");
    let right = right?.expect("retry returns the existing receipt");
    anyhow::ensure!(
        same_receipt(&left, &right),
        "same request returned different receipts"
    );
    anyhow::ensure!(
        left.seq == start.seq + 1,
        "concurrent writes created multiple events"
    );

    let mut conflict = concurrent.clone();
    conflict.command.argv.push("--different".into());
    anyhow::ensure!(
        reopened.decision(&conflict, None).await.is_err(),
        "the same request ID with different argv must conflict"
    );
    let mut ordinary_decision = agentsandbox::model::Event::new("security.decision.extra");
    ordinary_decision.id = uuid::Uuid::new_v4().to_string();
    anyhow::ensure!(
        first.append(ordinary_decision).await.is_err(),
        "decision namespace events must use the typed decision mutation"
    );
    anyhow::ensure!(
        first.verify().await?.seq == start.seq + 1,
        "refusal changed audit head"
    );

    // An exact review then approval is two durable writes. Approval requires the
    // exact hash and a review receipt, even if an approval receipt is cached.
    let review = request("review", &["cargo", "test", "mysql-oracle"]);
    let initial = first
        .decision(
            &review,
            Some((
                decision(Outcome::Review, "human_review_required"),
                json!({"review": true}),
            )),
        )
        .await?
        .expect("review proposal is stored");
    anyhow::ensure!(initial.seq == start.seq + 2, "review was not appended once");

    let mut approval_without_review = request("review", &["cargo", "test", "mysql-oracle"]);
    approval_without_review.human_approved = true;
    approval_without_review.approval_hash = Some(request_hash(&approval_without_review));
    anyhow::ensure!(
        reopened
            .decision(
                &approval_without_review,
                Some((
                    decision(Outcome::Allow, "human_approval"),
                    json!({"approved": true})
                )),
            )
            .await
            .is_err(),
        "approval without an earlier review must be refused"
    );
    anyhow::ensure!(
        first.verify().await?.seq == start.seq + 2,
        "review refusal changed audit head"
    );

    let mut approved = review.clone();
    approved.human_approved = true;
    approved.approval_hash = Some(request_hash(&review));
    let approval = reopened
        .decision(
            &approved,
            Some((
                decision(Outcome::Allow, "human_approval"),
                json!({"approved": true}),
            )),
        )
        .await?
        .expect("exact approval after review is stored");
    anyhow::ensure!(
        approval.seq == start.seq + 3,
        "approval was not appended once"
    );

    let mut wrong_hash = approved.clone();
    wrong_hash.approval_hash = Some("0".repeat(64));
    anyhow::ensure!(
        first
            .decision(
                &wrong_hash,
                Some((decision(Outcome::Allow, "cached_wrong_hash"), json!({}))),
            )
            .await
            .is_err(),
        "wrong approval hash must be refused despite the cached approval"
    );
    anyhow::ensure!(
        first.verify().await?.seq == start.seq + 3,
        "approval refusal changed audit head"
    );

    let replay = first
        .decision(
            &approved,
            Some((decision(Outcome::Deny, "ignored_retry"), json!({}))),
        )
        .await?
        .expect("other handle replays the durable approval");
    anyhow::ensure!(
        same_receipt(&approval, &replay),
        "reopened handle did not replay receipt"
    );
    anyhow::ensure!(
        first.verify().await?.seq == start.seq + 3,
        "replay changed audit head"
    );
    Ok(())
}

pub(super) fn request(profile: &str, argv: &[&str]) -> DecisionRequest {
    DecisionRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        profile: profile.into(),
        command: CommandSpec {
            argv: argv.iter().map(|arg| (*arg).into()).collect(),
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

fn decision(outcome: Outcome, rule: &str) -> Decision {
    Decision::new(outcome, rule, "MySQL decision storage exercise")
}

fn same_receipt(left: &StoredEvent, right: &StoredEvent) -> bool {
    left.seq == right.seq && left.hash == right.hash && left.event.id == right.event.id
}
