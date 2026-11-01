# P1 — factorisation des bytes canoniques déjà validés

1 novembre 2026, lecture seule sur HEAD `f6f93b3c1f2389c5589b8dec07bf9864956437c4`. Aucun Cargo/test, fichier H10 ou opération moteur exécuté. Le gel S7 de `src/broker/journal/tests.rs` reste `a9db5d10801144bafad2e24689b5a34d46b7499ece523ee531647f015ed8dde5`. La présente note privée est l'unique écriture de ce sous-travail.

## Hypothèse vérifiée dans le contrôle de flux

Dans `src/broker/checkpoint.rs:214-232`, `BrokerCheckpointV2::canonical_bytes` appelle `validate_shape`, qui appelle déjà `canonical(self)` pour la limite 32 Mio, puis sérialise encore `self` et refait cette même limite. Chaque appel public réussi sérialise donc deux fois le V2 complet, en plus des sérialisations nécessaires des enfants pour leurs validations.

Même structure dans `src/broker/lifecycle.rs:298-327` (V3) et `src/broker/outbox.rs:300-322` (V4) : leur `validate_shape` sérialise le checkpoint complet pour la limite, puis `canonical_bytes` le sérialise à nouveau. Le V3 valide en outre le V2 par son API canonique, et le V4 valide le V3 de la même manière. Cette répétition existe réellement dans la source ; son coût et sa part dans H10 ne sont pas encore établis par cette note.

Le `canonical` commun (`src/broker/journal.rs:40-58`) produit un `serde_json::Value`, trie récursivement les objets par BTreeMap, puis appelle `serde_json::to_vec`. Chaque répétition parcourt les données et alloue ces formes, indépendamment des bytes obtenus au passage précédent. Aucun nombre d'allocations ni gain de temps n'est mesuré ici.

## Factorisation ciblée proposée — non implémentée

Dans chacun des trois impls, déplacer le corps actuel de `validate_shape` dans une fonction **privée** `validated_canonical_bytes(&self) -> JournalResult<Vec<u8>>`. Conserver les mêmes tests dans le même ordre ; à l'endroit précis de l'actuel `canonical(self)` final, garder ses bytes, contrôler la même taille, puis les retourner.

- `validate_shape` devient uniquement `self.validated_canonical_bytes().map(drop)` : tous ses callers, y compris désérialisation, contexte et transition, continuent d'exécuter tous les contrôles et limites. Aucun validateur partiel n'est introduit.
- `canonical_bytes` retourne ces bytes validés. Pour V2, conserver son second contrôle numérique `bytes.len() > MAX_BYTES` si Root préfère maintenir littéralement ce contrôle public ; il ne nécessite plus une seconde sérialisation. Pour V3/V4, il n'existe actuellement aucun contrôle supplémentaire après `validate_shape`, seulement la seconde sérialisation.
- Ne changer ni champs, dérivations Serde, wire, domaines de hash, limites, erreurs, APIs publiques, formats V1/V2/V3/V4, golden ou ordre de validation. Pas de stockage du Vec dans l'objet, cache, verrou, intériorité mutable, parseur alternatif ou sérialisation manuelle des enfants.

L'ordre exact à préserver est :

| Type | Séquence avant retour des bytes |
|---|---|
| V2 | version2 → runner.canonical_bytes → supervision.validate_shape → canonical(self) → cap32Mio ; puis éventuel deuxième contrôle numérique public |
| V3 | version3 → commands.len≤64 → broker.canonical_bytes → chaque command.validate_shape et unicité UUID/compte approvals → approvals≤1 → canonical(commands) et cap64Kio → canonical(self) et cap32Mio |
| V4 | version4 → events.len≤512 → lifecycle.canonical_bytes → chaque entry.validate_shape → validate_command_ids → canonical(events) et cap4Mio → canonical(self) et cap32Mio |

**Piège du court-circuit :** dans V3/V4, le contrôle actuel `A || B` ne sérialise pas le checkpoint complet si la liste dépasse déjà son cap. Le helper ne doit pas calculer `canonical(self)` avant le contrôle A. Deux `if` séquentiels reproduisent cet ordre ; calculer tous les bytes au début le changerait. Les erreurs Serde restent propagées au même point par le `?` existant. Ne pas remplacer les limites de bytes canoniques par des longueurs d'entrée ou des tailles de champs.

Le premier périmètre serait les trois fichiers `broker/{checkpoint,lifecycle,outbox}.rs`. `CheckpointPayloadV1::canonical_bytes` (`src/runner.rs:703-726`) fait déjà une seule sérialisation de soi après version/versions imbriquées/approbation puis limite : il n'a pas le défaut local shape→resérialisation et ne nécessite aucun changement pour ce lot. Les revalidations de ses différentes frontières (`Deserialize`, `from_bytes`, contexte) sont un autre sujet ; ne pas les supprimer dans cette factorisation.

## Sérialisation : conditions et limites de l'équivalence

Les trois checkpoints utilisent `#[derive(Clone, Serialize)]` avec champs privés possédés. L'inspection des types imbriqués du runner, RuntimeAuthority, OwnedContainerSnapshot, événements et DTO de contrôle n'a trouvé aucun `impl Serialize for ...`, `serialize_with`, horloge, Atomic/Cell/Mutex ou FD sérialisé dans ces graphes. Les implémentations personnalisées relevées concernent **Deserialize**, dont les guards de champs inconnus et validations doivent rester intacts. `OutboxEventV1::event_id` est `#[serde(skip)]` ; `RunPlan::credential_source` est également exclu et n'appartient pas au payload canonique du checkpoint.

