//! Private batch results operations.
use super::journal::same;
use super::*;
use crate::broker::terminal_facts::{
    BatchResultFactsV1, CaptureFactsV1, CoverageFactsV1, TransportFactsV1,
};
use futures_util::FutureExt;

impl Execution<'_> {
    pub(super) async fn save_worker_result(
        &self,
        retained: &ManagedPrearmedExecution,
    ) -> Result<()> {
        let (_, draft) = self.load().await?;
        if draft
            .run
            .plan
            .managed
            .as_ref()
            .is_some_and(|m| m.output_event.is_some())
        {
            return Ok(());
        }
        let worker = draft.worker()?;
        let capture = |capture: &crate::runtime::Capture| {
            let masked = draft.run.options().capture_output.then(|| {
                self.client
                    .redactor
                    .captured_text(&capture.bytes, capture.truncated)
            });
            CaptureFactsV1 {
                total_bytes: capture.total_bytes,
                truncated: capture.truncated,
                text_clipped: masked.as_ref().map(|text| text.len() > 8192),
                text: masked.map(|text| crate::redaction::clip(&text, 8192)),
            }
        };
        let facts = BatchResultFactsV1 {
            version: 1,
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            observation: retained.terminal().clone(),
            transport: Some(TransportFactsV1 {
                exit_code: retained.result().transport_exit_code,
                duration_ms_decimal: retained.result().duration_ms.to_string(),
                stdout: capture(&retained.result().stdout),
                stderr: capture(&retained.result().stderr),
            }),
        };
        let mut event = crate::runner::event(
            &draft.run.plan,
            "broker.batch.result",
            serde_json::to_value(&facts)?,
        );
        event["command"] = serde_json::to_value(&draft.run.plan.request.command)?;
        let event = self.client.redactor.value(&event);
        let typed: crate::model::Event = serde_json::from_value(event.clone())?;
        typed.validate()?;
        let request = &draft.run.plan.request;
        let adapter_context = crate::adapters::AdapterContext {
            session_id: draft.run.session().to_string(),
            project_id: request.project_id.clone().unwrap_or_default(),
            sandbox_id: request.sandbox_id.clone().unwrap_or_default(),
            agent_id: request.agent_id.clone().unwrap_or_default(),
            correlation_id: request.correlation_id.clone().unwrap_or_default(),
            trace_id: request.trace_id.clone().unwrap_or_default(),
        };
        let normalized = crate::adapters::parse_redacted_capture(
            &draft.run.options().provider,
            &retained.result().stdout.bytes,
            retained.result().stdout.truncated,
            &adapter_context,
            &self.client.redactor,
        );
        let mut events = Vec::with_capacity(normalized.events.len() + 2);
        for value in normalized.events {
            let mut event: crate::model::Event = serde_json::from_value(value)?;
            // Historical adapters use empty strings when context is absent.
            // V4 preserves the exact Option scope of the accepted intention.
            event.session_id = request.session_id.clone();
            event.project_id = request.project_id.clone();
            event.sandbox_id = request.sandbox_id.clone();
            event.correlation_id = request.correlation_id.clone();
            event.trace_id = request.trace_id.clone();
            events.push(event);
        }
        let coverage = CoverageFactsV1 {
            version: 1,
            normalized_events: Some(events.len()),
            maximum_events: crate::adapters::MAX_EVENTS_PER_CAPTURE,
            event_limit_reached: Some(events.len() == crate::adapters::MAX_EVENTS_PER_CAPTURE),
            stdout_truncated: Some(retained.result().stdout.truncated),
            unterminated_final_line_omitted: Some(normalized.unterminated_final_line_omitted),
            nested_authorization: false,
            raw_output_capture: draft.run.options().capture_output,
        };
        let mut coverage: crate::model::Event = serde_json::from_value(crate::runner::event(
            &draft.run.plan,
            "observer.coverage",
            serde_json::to_value(coverage)?,
        ))?;
        coverage.coverage = "partial".into();
        events.push(coverage);
        events.push(typed);
        let events = events
            .into_iter()
            .map(|event| {
                Ok(serde_json::from_value(
                    self.client.redactor.value(&serde_json::to_value(event)?),
                )?)
            })
            .collect::<Result<Vec<crate::model::Event>>>()?;
        // One durable revision freezes the whole bounded capture batch before
        // its first POST. A crash cannot leave a falsely complete partial batch.
        self.executor
            .writer
            .enqueue_events(self.operation, events)
            .await?;
        self.patch(|draft| {
            let managed = draft
                .run
                .plan
                .managed
                .as_mut()
                .context("managed result disappeared")?;
            if let Some(previous) = &managed.output_event {
                ensure!(same(previous, &event)?, "terminal result already differs");
            }
            managed.output_event = Some(event.clone());
            managed.output_acknowledged = false;
            managed.result = None;
            Ok(())
        })
        .await
    }

    pub(super) async fn save_observed_result(&self, terminal: &ContainerObservation) -> Result<()> {
        let (job, draft) = self.load().await?;
        if draft
            .run
            .plan
            .managed
            .as_ref()
            .is_some_and(|m| m.output_event.is_some())
        {
            return Ok(());
        }
        let existing = job.broker_checkpoint_v4().and_then(|cp| {
            cp.events()
                .iter()
                .find(|entry| entry.event().kind == "broker.batch.result")
                .map(|entry| entry.event().clone())
        });
        let event = if let Some(event) = existing {
            event
        } else {
            let facts = BatchResultFactsV1 {
                version: 1,
                worker_id: draft.worker()?.worker_id.clone(),
                attempt_id: draft.worker()?.attempt.attempt_id.clone(),
                observation: terminal.clone(),
                transport: None,
            };
            let event: crate::model::Event =
                serde_json::from_value(self.client.redactor.value(&crate::runner::event(
                    &draft.run.plan,
                    "broker.batch.result",
                    serde_json::to_value(facts)?,
                )))?;
            let coverage = CoverageFactsV1 {
                version: 1,
                normalized_events: None,
                maximum_events: crate::adapters::MAX_EVENTS_PER_CAPTURE,
                event_limit_reached: None,
                stdout_truncated: None,
                unterminated_final_line_omitted: None,
                nested_authorization: false,
                raw_output_capture: draft.run.options().capture_output,
            };
            let mut coverage: crate::model::Event = serde_json::from_value(crate::runner::event(
                &draft.run.plan,
                "observer.coverage",
                serde_json::to_value(coverage)?,
            ))?;
            coverage.coverage = "partial".into();
            self.executor
                .writer
                .enqueue_events(self.operation, vec![coverage, event.clone()])
                .await?;
            event
        };
        let event = serde_json::to_value(event)?;
        self.patch(|draft| {
            let managed = draft
                .run
                .plan
                .managed
                .as_mut()
                .context("managed state disappeared")?;
            ensure!(
                managed.output_event.is_none(),
                "terminal event appeared concurrently"
            );
            managed.output_event = Some(event.clone());
            managed.output_acknowledged = false;
            Ok(())
        })
        .await
    }

    pub(super) async fn post_worker_result(&self) -> Result<()> {
        let (_, draft) = self.load().await?;
        let managed = draft
            .run
            .plan
            .managed
            .as_ref()
            .context("managed state is missing")?;
        if managed.output_acknowledged {
            return Ok(());
        }
        let Some(event) = managed.output_event.clone() else {
            // Attach output lost across a process crash is unknown. A later
            // terminal observation cannot reconstruct historical captures.
            return Ok(());
        };
        self.executor
            .writer
            .enqueue_event(self.operation, serde_json::from_value(event.clone())?)
            .await?;
        self.replay_events().await?;
        self.patch(|draft| {
            let managed = draft
                .run
                .plan
                .managed
                .as_mut()
                .context("managed result disappeared")?;
            ensure!(
                managed
                    .output_event
                    .as_ref()
                    .is_some_and(|current| same(current, &event).unwrap_or(false)),
                "pending event changed"
            );
            managed.output_acknowledged = true;
            Ok(())
        })
        .await
    }

    pub(super) async fn replay_events(&self) -> Result<()> {
        let (job, _) = self.load().await?;
        let pending = job
            .broker_checkpoint_v4()
            .map(|cp| {
                cp.events()
                    .iter()
                    .filter(|entry| entry.audit_seq().is_none())
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut acknowledgements = Vec::with_capacity(pending.len());
        for entry in pending {
            let event = serde_json::to_value(entry.event())?;
            let seq = if self.retirement_control_delivery(&job, &entry)? {
                self.client.post_control_event(event).await?
            } else {
                self.client.post_event(event).await?
            };
            acknowledgements.push((entry.event_id(), seq));
        }
        if !acknowledgements.is_empty() {
            self.executor
                .writer
                .acknowledge_events(self.operation, acknowledgements)
                .await?;
        }
        Ok(())
    }

    fn redact_for_outbox(&self, event: serde_json::Value) -> Result<crate::model::Event> {
        // Freeze redaction before the outbox entry. POST uses exactly these
        // bytes/UUID/timestamp again after a lost HTTP or writer ACK.
        let event: crate::model::Event =
            serde_json::from_value(self.client.redactor.value(&event))?;
        event.validate()?;
        Ok(event)
    }

    pub(super) async fn emit_event(&self, event: crate::model::Event) -> Result<()> {
        let event = self.redact_for_outbox(serde_json::to_value(event)?)?;
        self.executor
            .writer
            .enqueue_event(self.operation, event.clone())
            .await?;
        let seq = self
            .client
            .post_event(serde_json::to_value(&event)?)
            .await?;
        self.executor
            .writer
            .acknowledge_event(self.operation, Uuid::parse_str(&event.id)?, seq)
            .await?;
        Ok(())
    }

    pub(super) async fn fact_event(&self, kind: &str, data: &impl Serialize) -> Result<()> {
        let (_, draft) = self.load().await?;
        let event = crate::runner::event(&draft.run.plan, kind, serde_json::to_value(data)?);
        self.emit_event(serde_json::from_value(event)?).await
    }

    pub(super) async fn final_scan(&self, runtime: &ManagedRuntime) -> Result<BatchStep<usize>> {
        self.stage(LocalFailurePhaseV1::FinalScan);
        let (job, draft) = self.load().await?;
        if let Some(checkpoint) = job.broker_checkpoint_v4() {
            for entry in checkpoint.events().iter().rev() {
                if entry.event().kind == "broker.batch.scan" {
                    let facts: ScanFactsV1 = serde_json::from_value(entry.event().data.clone())?;
                    ensure!(
                        facts.version == 1 && facts.worker_id == draft.worker()?.worker_id,
                        "scan result context differs"
                    );
                    return Ok(ControlFlow::Continue(facts.changed_files));
                }
            }
        }
        let receipt = self.admission_receipt().await?;
        // A final scan whose transport bytes were lost can be repeated under a
        // new helper identity. Import, worker and existing claims are untouched.
        let completion = continue_step!(
            self.helper(
                runtime,
                HelperPurposeV1::FinalScan,
                HelperPermissionV1::Admission { receipt },
                Some(Uuid::new_v4()),
            )
            .await?
        );
        let manifest = completion
            .manifest
            .context("final scan bytes are unavailable")?;
        let checkpoint = draft
            .run
            .plan
            .managed
            .as_ref()
            .context("source manifest is missing")?;
        let changes = workspace::diff_manifests(&checkpoint.before, &manifest);
        let changed_files = changes.len();
        let mut facts = ScanFactsV1 {
            version: 1,
            worker_id: draft.worker()?.worker_id.clone(),
            helper_id: completion.policy.helper_id,
            manifest_digest: content_hash("ai-guard:workspace-manifest:v1", &manifest)?,
            changed_files,
            truncated: changed_files > 200,
            changes: changes
                .into_iter()
                .take(200)
                .map(|change| ChangeFactsV1 {
                    path: change.path,
                    change: match change.change {
                        "created" => ChangeKindV1::Created,
                        "deleted" => ChangeKindV1::Deleted,
                        _ => ChangeKindV1::Modified,
                    },
                    before: change.before,
                    after: change.after,
                })
                .collect(),
        };
        // Paths may themselves be long. Bound after redaction as well as by
        // count; the total count and the explicit truncated flag stay true.
        while serde_json::to_vec(&self.client.redactor.value(&serde_json::to_value(&facts)?))?.len()
            > 110 * 1024
        {
            ensure!(
                facts.changes.pop().is_some(),
                "scan metadata exceeds its bound"
            );
            facts.truncated = true;
        }
        self.fact_event("broker.batch.scan", &facts).await?;
        Ok(ControlFlow::Continue(changed_files))
    }

    pub(super) async fn export(
        &self,
        target: ValidatedExportTarget,
    ) -> Result<BatchStep<ExportCompletion>> {
        self.stage(LocalFailurePhaseV1::Export);
        self.replay_events().await?;
        self.reconcile_pending().await?;
        self.generation_current()?;
        let (job, draft) = self.load().await?;
        if let Some(checkpoint) = job.broker_checkpoint_v4() {
            for entry in checkpoint.events().iter().rev() {
                if entry.event().kind == "broker.batch.export" {
                    let facts: ExportFactsV1 = serde_json::from_value(entry.event().data.clone())?;
                    if facts.request_id == target.request_id() {
                        ensure!(
                            facts.version == 1
                                && facts.destination_ref == target.destination_ref()
                                && facts.destination_intent_hash
                                    == target.destination_intent_hash(),
                            "export request conflicts with its result"
                        );
                        return Ok(ControlFlow::Continue(ExportCompletion {
                            request_id: facts.request_id,
                            manifest_digest: facts.manifest_digest,
                            files: facts.files,
                        }));
                    }
                }
            }
        }
        let worker = draft.worker()?.clone();
        ensure!(
            worker.state == WorkerStateV1::Terminal && !worker.quota_held,
            "export requires an acknowledged runtime terminal"
        );
        let checkpoint = draft
            .run
            .plan
            .managed
            .as_ref()
            .context("managed preparation is missing")?;
        let runtime = managed::broker_runtime(
            checkpoint,
            self.context.runtime(),
            draft.run.options().allow_rootful_runtime,
            self.launch_profile.as_ref(),
        )
        .await?;
        let request = ExportRequestV1 {
            version: 1,
            operation_id: target.request_id().to_string(),
            worker_id: worker.worker_id,
            attempt_id: worker.attempt.attempt_id,
            destination_ref: target.destination_ref().into(),
            destination_intent_hash: target.destination_intent_hash().into(),
        };
        let (response, _) = self
            .rpc(Operation::Export {
                request,
                response: None,
            })
            .await?;
        let Operation::Export {
            response: Some(receipt),
            ..
        } = response
        else {
            return Err(anyhow!("export permission acknowledgement is missing"));
        };
        let permission = HelperPermissionV1::Export {
            receipt,
            destination_ref: target.destination_ref().into(),
            destination_intent_hash: target.destination_intent_hash().into(),
        };
        let completion = continue_step!(
            self.helper(
                &runtime,
                HelperPurposeV1::Export,
                permission,
                Some(target.request_id()),
            )
            .boxed()
            .await?
        );
        let bytes = completion
            .bytes
            .context("export transport bytes are unavailable; destination was not recreated")?;
        self.publish_export(target, bytes)
            .await
            .map(ControlFlow::Continue)
    }

    pub(super) async fn publish_export(
        &self,
        target: ValidatedExportTarget,
        bytes: Vec<u8>,
    ) -> Result<ExportCompletion> {
        let (_, draft) = self.load().await?;
        self.generation_current()?;
        let name = target
            .destination()
            .file_name()
            .context("export destination has no component")?
            .to_os_string();
        let request_id = target.request_id();
        let destination_ref = target.destination_ref().to_owned();
        let destination_intent_hash = target.destination_intent_hash().to_owned();
        let report = tokio::task::spawn_blocking(move || {
            target
                .parent()
                .extract_with_report(bytes.as_slice(), &name, Limits::default())
        })
        .await??;
        let manifest_digest = content_hash("ai-guard:workspace-manifest:v1", &report.manifest)?;
        // Keep the closed export V1 fact and IPC acknowledgement unchanged.
        // This separate descriptive event extends the generic audit stream;
        // its absence in historical exports means unknown, never zero.
        let policy = ExportPolicyFactsV1 {
            version: 1,
            request_id,
            manifest_digest: manifest_digest.clone(),
            omitted_entries: report.omitted_entries,
        };
        let files = report.manifest.len();
        let result = ExportFactsV1 {
            version: 1,
            request_id,
            destination_ref,
            destination_intent_hash,
            manifest_digest: manifest_digest.clone(),
            files,
        };
        let event =
            |kind, data| self.redact_for_outbox(crate::runner::event(&draft.run.plan, kind, data));
        let events = vec![
            event("workspace.export.policy", serde_json::to_value(policy)?)?,
            event("broker.batch.export", serde_json::to_value(result)?)?,
        ];
        // Publish both observations in one revision before either POST so a
        // lost response can replay the same audit facts and export receipt.
        self.executor
            .writer
            .enqueue_events(self.operation, events)
            .await?;
        self.replay_events().await?;
        Ok(ExportCompletion {
            request_id,
            manifest_digest,
            files,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ChangeKindV1 {
    Created,
    Modified,
    Deleted,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeFactsV1 {
    path: String,
    change: ChangeKindV1,
    before: Option<workspace::Fingerprint>,
    after: Option<workspace::Fingerprint>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScanFactsV1 {
    version: u32,
    worker_id: String,
    helper_id: String,
    manifest_digest: String,
    changed_files: usize,
    truncated: bool,
    changes: Vec<ChangeFactsV1>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportFactsV1 {
    version: u32,
    request_id: Uuid,
    destination_ref: String,
    destination_intent_hash: String,
    manifest_digest: String,
    files: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportPolicyFactsV1 {
    version: u32,
    request_id: Uuid,
    manifest_digest: String,
    omitted_entries: u64,
}

#[cfg(test)]
mod export_report_compatibility_tests {
    use super::{ExportFactsV1, ExportPolicyFactsV1};

    const LEGACY: &str = r#"{"version":1,"request_id":"00000000-0000-4000-8000-000000000001","destination_ref":"synthetic-export","destination_intent_hash":"synthetic-intent","manifest_digest":"synthetic-manifest","files":2}"#;

    #[test]
    fn legacy_export_fact_keeps_unknown_omissions_and_identical_bytes() {
        let facts: ExportFactsV1 = serde_json::from_str(LEGACY).unwrap();
        // Rewriting a historical fact with a fabricated zero would change the
        // retained event bytes and its hash during reconciliation.
        assert_eq!(serde_json::to_string(&facts).unwrap(), LEGACY);
    }

    #[test]
    fn observed_export_omissions_survive_fact_serialization() {
        for observed in [0, 3] {
            let legacy: ExportFactsV1 = serde_json::from_str(LEGACY).unwrap();
            let facts = ExportPolicyFactsV1 {
                version: 1,
                request_id: legacy.request_id,
                manifest_digest: legacy.manifest_digest,
                omitted_entries: observed,
            };
            let bytes = serde_json::to_vec(&facts).unwrap();
            let replay: ExportPolicyFactsV1 = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(replay.omitted_entries, observed);
            assert_eq!(serde_json::to_vec(&replay).unwrap(), bytes);
        }
    }
}
