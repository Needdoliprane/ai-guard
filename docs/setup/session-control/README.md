# Prepare a session and send an instruction

[Français](README.fr.md)

## Dashboard with owned sources

Use a dashboard and gateway configured for `owned_sources`. Sign in with a named account and select a personal context. Ask the operator for the opaque launch target identifier and its current revision. The context must grant each action independently. [Owned sources](../identity/OWNED_SOURCES.md)

1. Open **Nouvelle session IA**, enter the published target and revision, then select **Préparer le lancement**. Review the retained preparation, its expiry and its audit receipt. Preparing leaves the worker and quota untouched.
2. Select **Lancer la session** to commit that preparation. The acceptance receipt identifies the session and its named grant. Check the broker and runtime observations for admission and startup.
3. If the launch result is unknown, use **Réessayer le même lancement** from the same context. This reuses the operation and preparation already submitted. The control can return an existing receipt after the preparation expires. A refusal during reconciliation leaves the original outcome unknown.
4. **Envoyer une consigne** requires a current conversation binding for the exact session, worker, attempt and input epoch. Review the destination before sending. Read the retained receipt to distinguish recorded, transmitted, acknowledged and unknown delivery. Acknowledgment concerns the provider protocol. Observe any resulting action separately.

To return to an existing session, open **Ressources** and choose **Consulter le canal** on a session you own. The dashboard reads the binding from the gateway. Select **Utiliser ce canal** when the channel is active and has time remaining. **Relire le canal** checks the current status. If the input epoch changes, select the new channel explicitly.

If your context grants sending and has no Resources view, open **Sessions** and enter the exact identifier of a session you own. The gateway still checks the context, tenant and session ownership.

The current implementation covers the durable control-plane operations and the dashboard views. The broker composition exercises a controlled stdio child and a real owned Store in tests. The bridge from supervised production admission and the end-to-end runtime and provider paths remain open. [Session contract](../../engineering/SESSION_CONTROL.md) · [Qualification gates](../../engineering/PRODUCTION_READINESS.md)

## Prepare and confirm on Legacy SQLite

The Legacy CLI uses a local named account and a personal context with `launch_session`. The operator provisions a fixed target for that account and organization. This procedure covers preparation and confirmation. Broker admission and runtime startup remain open for this profile.

### Operator maintenance

1. Use the existing tenant SQLite database at format 10–13. Stop the gateway, brokers and automatic restarts before setting `--confirm-control-stopped`. If needed, install the `human_rights_v2` marker first with `migrate-human-rights` and its own fresh backup. [Human rights migration](../identity/README.md#enable-the-human-rights-capabilities)
2. Set `tenant_db` and `tenant_id` to the source database path and tenant. Choose an unused `prepare_backup` path in a private directory, then install the separate preparation capability.

   ```sh
   agentsandbox enable-legacy-session-prepare \
     --data "$tenant_db" --tenant-id "$tenant_id" \
     --backup "$prepare_backup" --confirm-control-stopped
   ```

   Use the same audit capacity settings for both maintenance commands. The defaults match the gateway at 100,000 events and 256 MiB. Pass `--max-events` or `--audit-max-bytes` when the gateway configures a different limit, with the latter specified in bytes. If the gateway uses a private `--audit-capacity-file`, pass that file. With a tenant registry, save the tenant’s `audit_capacity_policy` object on its own in a private JSON file and pass that file. An incompatible ceiling or policy is refused before SQLite is opened. When a valid policy leaves the ordinary budget full, the write is refused to preserve the STOP and identity reserves.

   Keep the verified backup and the JSON report. `from_version` and `to_version` both report the existing store format. `backup_path`, `genesis_seq` and `genesis_hash` identify the backup and marker. A second installation is refused. If the report is lost, preserve the backup and inspect the tenant audit for `control.legacy_session_prepare.genesis` before retrying. The current migration to `owned_sources` refuses a database carrying this marker. [Human source maintenance](../human-sources-maintenance/README.md)

3. With the control still stopped, provision the target from a private JSON file referenced by `target_file`. Use `expected_revision: 0` and catalogue revision `1` for a new target. The target records the permitted local accounts, the broker incarnation and grant revision, and a fixed `generic` launch specification using `fixed_sandbox_v2` and `network: none` with no provider credential. [Target format](../../../src/control/session_control.rs) · [Broker grants](../workloads/README.md#prepare-a-broker-grant-or-revocation-from-the-cli)

   ```sh
   agentsandbox human provision-legacy-session-target \
     --data "$tenant_db" --tenant-id "$tenant_id" \
     --target-file "$target_file" --confirm-control-stopped
   ```

   The JSON receipt returns `target_id`, `target_revision`, `audit_seq` and `audit_hash`. Reusing the same target file and revision returns the same audit receipt with `replayed: true`. Give the named user the target ID and revision. Resume the Legacy gateway for the human requests below.

### Named request and replay

Use the Legacy gateway origin in `gateway` and your private local `credential_file`. Keep the credential and request files under a private directory in regular files owned by the invoking account with mode `0600`. HTTP is available on loopback. For HTTPS with a private CA, pass `--gateway-ca-file` with the approved public CA bundle to each command.

```sh
agentsandbox human identity --gateway "$gateway" --credential-file "$credential_file"
```

Use the ID of the personal context in `context`. Save `prepare_request` as a private JSON file with `version` set to `1`, a new UUID `operation_id`, the operator’s `launch_target_id` and `expected_target_revision`, and an RFC 3339 `expires_at` within the validity of the context and credential.

```sh
agentsandbox human prepare-legacy-session --gateway "$gateway" \
  --credential-file "$credential_file" --context "$context" --request-file "$prepare_request"
```

Review the target, revision, expiry and audit receipt. The preparation leaves the worker and quota untouched and creates no session grant. Save `confirmation_request` as a private JSON file with `version: 1`, a new `operation_id`, and the same `launch_target_id` and `expected_target_revision`. Copy the original preparation request’s `operation_id` to `preparation_operation_id` and the receipt’s `input_hash` to `preparation_input_hash`.

```sh
agentsandbox human confirm-legacy-session --gateway "$gateway" \
  --credential-file "$credential_file" --context "$context" --request-file "$confirmation_request"
```

The confirmation receipt records `binding_ref`, `session_id`, `audit_seq` and `audit_hash`. It attests to a retained confirmation only. The broker admission, grant, worker reservation and start claim have yet to occur.

After a lost response, keep the request file and replay the same command with the original operation ID and content. A successful replay sets `replayed: true` and returns the original audit receipt. The gateway rechecks the local identity, membership and current target permissions. Use `human identity` to obtain a fresh personal context if needed. Replaying Prepare still requires a valid request expiry. The gateway can return an already committed confirmation after the preparation expires. A conflicting request or an authorization refusal leaves the original outcome unresolved.

For `migration_required`, check that the preparation marker is installed. For `context_revoked` or `permission_denied`, check the current membership, personal context and target access before retrying. For `conflict`, reread the target revision and the retained request before choosing another operation.

On `9da3f556`, `legacy_session_prepare_capability_is_explicit_and_survives_reopen` and `legacy_local_confirmation_survives_reopen_without_admission` pass on SQLite and the test router. The capacity regressions added after that revision and recovery through a deployed gateway with a real broker are `NOT_RUN`. [Tests](../../../tests/human_rights_migration.rs) · [Session contract](../../engineering/SESSION_CONTROL.md) · [Qualification gates](../../engineering/PRODUCTION_READINESS.md)
