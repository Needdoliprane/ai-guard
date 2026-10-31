# Revue indépendante — diagnostics de pré-démarrage

Périmètre : lecture seule du worktree `broker-service`, branche `codex/broker-service`. Aucun Cargo, moteur, fournisseur, secret, service ou fichier applicatif modifié par le relecteur. Revue ciblée du lot diagnostic ; aucune qualification G09 ou production. Les deux essais Docker H4/H5 restent **FAIL avant WorkerRunning**, sans injection de panne ; leur cause n'est pas établie par ce lot. Aucun troisième essai Docker exécuté ou autorisé ici.

## Conclusion source D2 et delta D3 figés

**Aucun P0/P1 reproductible identifié dans ce périmètre.** La lecture WIP a été complétée sur les gels D1 puis D2. D2 déplace le module de tests du client, remplace le sample d'erreur par `Option<Box<HostClockSampleV1>>` et limite deux getters runtime à `cfg(test)` ; JSON et contrôles de succès identiques. D3 ne modifie que `tests/broker_runtime.rs:665` : deux `if` imbriqués deviennent une let-chain, conditions et corps conservés. Le manifeste D2→D3 confirme ce fichier seul ; tous les SHA applicatifs D2 ci-dessous sont inchangés. Conclusion source indépendante ; validations Luna séparées.

- **Autorité et codes préservés.** `src/broker/enforcer/client.rs:763` consomme toujours l'unique `FreshStartPermission` et contrôle Arm, worker, attempt, namespace, subject, génération, emitter et claim avant le RPC Prepare. Le wrapper public (`:758`) conserve les codes publics via `wire_code` (`enforcer/diagnostics.rs:76`). Une erreur de transport Prepare reste `prepare_outcome_unknown`, distincte d'un refus distant explicite ; aucune conclusion de non-exécution n'en est déduite. Les réponses restent liées à l'Arm et au hash de registration. `SpawnGuard` (`client.rs:909`) conserve génération, health, cible, Ready/prepared, freshness 750 ms, stop, identité/monotonie d'horloge et deadline originale. Ni réarmement ni durée supplémentaire.
- **Dernier garde adjacent au spawn.** `src/runtime.rs:1132` prépare les closures avant le garde ; `:1143–1145` fait le contrôle puis `command.spawn()`, sans nouvelle attente, IO, préparation ou callback sur succès entre les deux. Les contextes sont construits seulement sur erreur. Les erreurs natives restent accessibles par `anyhow::downcast_ref`, même sous le contexte runtime.
- **Métadonnées bornées, sans texte arbitraire.** `enforcer/diagnostics.rs:49` est Clone/Serialize/Error, sans Deserialize ; Arm UUID et deadline viennent de l'enregistrement validé, le sample est celui du contrôle existant, absent avant sa lecture, jamais prétendu horodatage exact de l'erreur. `runtime/prestart.rs:24` ne conserve qu'un CID de 64 caractères hexadécimaux minuscules ; toute autre valeur devient `None`. Display/Debug sont opaques. `executor/diagnostics.rs:25` ne copie que les deux types connus par downcast ; une erreur générique ne produit aucun diagnostic inventé. Le payload v1 conserve `runtime_effect=unknown` ; il n'accorde aucune autorité, terminalité, preuve d'absence, fencing ou libération de quota.
- **Événement via le chemin durable existant.** `executor/journal.rs:123` tente d'abord l'ancien `FailureObserved/Unavailable`, puis `broker.batch.failure` pour les types reconnus. `results.rs:277` masque avant validation/persistance, enqueue V4 avant HTTP, puis ACK exact UUID/séquence ; une perte HTTP/ACK garde la sémantique pending existante. L'enqueue peut échouer avant durabilité, notamment si le strict `load` de `fact_event` échoue : le message annoncé est alors « delivery incomplete », sans prétendre l'événement absent ou livré. Ce diagnostic best effort ne déclenche ni retry de start ni nouvel état terminal. Le wire IPC, `LocalFailureV1`, les DTO Status, les formats persistés et les dépendances restent inchangés.

## Preuves et limites de validation

