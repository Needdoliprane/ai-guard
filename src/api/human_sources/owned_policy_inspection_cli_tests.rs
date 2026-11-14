//! Actual Cargo-bound CLI against actual owned gateways; no future Rust API.
//! Root supplies the exact same QA executable/digest variables as parent A1.
use super::{
    owned_policy_cli_tests::{ActualCli, CliOutput, private_input, text},
    owned_policy_inspection_tests::{Fixture, Server, RUNNER, INSPECT, PROTOCOL, policy, replacement, snapshot},
};
use crate::{
    control::egress_policy::{EgressPolicyReceiptV1, EgressReplacePolicyRequestV1},
    runner::AuditClient,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    path::Path,
};

const REPLACE: &str = "/control/api/v1/egress/policies/replace";

fn stdout(output: &CliOutput) -> Result<Value> {
    ensure!(output.stdout.ends_with(b"\n")
        && output.stdout.iter().filter(|b| **b == b'\n').count() == 1
        && !output.stdout.contains(&b'\r'), "CLI summary must be one complete line");
    let value: Value = serde_json::from_slice(&output.stdout)?;
    ensure!(value["historical"] == true, "CLI summary lost historical coverage");
    for forbidden in [RUNNER,"example.com","destinations","control_namespace_uuid","binding_id","runtime_applied"] {
        ensure!(!String::from_utf8_lossy(&output.stdout).contains(forbidden)
            && !String::from_utf8_lossy(&output.stderr).contains(forbidden), "CLI summary leaked data or claimed runtime coverage");
    }
    Ok(value)
}

fn preparation(path: &Path) -> Result<(EgressReplacePolicyRequestV1, Vec<u8>, Vec<u8>)> {
    let bytes=std::fs::read(path.join("request.json"))?;
    let request=EgressReplacePolicyRequestV1::from_json(&bytes)?;
    ensure!(request.canonical_bytes()? == bytes,"prepare did not persist canonical existing A1 DTO");
    let metadata=std::fs::read(path.join("metadata.json"))?;
    ensure!(std::fs::metadata(path)?.permissions().mode()&0o777==0o700,"operation directory permissions");
    for name in ["request.json","metadata.json","current.json","owner.lock"] {
        ensure!(std::fs::symlink_metadata(path.join(name))?.is_file()
            && std::fs::metadata(path.join(name))?.permissions().mode()&0o777==0o600,"operation file permissions");
    }
    Ok((request,bytes,metadata))
}

