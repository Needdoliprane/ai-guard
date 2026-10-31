# H9 — premier refus codé avant claim

Lecture après signal terminal Root. Application PR4 au HEAD `af4a8682edf1bfcb18dcd08113e86f9724e9bcc1` revalidé ; seul harnais d'observation modifié selon Root. Inspection JSON whitelist, stat et SQLite `-readonly`, sans Store open, moteur, Cargo, config, prompt, secret ou erreur libre. Aucune source/QA modifiée par ce reviewer.

## Premier fait codé

SQLite **seq17**, événement **`ed0b84a8-7c4d-48a1-bdfe-13710373b3dc`**, `broker.batch.failure`, reçu20:11:08.973Z :

| Champ | Valeur exacte |
|---|---|
| phase / broker_step | `worker` / `readiness_before_claim` |
| enforcer.stage / reason | **`before_claim` / `slot_phase_mismatch`** |
| Arm ID | `10f5969c-eb1c-4866-b376-5bfadef8c8a5` |
| native_sample | **null** : aucune méthode/boot/current d'un check natif d'erreur |
| original_deadline_ns_decimal | `347579793810375` |
| runtime.stage / runtime_effect | null / `unknown` |

Le refus exact est une phase locale de slot différente de Ready. **DeadlineExpired n'est pas établi.** Ne pas fabriquer un sample à partir d'un probe ultérieur, d'une mtime ou de l'horodatage UTC du Store.

## Journal et liaison des identités

QA `/private/tmp/br-BSL68s`, job `a51e38fa-e3b4-48ee-8553-d96a908f3eaa`, sous `b/journals/g09/broker-journal-v1/2101da84165b61166ba2505ed142e946b6ceac22c516224ca82bf3bb46167e95/jobs/<job>/`. Le pointeur annonce head35/hash `520d16167e9f40d619c19beace617a1bf2c84dddbab6182f45de0cb0759e6679` ; projection des fichiers conservés, sans nouvelle ouverture applicative de leur chaîne.

- 14 opérations supervision sur14 ACKées, zéro pending, aucun WorkerClaim.
- WorkerArm ACK33/seq16, même Arm ID que le diagnostic ; timeout5s, sample Arm `347574793810375` ns, deadline ci-dessus, Darwin/boot `df920bcd-1409-46e1-844a-47ba7ceb0e6c`, observed_utc null.
- Première FailureObserved rev34 : commande `e74772be-1fac-44d1-814b-ab2c0b3d3176`, worker/unavailable.
- Naissance du diagnostic en **rev35 V4**, même Event.id, input_hash `74e26b3cc693801b91af7fa2953ce02f1bdc74d42d49e82d3e5166ef41b01df0`, **audit_seq null**. Le POST est pourtant confirmé dans SQLite seq17 : ACK local pending à la coupure du harnais après GET durable. Ce n'est pas une perte d'événement ni une raison de réémettre start ; la reprise doit conserver cet UUID.
- Worker `afc09f4f-99a1-4b10-af44-c8b9bcf64681`, attempt `edda1573-e814-462b-91c6-c45bca8d7dfa`, Created/revision4/quota retenu, zéro allocation. CID `44788e90678f0e2e614e0a96ae5fa5be12124e4fe5195123b975cbe761a06c4f` ; inspection ciblée Luna conserve Created et aucun StartedAt/FinishedAt. Le code0 d'un Created ne vaut pas terminal.

## Interprétation bornée

La source `claim_snapshot_failure` (`enforcer/client.rs:1008–1023`) teste acknowledged, prepared_once, puis phase. Le diagnostic établit donc acknowledged=true et prepared_once=false avant phase!=Ready. Les contrôles ultérieurs du snapshot (génération/fraîcheur/stop), et le sample natif (`732–769`), ne sont pas attestés par ce motif. Le contrôle santé et le verrou local ont été franchis.

Le contexte broker montre que Register puis LoadAfterRegistration ont terminé ; ensuite le check Ready a refusé (`executor/helpers.rs:97–144`). Register peut retourner un slot usable dont la phase n'est plus Ready (`enforcer/server.rs:361–370`) ; son ACK ne donne pas une permission de claim. Le garde-fou a correctement bloqué le lancement et retenu le quota.

La phase exacte observée et son premier motif de changement ne sont pas dans cet événement. Expiration de l'Arm, rejet de clock, arrêt demandé ou autre passage à un état nonReady restent à distinguer ; aucun correctif causal de ces mécanismes n'est démontré ici. Le délai mural entre rev32 et33 est9,195159974s, mais **ce n'est pas un sample natif d'échec**, et il n'est pas utilisé pour annoncer une expiration.

## Suite ciblée proposée

Ne pas rendre admissible une phase nonReady, retenter claim, renouveler l'Arm5s ou relâcher freshness/génération. Le lot concret encore nécessaire est diagnostique : sur ce même refus seulement, copier les champs déjà lus sous verrou (phase du slot et horloge retenue, avec provenance explicite `slot_snapshot`) dans des métadonnées fermées. Conserver `native_sample=null` quand le check n'en a pas pris ; aucun nouveau sample/RPC/IO sur le chemin de succès. Un test natif borné doit montrer qu'un slot passé nonReady entre Register et claim produit ce diagnostic, **zéro requête de claim/start**, et le même code public. Cela localiserait le passage d'état avant de choisir une correction de comportement ; ce rapport n'autorise aucune édition ou nouvelle tentative.

Le harnais échoue cette fois sur le diagnostic durable, avant le SIGKILL prévu : RC101,232,70s (`03-h9-runtime.log:8–16`). La défaillance codée est établie ; la cause du changement de phase reste inconnue. Gardiens/volumes/QA conservés, D5 ouvert. Autonomie à échéance et reprise après SIGKILL Running : **NOT_RUN dans H9**, aucun gate fermé.