Relecture des tests écrits : contexte/downcasts et CID invalide, erreur arbitraire ignorée, sérialisation sans texte privé, distinction refus/Unknown et codes publics, classification clock, prédicat final Ready/prepared/génération/fraîcheur ; fixture Rust `runtime_prearmed` conservant l'erreur originale sous les contextes runtime. Ces tests ne prouvent ni un démarrage Docker réussi ni la cause des deux échecs réels, et ne simulent pas une panne électrique. Le chemin de livraison V4 est réutilisé ; pas de nouvelle campagne moteur/outbox exécutée par le relecteur.

Logs Luna lus sans relancer de commande de validation :

- **D1 : PASS 8 fonctions uniques** = 5 diagnostics + 1 contexte prestart + 1 prédicat native + 1 fixture Rust `prearmed_stop_interrupts_pending_started_callback_without_global_ctrl_c` (15,79 s). Clippy **FAIL**, RC101, trois points mécaniques conservés dans l'historique. Preuves : `/private/tmp/ai-guard-d1-software.3ipHYm`.
- **D2 : PASS 7 unités rejouées**, RC0, et fmt RC0 ; Clippy **FAIL**, RC101, seulement `collapsible_if` du harnais. La fixture D1 est conservée ; le total reste **8**, ni 15 ni les 33 STOP-only historiques. Preuves : `/private/tmp/ai-guard-d2-software.DkH0QN`.
- **D3 : PASS fmt, Clippy all-targets strict et build release**, trois RC0. Lecture du delta let-chain équivalent et des logs ; 158 fichiers vérifiés OK avant/après, relevés identiques, deux binaires archivés vérifiés OK. Preuves : `/private/tmp/ai-guard-d3-final.UPraQc`.

**NOT_RUN : émission du nouveau diagnostic dans un scénario moteur réel.** Les huit tests logiciels prouvent propagation typée et masquage selon leurs assertions ; le raccordement à l'outbox est relu en source. Ils ne prouvent pas une livraison réelle `broker.batch.failure`, une causalité H4/H5, ni la recette de panne broker. Une expiration de freshness 750 ms après Prepare reste une hypothèse, sans mesure établissant la cause. **G09 reste FAIL** dans ce contexte ; aucune qualification production.

## Sources figées relues — SHA-256

```text
f48e76fe93756961aab4965a959309158f319d7735ddd25d7e515aa758892ba8 src/broker/enforcer.rs
40cd3cde67a88d23f4c9d450d684e5b4c273af44b857a98b4fded6ebf40c9c49 src/broker/enforcer/client.rs
ec9817f250b58a76dde5decee41afe902985cdc0c01210ab50a2b6ad2d01c988 src/broker/enforcer/diagnostics.rs
d730043a1562a94c117094a49dfd98cd4f48a251e2b84fdbfe9fa06620f4894a src/runtime.rs
8617d24b52aa3972227fc419cc87e79a63336fe6d9d8b886347ee2fb661c91bd src/runtime/prestart.rs
da86a87080949cad68bbc6cc373d17f76c1fa1ec2fbb7a8ed65ef2348bea7db1 tests/runtime_prearmed.rs
7bad7f8dc1ed5b9e7f19b09786ae7fc21158839ae0d0d7e23a906ad701dfb2ab src/runner/broker/executor.rs
47ccfc4ad5ee9d903ca2290863541bddb4cb86b7e0630dce148094405f405a19 src/runner/broker/executor/helpers.rs
3f402f0201d4e94f9e7c0f05b2896ff10fd47a20cb4a0a56255689644f8a1beb src/runner/broker/executor/journal.rs
cec99aec3a735f222b7e3fb6740c6d728f8181b673ae963b1bdf6ffb2ff5fbb2 src/runner/broker/executor/diagnostics.rs
c9564b9abb342a0eca088b925b568dbc9582292ef1cf6342f276bc57669a6d3f src/runner/broker/executor/results.rs (chemin existant inchangé)
70681753f75d8b3b9636f9377d0ca29beaab2a7b3f03f4d8725deb4132cbc73d tests/broker_runtime.rs (D3, let-chain seulement)
```