fn report(path: &Path, origin: &str, fixture: &Fixture) -> Result<Value> {
    ensure!(std::fs::metadata(path)?.permissions().mode()&0o777==0o700,"inspection directory is not private");
    let names=std::fs::read_dir(path)?.map(|entry|entry.map(|e|e.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<BTreeSet<_>>>()?;
    let expected=["request.json","metadata.json","current.json","owner.lock"].into_iter().map(str::to_owned).collect();
    ensure!(names==expected,"inspection report gained pending/receipt/unknown records");
    for name in &names {
        let meta=std::fs::symlink_metadata(path.join(name))?;
        ensure!(meta.is_file()&&meta.permissions().mode()&0o777==0o600,"inspection record is not private regular file");
    }
    let request=std::fs::read(path.join("request.json"))?;
    let observation=std::fs::read(path.join("current.json"))?;
    let metadata:Value=serde_json::from_slice(&std::fs::read(path.join("metadata.json"))?)?;
    let value:Value=serde_json::from_slice(&observation)?;
    ensure!(request==br#"{"version":1,"selector":{"kind":"tenant"}}"#
        &&metadata["version"]==1&&metadata["kind"]=="policy_inspection"&&metadata["gateway"]==origin
        &&metadata["request_sha256"]==hex::encode(Sha256::digest(&request))
        &&metadata["observation_sha256"]==hex::encode(Sha256::digest(&observation)),"report binding/hash contract");
    ensure!(value["scope"]==json!({"tenant_id":"A","control_namespace_uuid":fixture.namespace})
        &&value["selector"]==json!({"kind":"tenant"})&&value["head"]==json!({"presence":"absent"})
        &&value["historical"]==true&&value["coverage"]=="policy_registry_metadata","actual retained observation");
    ensure!(!String::from_utf8_lossy(&observation).contains(RUNNER)
        &&!String::from_utf8_lossy(&observation).contains("example.com"),"inspection report exposed secret/policy");
    Ok(value)
}

async fn inspect_cli(cli:&ActualCli,server:&Server,token:&Path,destination:&Path)->Result<CliOutput> {
    cli.run(&["broker","policy","inspect","--gateway",&server.origin,"--token-file",text(token)?,
        "--tenant","--output-dir",text(destination)?]).await
}

async fn prepare_cli(cli:&ActualCli,inspection:&Path,draft:&Path,destination:&Path)->Result<CliOutput> {
    cli.run(&["broker","policy","prepare","--inspection-dir",text(inspection)?,"--input",text(draft)?,
        "--output-dir",text(destination)?]).await
}

fn unchanged(path:&Path,request:&[u8],metadata:&[u8])->Result<()> {
    ensure!(std::fs::read(path.join("request.json"))?==request
        &&std::fs::read(path.join("metadata.json"))?==metadata,"apply/resume repaired immutable operation/scope/CAS");
    Ok(())
}

fn persisted_receipt(database:&Path,operation:uuid::Uuid,receipt:&[u8])->Result<()> {
    let db=rusqlite::Connection::open_with_flags(database,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let (canonical,seq,hash,kind):(String,i64,String,String)=db.query_row(
        "SELECT o.receipt_json,e.seq,e.hash,e.kind FROM egress_policy_operations o JOIN events e ON e.seq=o.event_seq WHERE o.operation_id=?1",
        [operation.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let value:Value=serde_json::from_slice(receipt)?;
    ensure!(canonical.as_bytes()==receipt&&value["operation_id"]==operation.to_string()
        &&value["event_seq"]==seq&&value["event_hash"]==hash&&kind=="control.egress_policy.replaced",
        "CLI receipt not correlated with independent actual SQLite event");
    Ok(())
}

// Break caught: guessed scope/CAS, implicit inspection during prepare/apply,
// changed UUID on recovery, or success without a durable canonical receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an explicitly selected same-build Cargo-bound normal CLI in isolated QA"]
async fn real_cli_inspect_prepare_apply_keeps_observed_scope_cas_and_operation()->Result<()> {
    let files=tempfile::Builder::new().permissions(Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir())?)?;
    let cli=ActualCli::from_qa(files.path())?;
    let fixture=Fixture::new(15);let mut server=Server::new(&fixture.state).await?;
    let token=files.path().join("runner-token");private_input(&token,RUNNER.as_bytes())?;
    let inspection=files.path().join("inspection");let draft=files.path().join("draft.json");
    let operation=files.path().join("operation");
    let desired=policy();private_input(&draft,&serde_json::to_vec(&json!({"version":1,"action":"replace","policy":desired}))?)?;
    let outcome:Result<(uuid::Uuid,Vec<u8>)>=async {
        server.sdk_witness(&fixture,RUNNER,"A",15).await?;
        let before=snapshot(&fixture)?;
        println!("OPI_CLI_SETUP_REAL_OWNED15_REGISTRY_BINDING_SDK200");
        let output=inspect_cli(&cli,&server,&token,&inspection).await?;
        let absent=String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand 'inspect'");
        ensure!(output.status.success()||absent,"OPI_CLI_FAILURE_NOT_MISSING_INSPECT");
        ensure!(output.status.success(),"OPI_CLI_INSPECT_EXPECTED_RECOGNIZED_SUCCESS_AFTER_REAL_SETUP actual_exit={:?} invalid_subcommand={absent}",output.status.code());
        let summary=stdout(&output)?;ensure!(summary["coverage"]=="policy_registry_metadata","inspection CLI coverage");
        let observed=report(&inspection,&server.origin,&fixture)?;
        ensure!(snapshot(&fixture)?==before&&server.count("POST",INSPECT)==1&&server.count("POST",REPLACE)==0,
            "inspection produced effects or duplicate requests");
        let requests=server.paths.lock().unwrap().len();
        let output=prepare_cli(&cli,&inspection,&draft,&operation).await?;
        ensure!(output.status.success(),"inspection-based prepare failed");
        let (request,request_bytes,metadata)=preparation(&operation)?;
        let id=request.operation_id();
        let summary=stdout(&output)?;
        ensure!(summary["operation_id"]==id.to_string()&&summary["phase"]=="prepared"&&summary["attempts"]==0,
            "derived preparation summary");
        ensure!(serde_json::to_value(request.expected_scope())?==observed["scope"]
            &&request.selector()==&crate::control::egress_policy::EgressPolicySelectorV1::Tenant
            &&request.expected_revision()==0&&request.expected_epoch()==0&&request.policy()==&desired
            &&request.tenant_ceiling().is_none(),"prepared A1 scope/CAS/policy diverged from explicit report/draft");
        ensure!(server.paths.lock().unwrap().len()==requests&&snapshot(&fixture)?==before,"prepare performed IO/effect");

        // Old complete-envelope prepare remains a network-free supported mode.
        let old_input=files.path().join("old-input.json");let old_operation=files.path().join("old-operation");
        let old=replacement(&fixture,0,0);
        private_input(&old_input,&serde_json::to_vec(&json!({"version":1,"action":"replace","request":old}))?)?;
        let output=cli.run(&["broker","policy","prepare","--gateway",&server.origin,"--input",text(&old_input)?,
            "--output-dir",text(&old_operation)?]).await?;
        ensure!(output.status.success()&&preparation(&old_operation)?.0.operation_id()==old.operation_id(),"old complete prepare regressed");
        ensure!(server.paths.lock().unwrap().len()==requests&&snapshot(&fixture)?==before,"old prepare performed IO/effect");

        // A report is intentionally incompatible with a mutation directory.
        let output=cli.run(&["broker","policy","apply","--operation-dir",text(&inspection)?,"--token-file",text(&token)?]).await?;
        ensure!(!output.status.success()&&server.paths.lock().unwrap().len()==requests,"apply adopted an inspection report");
        let output=cli.run(&["broker","policy","apply","--operation-dir",text(&operation)?,"--token-file",text(&token)?]).await?;
        ensure!(output.status.success(),"real derived A1 apply failed");
        let summary=stdout(&output)?;
        ensure!(summary["operation_id"]==id.to_string()&&summary["phase"]=="receipt_recorded"
            &&summary["attempts"]==1&&summary["receipt_present"]==true,"derived apply result");
        let receipt=std::fs::read(operation.join("receipt.json"))?;
        let parsed=EgressPolicyReceiptV1::from_canonical_bytes(&receipt)?;parsed.validate_for_replace(&request)?;
        unchanged(&operation,&request_bytes,&metadata)?;
        ensure!(server.count("POST",INSPECT)==1&&server.count("POST",REPLACE)==1
            &&server.count("GET",PROTOCOL)==2,"apply did not preserve exact explicit network path");
        let output=cli.run(&["broker","policy","resume","--operation-dir",text(&operation)?,"--token-file",text(&token)?]).await?;
        ensure!(output.status.success(),"retained receipt resume failed");
        let summary=stdout(&output)?;
        ensure!(summary["operation_id"]==id.to_string()&&summary["attempts"]==1
            &&std::fs::read(operation.join("receipt.json"))?==receipt,"resume changed operation or canonical receipt");
        unchanged(&operation,&request_bytes,&metadata)?;
        ensure!(server.count("POST",INSPECT)==1&&server.count("POST",REPLACE)==1&&server.count("GET",PROTOCOL)==2,
            "receipt resume performed hidden inspection/preflight/effect");
        Ok((id,receipt))
    }.await;
    let close=server.close().await;let drain=fixture.drain().await;
    close?;drain?;let (id,receipt)=outcome?;
    persisted_receipt(&fixture.database,id,&receipt)
}

// Break caught: refreshing a retained head/CAS or accepting the same bearer at
// the same origin after its actual namespace changed. Both Stores are real.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an explicitly selected same-build Cargo-bound normal CLI in isolated QA"]
async fn retained_inspection_never_repairs_changed_head_or_namespace()->Result<()> {
    let files=tempfile::Builder::new().permissions(Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir())?)?;
    let cli=ActualCli::from_qa(files.path())?;
    let fixture=Fixture::new(15);let next=Fixture::new(15);
    ensure!(fixture.namespace!=next.namespace,"namespace replacement fixture not distinct");
    let mut server=Server::new(&fixture.state).await?;
    let token=files.path().join("runner-token");private_input(&token,RUNNER.as_bytes())?;
    let inspection=files.path().join("inspection");let draft=files.path().join("draft.json");
    let operation=files.path().join("stale-operation");let replaced=files.path().join("namespace-operation");
    private_input(&draft,&serde_json::to_vec(&json!({"version":1,"action":"replace","policy":policy()}))?)?;
    let outcome:Result<()>=async {
        server.sdk_witness(&fixture,RUNNER,"A",15).await?;
        println!("OPI_RETAINED_SETUP_REAL_OWNED15_REGISTRY_BINDING_SDK200");
        let output=inspect_cli(&cli,&server,&token,&inspection).await?;
        let absent=String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand 'inspect'");
        ensure!(output.status.success()||absent,"OPI_RETAINED_CLI_FAILURE_NOT_MISSING_INSPECT");
        ensure!(output.status.success(),"OPI_RETAINED_CLI_INSPECT_EXPECTED_RECOGNIZED_SUCCESS_AFTER_REAL_SETUP actual_exit={:?} invalid_subcommand={absent}",output.status.code());
        stdout(&output)?;report(&inspection,&server.origin,&fixture)?;
        let inspection_bytes=std::fs::read(inspection.join("current.json"))?;
        let sdk=AuditClient::new(&server.origin,RUNNER)?;
        let changed=replacement(&fixture,0,0);sdk.egress_replace_policy(&changed).await?;
        let stable=snapshot(&fixture)?;
        let output=prepare_cli(&cli,&inspection,&draft,&operation).await?;ensure!(output.status.success(),"stale report preparation failed");
        let (request,bytes,metadata)=preparation(&operation)?;let id=request.operation_id();
        ensure!(request.expected_revision()==0&&request.expected_epoch()==0,"prepare repaired stale report CAS");
        let posts=server.count("POST",REPLACE);
        for (command,attempts) in [("apply",1),("resume",2)] {
            let output=cli.run(&["broker","policy",command,"--operation-dir",text(&operation)?,"--token-file",text(&token)?]).await?;
            ensure!(!output.status.success(),"stale CAS was accepted");
            let summary=stdout(&output)?;
            ensure!(summary["operation_id"]==id.to_string()&&summary["attempts"]==attempts
                &&summary["last_error"]=="egress_policy_conflict"&&summary["receipt_present"]==false,"stale CAS did not retain typed Conflict");
            unchanged(&operation,&bytes,&metadata)?;
            ensure!(server.count("POST",INSPECT)==1&&server.count("POST",REPLACE)==posts+attempts as usize
                &&snapshot(&fixture)?==stable,"stale operation refreshed head or had effects");
        }
        ensure!(std::fs::read(inspection.join("current.json"))?==inspection_bytes,"apply rewrote retained observation");
        let output=prepare_cli(&cli,&inspection,&draft,&replaced).await?;
        ensure!(output.status.success(),"namespace test preparation failed");
        let (request,bytes,metadata)=preparation(&replaced)?;
        let id=request.operation_id();
        ensure!(request.expected_scope().control_namespace_uuid()==fixture.namespace,"old namespace not retained");
        let retained_origin=server.origin.clone();server.replace(&next.state);
        // A real diagnostic at the unchanged origin proves the new authority.
        server.sdk_witness(&next,RUNNER,"A",15).await?;
        ensure!(server.origin==retained_origin,"test changed URL instead of actual gateway");
        let next_before=snapshot(&next)?;let posts=server.count("POST",REPLACE);
        for command in ["apply","resume"] {
            let output=cli.run(&["broker","policy",command,"--operation-dir",text(&replaced)?,"--token-file",text(&token)?]).await?;
            ensure!(!output.status.success(),"changed namespace accepted");
            // Existing A1 rejects ScopeConflict at its real A0 preflight, so
            // there need not be a JSON mutation summary or effect POST.
            ensure!(String::from_utf8_lossy(&output.stderr).contains("egress_policy_scope_conflict"),"changed namespace did not return ScopeConflict");
            unchanged(&replaced,&bytes,&metadata)?;
            ensure!(request.operation_id()==id&&server.count("POST",INSPECT)==1
                &&server.count("POST",REPLACE)==posts&&snapshot(&next)?==next_before,
                "changed namespace triggered inspect/repair/effect");
        }
        ensure!(snapshot(&fixture)?==stable,"namespace test touched retired authority");
        Ok(())
    }.await;
    let close=server.close().await;let old_drain=fixture.drain().await;let new_drain=next.drain().await;
    close?;old_drain?;new_drain?;outcome
}
