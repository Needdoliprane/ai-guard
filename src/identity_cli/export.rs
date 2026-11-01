//! Bounded private export preparation; only a complete snapshot is published.
use super::http::{Client, selection_query, validate_event_page, validate_page};
use super::offline::{destination, print_json, save};
use super::{ContextArgs, ExportArgs, ResumeExportArgs, SelectionArgs, View};
use crate::{
    control_fs::{Directory, PrivateFile, publication::PublicationFailure},
    identity::*,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    time::{Duration, Instant},
};

const MAX_PAGES: usize = 25;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_SECONDS: i64 = 60;
const PLAN_BYTES: u64 = 16_384;
const RECORD_BYTES: u64 = 300_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    version: u32,
    gateway_origin: String,
    credential_digest: String,
    context_id: String,
    view: HumanReadViewV1,
    resource: ResourceRefV1,
    created_at: String,
    final_name: String,
    parent_path: PathBuf,
    directory_device: u64,
    directory_inode: u64,
}
impl Plan {
    fn selection(&self) -> SelectionArgs {
        use super::ResourceKind;
        SelectionArgs {
            view: match self.view {
                HumanReadViewV1::Metadata => View::Metadata,
                HumanReadViewV1::Content => View::Content,
            },
            resource_kind: match self.resource.kind {
                ResourceKindV1::Session => ResourceKind::Session,
                ResourceKindV1::Sandbox => ResourceKind::Sandbox,
                ResourceKindV1::Agent => ResourceKind::Agent,
                ResourceKindV1::Project => ResourceKind::Project,
            },
            resource_id: self.resource.resource_id.clone(),
        }
    }
    fn verify(
        &self,
        client: &Client,
        access: &ContextArgs,
        parent: &Directory,
        directory: &Directory,
        leaf: &str,
    ) -> Result<()> {
        self.resource.validate()?;
        super::http::validate_opaque(&self.context_id)?;
        let facts = directory.try_clone_file()?.metadata()?;
        ensure!(
            self.version == 1
                && self.gateway_origin == client.origin()
                && self.credential_digest == client.credential_digest
                && self.context_id == access.context
                && self.final_name == leaf
                && self.parent_path == parent.path()
                && self.directory_device == facts.dev()
                && self.directory_inode == facts.ino(),
            "retained export differs from the exact gateway, private credential, context or output destination; no preparation is adopted"
        );
        chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .context("retained export creation time is invalid")?;
        Ok(())
    }
    fn remaining(&self) -> Result<Duration> {
        let created = chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .context("retained export creation time is invalid")?;
        let elapsed = chrono::Utc::now()
            .signed_duration_since(created)
            .num_milliseconds();
        ensure!(
            (0..MAX_SECONDS * 1000).contains(&elapsed),
            "export traversal exceeded its 60 second bound or the clock changed; retain this incomplete preparation and start a new explicit export in a new destination"
        );
        Ok(Duration::from_millis((MAX_SECONDS * 1000 - elapsed) as u64))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPage {
    version: u32,
    response_bytes: usize,
    page: HumanPageV1<HumanEventItemV1>,
}
#[derive(Serialize)]
struct PageEvidence {
    file: String,
    sha256: String,
    response_bytes: usize,
    read_receipt: HumanReadReceiptV1,
}
#[derive(Serialize)]
struct Manifest {
    version: u32,
    selection_complete: bool,
    coverage: &'static str,
    authorization_version: String,
    page_count: usize,
    response_bytes: usize,
    pages: Vec<PageEvidence>,
    receipt_scope: &'static str,
}
struct Prepared {
    page_count: usize,
    response_bytes: usize,
    stored_bytes: usize,
    next_cursor: Option<String>,
    authorization_version: Option<String>,
    complete: bool,
    evidence: Vec<PageEvidence>,
    cursors: BTreeSet<String>,
    files: Vec<PrivateFile>,
}

pub(super) async fn start(args: ExportArgs) -> Result<()> {
    let client = Client::new(&args.access.connection)?;
    super::http::validate_opaque(&args.access.context)?;
    selection_query(&args.selection)?;
    let (parent, leaf) = destination(&args.output_dir)?;
    parent.require_absent_child(&leaf)?;
    let stage_name = format!("{leaf}.preparing");
    let stage = parent.exclusive_child(&stage_name)
        .context("export preparation could not be created; preserve existing entries and use resume-export for an existing preparation")?;
    let view = match args.selection.view {
        View::Metadata => HumanReadViewV1::Metadata,
        View::Content => HumanReadViewV1::Content,
    };
    let resource = ResourceRefV1 {
        kind: match args.selection.resource_kind {
            super::ResourceKind::Session => ResourceKindV1::Session,
            super::ResourceKind::Sandbox => ResourceKindV1::Sandbox,
            super::ResourceKind::Agent => ResourceKindV1::Agent,
            super::ResourceKind::Project => ResourceKindV1::Project,
        },
        resource_id: args.selection.resource_id,
    };
    let facts = stage.try_clone_file()?.metadata()?;
    let plan = Plan {
        version: 1,
        gateway_origin: client.origin().into(),
        credential_digest: client.credential_digest.clone(),
        context_id: args.access.context.clone(),
        view,
        resource,
        created_at: chrono::Utc::now().to_rfc3339(),
        final_name: leaf.clone(),
        parent_path: parent.path().to_owned(),
        directory_device: facts.dev(),
        directory_inode: facts.ino(),
    };
    let result = async {
        let bytes = serde_json::to_vec_pretty(&plan)?;
        ensure!(
            bytes.len() as u64 <= PLAN_BYTES,
            "export selection exceeds its bound"
        );
        let plan_file = save(&stage, "selection.json", &bytes)?;
        stage.durable_bootstrap()?;
        ensure!(
            plan_file.persistent_try_lock()?,
            "export preparation is already in use"
        );
        run(
            &client,
            &args.access,
            &parent,
            &stage,
            &leaf,
            &plan_file,
            &plan,
        )
        .await
    }
    .await;
    retained(result, &parent, &stage_name, &leaf)
}

pub(super) async fn resume(args: ResumeExportArgs) -> Result<()> {
    let client = Client::new(&args.access.connection)?;
    let (parent, leaf) = destination(&args.output_dir)?;
    let stage_name = format!("{leaf}.preparing");
    let staged = parent.persistent_child(&stage_name, false)?;
    let published = parent.persistent_child(&leaf, false)?;
    ensure!(
        staged.is_none() || published.is_none(),
        "both final export and preparation exist; preserve them, no namespace is adopted or overwritten"
    );
    let already_published = published.is_some();
    let stage = staged
        .or(published)
        .context("retained export preparation is missing; no export is recreated implicitly")?;
    let plan_file = stage.persistent_read("selection.json",PLAN_BYTES,&[])?
        .context("export selection receipt is missing; retain this incomplete preparation and start a new explicit export in a new destination")?;
    ensure!(
        plan_file.persistent_try_lock()?,
        "export preparation is in use; retry after the other command exits"
    );
    let plan: Plan = serde_json::from_slice(plan_file.bytes()).map_err(|_| {
        anyhow::anyhow!("retained export selection is invalid; preserve the preparation")
    })?;
    plan.verify(&client, &args.access, &parent, &stage, &leaf)?;
    let result = if already_published {
        let prepared = inspect(&stage, &plan, &plan_file)?;
        ensure!(
            prepared.complete,
            "published export lacks a complete retained selection; preserve it for diagnosis"
        );
        verify_manifest(&stage, &prepared)?;
        current_authorization(
            &client,
            &args.access,
            prepared.authorization_version.as_deref(),
        )
        .await?;
        sync_pages(&prepared)?;
        stage.sync()?;
        parent.sync()?;
        exported(&parent, &leaf, &prepared)
    } else {
        run(
            &client,
            &args.access,
            &parent,
            &stage,
            &leaf,
            &plan_file,
            &plan,
        )
        .await
    };
    retained(result, &parent, &stage_name, &leaf)
}

fn retained(result: Result<()>, parent: &Directory, stage_name: &str, leaf: &str) -> Result<()> {
    result.with_context(|| format!(
        "export is unresolved; preserve {} and any final {}; run human resume-export with the same gateway, credential file, exact context and output directory; an expired context requires a new explicit export in a new destination",
        parent.path().join(stage_name).display(),parent.path().join(leaf).display()
    ))
}

fn inspect(stage: &Directory, plan: &Plan, plan_file: &PrivateFile) -> Result<Prepared> {
    plan_file.persistent_revalidate()?;
    let entries = stage.persistent_entries((MAX_PAGES + 3) as u32)?;
    ensure!(
        entries.iter().all(|entry| !entry.directory
            && (entry.name == "selection.json"
                || entry.name == "completion.json"
                || (1..=MAX_PAGES).any(|n| entry.name == format!("page-{n:04}.json")))),
        "export preparation contains unexpected entries; preserve them without adoption or cleanup"
    );
    let mut result = Prepared {
        page_count: 0,
        response_bytes: 0,
        stored_bytes: plan_file.bytes().len(),
        next_cursor: None,
        authorization_version: None,
        complete: false,
        evidence: vec![],
        cursors: BTreeSet::new(),
        files: vec![],
    };
    let view = plan.selection().view;
    let mut missing = false;
    for number in 1..=MAX_PAGES {
        let name = format!("page-{number:04}.json");
        let file = stage.persistent_read(&name, RECORD_BYTES, &[])?;
        let Some(file) = file else {
            missing = true;
            continue;
        };
        ensure!(
            !missing && !result.complete,
            "export page sequence has a gap or data after completion; preserve the preparation"
        );
        let record: StoredPage = serde_json::from_slice(file.bytes())
            .map_err(|_| anyhow::anyhow!("retained export page is partial or invalid; preserve it and start a new export in a new destination"))?;
        admit(
            &mut result,
            &record,
            view,
            &plan.context_id,
            file.bytes().len(),
            &plan.resource,
        )?;
        result.evidence.push(PageEvidence {
            file: name,
            sha256: file.digest().into(),
            response_bytes: record.response_bytes,
            read_receipt: record.page.read_receipt,
        });
        result.files.push(file);
    }
    ensure!(
        result.page_count > 0 || !entries.iter().any(|entry| entry.name == "completion.json"),
        "export completion receipt has no pages"
    );
    Ok(result)
}

fn admit(
    prepared: &mut Prepared,
    record: &StoredPage,
    view: View,
    context: &str,
    stored_bytes: usize,
    resource: &ResourceRefV1,
) -> Result<()> {
    validate_event_page(&record.page, context, view)?;
    ensure!(
        record.version == 1
            && record.response_bytes <= super::http::MAX_PAGE_BYTES
            && record
                .page
                .items
                .iter()
                .all(|item| &item.metadata.resource == resource),
        "export page differs from its explicit resource selection"
    );
    ensure!(
        record.page.selection_complete == Some(!record.page.has_more),
        "export server did not confirm the exact selection completion state"
    );
    ensure!(
        prepared
            .authorization_version
            .as_ref()
            .is_none_or(|r| r == &record.page.authorization_version),
        "authorization version changed during export; preserve this incomplete preparation"
    );
    ensure!(
        prepared.page_count < MAX_PAGES,
        "export exceeds the 25 page client bound"
    );
    prepared.response_bytes = prepared
        .response_bytes
        .checked_add(record.response_bytes)
        .context("export byte counter overflow")?;
    prepared.stored_bytes = prepared
        .stored_bytes
        .checked_add(stored_bytes)
        .context("export stored byte counter overflow")?;
    ensure!(
        prepared.response_bytes <= MAX_BYTES && prepared.stored_bytes <= MAX_BYTES,
        "export exceeds the 4 MiB client bound; retain this incomplete preparation and choose a smaller explicit selection"
    );
    if let Some(next) = &record.page.next_cursor {
        ensure!(
            prepared.cursors.insert(next.clone()),
            "export repeated a cursor; preserve the incomplete preparation"
        );
    }
    prepared.next_cursor = record.page.next_cursor.clone();
    prepared.authorization_version = Some(record.page.authorization_version.clone());
    prepared.complete = record.page.selection_complete == Some(true);
    prepared.page_count += 1;
    Ok(())
}

async fn run(
    client: &Client,
    access: &ContextArgs,
    parent: &Directory,
    stage: &Directory,
    leaf: &str,
    plan_file: &PrivateFile,
    plan: &Plan,
) -> Result<()> {
    plan.verify(client, access, parent, stage, leaf)?;
    let mut prepared = inspect(stage, plan, plan_file)?;
    let remaining = plan.remaining()?;
    tokio::time::timeout(remaining,async {
        let deadline = Instant::now() + remaining;
        while !prepared.complete {
            ensure!(prepared.page_count < MAX_PAGES && Instant::now() < deadline, "export exceeded its page or time bound; preparation retained");
            let mut query = selection_query(&plan.selection())?;
            query.push(("limit".into(),"100".into()));
            if let Some(cursor) = &prepared.next_cursor { query.push(("cursor".into(),cursor.clone())); }
            let (page,response_bytes): (HumanPageV1<HumanEventItemV1>,_) = client.get("export",Some(&access.context),&query).await?;
            let sanitized = client.sanitized(&page)?;
            let page = serde_json::from_value(sanitized).map_err(|_| anyhow::anyhow!("redacted export response is invalid"))?;
            let record = StoredPage { version:1,response_bytes,page };
            let bytes = serde_json::to_vec_pretty(&record)?;
            ensure!(bytes.len() as u64 <= RECORD_BYTES, "export record exceeds its private page bound");
            admit(&mut prepared,&record,plan.selection().view,&access.context,bytes.len(),&plan.resource)?;
            let name = format!("page-{:04}.json",prepared.page_count);
            plan_file.persistent_revalidate()?;
            let file = save(stage,&name,&bytes)?;
            prepared.evidence.push(PageEvidence { file:name,sha256:file.digest().into(),response_bytes,read_receipt:record.page.read_receipt });
            prepared.files.push(file);
        }
        current_authorization(client,access,prepared.authorization_version.as_deref()).await?;
        plan_file.persistent_sync()?;
        sync_pages(&prepared)?;
        publish_manifest(stage,&prepared)?;
        client.revalidate()?;
        stage.sync()?;
        ensure!(Instant::now() < deadline, "export exceeded its 60 second client bound before publication; preserve the complete preparation");
        parent.require_absent_child(leaf)?;
        match parent.publish_child_exclusive(stage,leaf) {
            Ok(()) => {
                let final_directory = parent.persistent_child(leaf,false)?.context("published export directory is unavailable; preserve the final destination")?;
                let final_plan = final_directory.persistent_read("selection.json",PLAN_BYTES,&[])?.context("published export selection receipt is unavailable")?;
                ensure!(final_plan.bytes() == plan_file.bytes(), "published export selection differs from its retained preparation");
                plan.verify(client,access,parent,&final_directory,leaf)?;
                let final_prepared = inspect(&final_directory,plan,&final_plan)?;
                verify_manifest(&final_directory,&final_prepared)?;
                sync_pages(&final_prepared)?;
                final_directory.sync()?;
                parent.sync()?;
                ensure!(Instant::now() < deadline, "export was published but final verification exceeded the 60 second bound; preserve the final output and verify it with resume-export");
                exported(parent,leaf,&final_prepared)
            },
            Err(PublicationFailure::NotPublished) => anyhow::bail!("complete export remains in preparation; exclusive publication failed, no destination was overwritten"),
            Err(PublicationFailure::DurabilityUnknown) => anyhow::bail!("export rename may have succeeded but durability is unknown; preserve both paths and resume the exact export to verify the final receipt"),
        }
    }).await.map_err(|_| anyhow::anyhow!("export exceeded its 60 second client bound; retain this incomplete preparation and start a new explicit export in a new destination"))?
}

fn sync_pages(prepared: &Prepared) -> Result<()> {
    for file in &prepared.files {
        file.persistent_sync()?;
    }
    Ok(())
}

async fn current_authorization(
    client: &Client,
    access: &ContextArgs,
    expected_version: Option<&str>,
) -> Result<()> {
    // Every completed traversal has a durable receipt. This final context read
    // rechecks revocation and the selected authorization version before publication.
    let query = vec![("limit".into(), "1".into())];
    let (page, _): (HumanPageV1<HumanResourceItemV1>, _) = client
        .get("resources", Some(&access.context), &query)
        .await?;
    validate_page(&page, &access.context)?;
    ensure!(
        expected_version == Some(page.authorization_version.as_str()),
        "export authorization changed before publication; preserve the preparation"
    );
    Ok(())
}

fn manifest(prepared: &Prepared) -> Result<Vec<u8>> {
    ensure!(prepared.complete, "incomplete export cannot be published");
    serde_json::to_vec_pretty(&Manifest {
        version:1,selection_complete:true,coverage:"authorized_selection",
        authorization_version:prepared.authorization_version.clone().context("completed export authorization version missing")?,
        page_count:prepared.page_count,response_bytes:prepared.response_bytes,
        pages:prepared.evidence.iter().map(|p| PageEvidence { file:p.file.clone(),sha256:p.sha256.clone(),response_bytes:p.response_bytes,read_receipt:p.read_receipt.clone() }).collect(),
        receipt_scope:"control recorded each authorized batch; this is not a signed complete audit chain or proof of a human reading the files",
    }).map_err(Into::into)
}
fn verify_manifest(stage: &Directory, prepared: &Prepared) -> Result<()> {
    let expected = manifest(prepared)?;
    ensure!(
        prepared.stored_bytes.saturating_add(expected.len()) <= MAX_BYTES,
        "completed export exceeds the 4 MiB stored byte bound"
    );
    let existing = stage
        .persistent_read("completion.json", PLAN_BYTES, &[])?
        .context("complete export publication receipt is missing; preserve the retained files")?;
    ensure!(
        existing.bytes() == expected,
        "export completion receipt conflicts with retained page evidence"
    );
    existing.persistent_sync()?;
    Ok(())
}
fn publish_manifest(stage: &Directory, prepared: &Prepared) -> Result<()> {
    let bytes = manifest(prepared)?;
    ensure!(
        prepared.stored_bytes.saturating_add(bytes.len()) <= MAX_BYTES,
        "completed export exceeds the 4 MiB stored byte bound"
    );
    if stage
        .persistent_read("completion.json", PLAN_BYTES, &[])?
        .is_some()
    {
        verify_manifest(stage, prepared)
    } else {
        save(stage, "completion.json", &bytes)?;
        Ok(())
    }
}
fn exported(parent: &Directory, leaf: &str, prepared: &Prepared) -> Result<()> {
    print_json(
        &serde_json::json!({"version":1,"status":"exported","output_dir":parent.path().join(leaf),"selection_complete":true,"coverage":"authorized_selection","pages":prepared.page_count,"response_bytes":prepared.response_bytes,"authorization_version":prepared.authorization_version}),
    )
}