Le tri récursif de `canonical` maintient l'ordre des objets ; l'ordre des Vec reste significatif et inchangé. Les chaînes, UUID, chemins déclarés et valeurs JSON sont sérialisés depuis des données déjà possédées, sans ouvrir les chemins référencés. Aucun trait Serialize générique ou injection d'objet d'extension ne doit être ajouté par la correction.

En général, retirer une seconde sérialisation n'est pas équivalent pour un Serialize arbitraire qui change d'état, lit une horloge, échoue seulement au second appel ou produit des bytes non déterministes. La proposition repose donc sur les DTO concrets actuels et leur pureté ; elle n'annonce pas une propriété de tout Serialize. Le second contrôle numérique V2 effectué sur les bytes retenus reste identique pour ces DTO déterministes, mais ne constituerait plus un détecteur d'une sérialisation volontairement instable. Si un custom Serialize/interior mutability apparaît dans ce graphe à l'avenir, cette prémisse devra être revue.

## Tests publics déjà pertinents, sans nouvelle exécution ici

- `tests/broker_checkpoint.rs::real_journal_promotes_v1_rev0_to_pending_decision_v2_without_rewriting_rev0` (`:922`) : golden SHA/domain V1 indépendants, V2 roundtrip canonique, unknown top-level/nested/tag/version refusés, puis vraie publication/ACK/reprise. La fixture connue reste distincte des bytes privés H10.
- `tests/broker_lifecycle.rs::lifecycle_wire_context_replay_caps_and_legacy_approval_boundaries` (`:454`) : V3 roundtrip, unknown top-level/nested/null, version, JSON tronqué/trailing refusés, contexte/replay et limite64/65 commandes (`:525-590`, `:697-710`).
- `tests/broker_outbox.rs::v4_survives_command_and_lifecycle_updates_and_direct_v3_downgrade_is_refused` (`:506`) : intégration V4 et préservation des événements/ACKs lors des mises à jour V3, refus de downgrade/CAS historique.
- `tests/broker_outbox.rs::wrong_context_secrets_strict_nested_decision_and_event_cap_are_rejected_without_writes` (`:675`) : 512 acceptés/513 refusés, contrôle de contexte, secrets et unknown nested decision opaques, aucun head avancé sur refus.
- `tests/broker_outbox.rs::five_hundred_events_birth_and_ack_in_single_revisions_with_stable_history` (`:353`) : vraie naissance/ACK/rejeux avec historique stable ; coûteux et à sélectionner selon besoin, pas à relancer par réflexe.
- `tests/broker_journal.rs::{managed_checkpoint_rejects_unknown_runtime_schema_versions,plan_checkpoint_builder_accepts_known_nulls_but_rejects_unknown_nulls}` (`:346`, `:735`) : conservations runner/version/unknown utiles si le périmètre devait atteindre runner ; sinon canaris ciblés facultatifs selon Root/Luna.

L'inspection de ces fichiers n'a pas trouvé de test public explicite au seuil exact des caps canoniques V2/V3/V4 de32Mio, des commandes64Kio ou des événements4Mio. La limite512 événements et64 commandes ne prouve pas ces limites de bytes. Ne pas présenter cette absence comme couverte par le seul roundtrip. Une revue comparant le helper aux branches actuelles doit donc recouper ces caps et leur court-circuit. Si Root ajoute une régression de bytes, construire un cas valide qui n'échoue pas d'abord sur une autre borne ; éviter une matrice géante d'allocations sans besoin.

## Acceptation avant fusion d'une éventuelle correction

1. Attendre la vraie mesure S7 sur copie H10 et la décision Root ; cette note n'autorise aucun changement de source.
2. Geler sur des fixtures synthétiques publiques les bytes/domaines existants pertinents V2/V3/V4, puis confirmer leur identité exacte après factorisation ; un roundtrip avec le même code des deux côtés ne suffit pas seul à prouver la stabilité d'un wire.
3. Comparer les erreurs publiques et leur priorité pour version/compte/shape/ID/caps/unknown ; conserver les mêmes checks et caps, notamment le court-circuit liste avant checkpoint. Aucun test ne doit qualifier « rapide=PASS ».
4. Luna exécute les régressions choisies ci-dessus et les checks de projet proportionnés ; aucune campagne moteur/fournisseur/E2E ou release massive n'est induite par ce lot purement logiciel.
5. Si une mesure après correction est autorisée, comparer séparément validation mémoire et load/status de la même copie de records, mêmes hooks et même commande/version/build. Compteurs de file-sync/dir-sync/lecture attendus inchangés. Publier quelques observations comme telles, sans percentile extrapolé, causalité H10 ou promesse de gain avant résultat.
6. Revue indépendante du delta des trois fonctions privées et de tous leurs callers ; aucune barrière Full pré/post, CAS/floor, provenance, registre, claim, FS, runtime, durée ou contrôle de secret modifié.

Statuts : PASS pour identification statique du doublon local et proposition bornée ; NOT_RUN pour tests/mesures de cette correction ; aucune causalité H10 ni accélération démontrée ; S7 et G09 conservent leurs propres statuts, cette note ne les change pas.
