# Qualification ciblée — broker checkpoint V2

Rapport privé de Luna, lot test-only dans le worktree `broker-service`. Aucun code applicatif n’a été modifié par cette agente. Le fichier ajouté/modifié pour les tests est `tests/broker_checkpoint.rs` seulement.

## Résultat final GEL5

- Cible entière `broker_checkpoint` : **PASS 6/6**, profil release; compilation 52,72 s, exécution 17,59 s, durée shell totale 1:11,07. Le résultat couvre les six fonctions, graphe Store compris.
- Sous-cible rapide debug sans le graphe : **PASS 5/5**, 1,84 s. Le graphe n’a pas été répété en debug après sa première exécution de 230,66 s.
- Canaris demandés : **26 PASS**, sans additionner les rejeux de cibles : `broker_journal` 12/12, `supervision_store` 9/9, `journal_sqlite` 1/1, unités `broker::journal::tests` 4/4. Les deux fonctions child marquées ignored ne sont pas ajoutées au compte; `writer_lock_race_child` a été invoquée et a réussi comme enfant de son test parent.
- `cargo fmt --all -- --check` : PASS, RC 0. `cargo clippy --locked --all-targets -- -D warnings` : PASS, RC 0. `cargo build --release --locked` : PASS, RC 0, incrémental 0,12 s.

## Commandes finales et modes

Toutes les commandes Cargo ont utilisé `login:false`, le cache exclusif `CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target` et `CARGO_INCREMENTAL=0` quand indiqué. PATH : `/opt/homebrew/bin:/usr/bin:/bin:/Users/louis/.cargo/bin`.

```sh
PATH=/opt/homebrew/bin:/usr/bin:/bin:/Users/louis/.cargo/bin CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target CARGO_INCREMENTAL=0 cargo test --locked --test broker_checkpoint -- --skip sqlite_store_receipts_cover_all_supervision_operation_variants 2>&1 | /usr/bin/tee /private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel5/expanded-quick-fixed-replay.log
# debug — PASS 5/5, 1 filtré, 1,84 s

PATH=/opt/homebrew/bin:/usr/bin:/bin:/Users/louis/.cargo/bin CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target CARGO_INCREMENTAL=0 cargo test --release --locked --test broker_checkpoint 2>&1 | /usr/bin/tee /private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel5/release-full-target.log
# release — PASS 6/6, aucun ignoré; compile 52,72 s, tests 17,59 s

PATH=/opt/homebrew/bin:/usr/bin:/bin:/Users/louis/.cargo/bin CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target CARGO_INCREMENTAL=0 cargo test --locked --test broker_journal 2>&1 | /usr/bin/tee /private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel5/broker-journal-canary.log
# debug — PASS 12/12, 1,41 s

PATH=/opt/homebrew/bin:/usr/bin:/bin:/Users/louis/.cargo/bin CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target CARGO_INCREMENTAL=0 cargo test --locked --test supervision_store 2>&1 | /usr/bin/tee /private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel5/supervision-store-canary.log
# debug — PASS 9/9, 1,20 s

PATH=/opt/homebrew/bin:/usr/bin:/bin:/Users/louis/.cargo/bin CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target CARGO_INCREMENTAL=0 cargo test --locked --test journal_sqlite 2>&1 | /usr/bin/tee /private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel5/journal-sqlite-canary.log
# debug — PASS 1/1, 0,15 s

PATH=/opt/homebrew/bin:/usr/bin:/bin:/Users/louis/.cargo/bin CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target CARGO_INCREMENTAL=0 cargo test --locked --lib broker::journal::tests 2>&1 | /usr/bin/tee /private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel5/journal-unit-canary.log
# debug — PASS 4, ignorés 2 (child helpers); test parent de course lance effectivement writer_lock_race_child

PATH=/opt/homebrew/bin:/usr/bin:/bin:/Users/louis/.cargo/bin CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target cargo fmt --all -- --check 2>&1 | /usr/bin/tee /private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel5/fmt-expanded-final.log
# PASS, RC 0

PATH=/opt/homebrew/bin:/usr/bin:/bin:/Users/louis/.cargo/bin CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target CARGO_INCREMENTAL=0 cargo clippy --locked --all-targets -- -D warnings 2>&1 | /usr/bin/tee /private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel5/clippy-expanded-final.log
# PASS, RC 0

PATH=/opt/homebrew/bin:/usr/bin:/bin:/Users/louis/.cargo/bin CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target CARGO_INCREMENTAL=0 cargo build --release --locked 2>&1 | /usr/bin/tee /private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel5/release-build.log
# PASS, RC 0, profil release incrémental 0,12 s
```

Le build final et la cible release ont tourné sur le toolchain `1.99.1-aarch64-apple-darwin`. Les binaires ne qualifient que cet hôte/profil; aucune conclusion Linux, worker/runtime ou modèle n’est revendiquée.

## Contrats et preuves couvertes

| Test | Preuve principale |
|---|---|
| `real_journal_promotes_v1_rev0_to_pending_decision_v2_without_rewriting_rev0` | Canari V1 rev0 ancré aux octets/hash natifs; progression V2 pending puis ACK Decision du Store réel; avance à une révision V2 ultérieure; rejeu de l’ancienne CAS rend son reçu historique sans rollback de la tête. Deux clones observent le refus après restauration d’un ancien pointeur `current` valide; le pointeur V2 exact est restauré avant reopen. Refus de namespace divergent, tenant isolé, version/champs/tags inconnus refusés. |
| `approved_v1_runner_cannot_be_downgraded_by_first_v2_revision` | Une révision V1 confirmée `human_approved=true` ne peut pas être effacée par la première V2 false. Rev0 et rev1 restent inchangées après refus. |
| `sqlite_decision_and_admission_ack_follow_pending_journal_revisions` | Decision, Admission et mutations Store/Journal avec pending avant ACK. BCP-01 forme WorkerMutation; BCP-02 cohérence requête/phase Admission; BCP-03 désérialisation stricte imbriquée Arm; BCP-04 résultat Prepare comparé au reducer; BCP-05 unicité worker/attempt/révision ACK. Les candidats forgés sont des négatifs synthétiques dérivés de résultats réels, jamais présentés comme réponses Store émises. |
| `admission_pending_must_equal_approved_current_runner_request` | L’Admission non approuvée liée à une ancienne Decision Initial ne satisfait pas un runner approuvé. |
| `prepared_managed_runner_can_match_a_pending_admission_without_worker` | BCP-06 runner `managed=Some`, binding privée présente, Admission exactement demandée mais pending/refusée, sans worker/helper/owned; quota=0 produit un refus réel `Quota` conservé dans le V2 checkpoint. Un helper neuf `owned=None` avec runner préparatoire bindingSome est rejeté avant ACK; candidats worker et owned pré-ACK refusés séparément. |
| `sqlite_store_receipts_cover_all_supervision_operation_variants` | Parcours cohérent des neuf tags : Decision, Admission, WorkerMutation, Arm, WorkerClaim, HelperClaim, HelperObservation, HelperCleanup, Export. Store SQLite réel et reçus réels pour les mutations, claims, observations, nettoyages et export. Branches permission ImportVerification, FinalScan/Admission et Export après ACK export. Inclut D01 V1 initial→managed valide→None→promotion V2 refusée et conservation byte-for-byte des révisions antérieures. |

Les tests utilisent des répertoires temporaires privés mode 0700; le Store SQLite est réel. Dans le graphe, le namespace vient du vrai Store migré schema3; le scope de journal porte tenant `tenant-checkpoint-test`, gateway `http://127.0.0.1:8787`, broker `broker-test`. Decision, Admission, mutation, claim, observation, cleanup et export s’appuient sur les API Store réelles et leurs reçus persistés.

Aucun moteur/runtime n’a été lancé. L’autorité Docker d’essai (`unix:///private/fixture.sock`), inspections, owned snapshots, observations, clock et effets runtime sont des données synthétiques explicites. La permission Export, FinalScan et ImportVerification est testée contre les liens Store/journal réellement produits. Le cas BCP-05 altère délibérément un candidat depuis un ACK Prepare réel afin de vérifier le refus; ce candidat n’est pas attribué au Store.

## BCP-01 à BCP-06

- **BCP-01 — payload et résultat de WorkerMutation.** Le RED GEL1 montrait que Store refusait `Unknown { reason: "" }` tandis que le Journal acceptait la transition pending. GEL2 compare les transitions attendues au reducer réel et valide la réponse intégrale. BCP-04 couvre aussi un Prepare ACK altéré (state Terminal/quota false) sans modifier le parent ni la tête.
- **BCP-02 — requête Admission exacte.** Le RED GEL1/GEL2 acceptait un runner approuvé avec ancienne Decision Initial ACK et Admission pending non approuvée. Le validateur exige l’égalité avec `runner.current_request`, y compris la phase; refus vérifié.
- **BCP-03 — Serde Arm imbriqué.** Le RED acceptait `extra:null` dans `Arm.owned` et `Arm.owned.observation` par `from_bytes` et `Deserialize`. GEL2 installe le garde strict local; roundtrip valide accepté, les deux mutations rejetées.
- **BCP-04 — ACK de worker exact.** Le RED Journal acceptait une réponse Prepare altérée. La couture gel2 au reducer réel rejette le résultat divergent sans nouvelle révision.
- **BCP-05 — unicité ACK de révision.** Deux Prepare pending contre le même parent sont représentables; un second ACK candidat, operation_id distinct mais même worker/attempt/révision, est rejeté. Le second candidat est synthétique et négatif, pas un reçu Store issu d’une seconde mutation.
- **BCP-06 — runner préparé avant Admission.** RED sur GEL2 pour `managed=Some`, `worker=None`, Admission pending exacte. GEL3 choisit le recoupement `requested` pour le runner préparé; `admitted` demeure requis pour worker/helper/owned/Arm/claims. Test additionnel réel quota-refusal vérifie cette conservation sans Worker/helper/owned.

## Chronologie des diagnostics et rejeux

Les erreurs suivantes sont des erreurs de fixture ou de harnais, séparées des RED comportementaux; elles ne sont pas additionnées au compte final.

1. Le premier lot cible a échoué car `private_tempdir` fournissait un chemin affecté par le symlink `/tmp` sur macOS; `Directory::open_existing` répondait ENOTDIR. `open_journal` canonicalise désormais la racine de test avant ouverture. `green-1.log` conserve le résultat.
2. `green-2.log` : deux tests réussissaient et le test SQLite échouait sur un contexte fixture incohérent. `green-3-target.log` après correction a passé 3/3. Les phases ultérieures ont corrigé la source des namespace/tenant fixtures, sans changement d’oracle produit.
3. Erreur mécanique de compilation du harnais : `WorkerMutationV1` n’expose pas `input_hash()`. Le candidat BCP-05 utilise maintenant `content_hash("ai-guard:worker-mutation:v1", request)`; le premier diagnostic est dans `graph-target.log`.
4. Premier graphe debug : conflit de contexte V1 managé causé par tenant fixture mismatch; correction tenant vers `tenant-checkpoint-test`, puis graphe 1/1 PASS en 230,66 s (`graph-target-after-tenant-fix.log`).
5. Après ajout du contrôle shared journal, un second `Journal::open` a répondu `Busy`, ce qui est son verrou exclusif prévu. Le test a été corrigé en deux clones d’un même Journal. Diagnostic `shared-v2-replay-diagnostic.log`, test ciblé corrigé PASS `shared-v2-replay-diagnostic-after-fix.log`.
6. Un run complet debug lancé immédiatement avant la décision root de l’interrompre avait un échec de fixture `Busy`; quatre tests avaient fini PASS et le graphe tournait encore. À demande root, PID 72875 (cargo 72862) a reçu SIGINT à 00:52. Ce run incomplet n’est pas compté PASS. Trace `gel3/broker-checkpoint-final-target.log`; l’oracle Busy est corrigé et cible complète release finale 6/6 PASS.
7. L’appel `/usr/bin/time` a échoué avant lancement Cargo (`env: no such file or directory`); aucune compilation associée. Les durées ultérieures viennent du builtin `time` zsh et des logs exacts.
8. Clippy GEL4 a d’abord signalé `large_enum_variant` sur les enums publics/wire du module checkpoint. GEL5 applique deux allowances locales motivées, sans changement des neuf variantes ni désactivation générale. Clippy a ensuite signalé deux lint de fixture; le helper test conserve un allowance local avec raison, l’assertion bool utilise `assert!`. Clippy final strict passe.

## Temps debug/release et limites du benchmark

Le graphe release GEL3 a pris 22,39 s, GEL4 16,71 s, GEL5 final 17,59 s. Le premier graphe debug a pris 230,66 s. Le sample debug montrait `Journal::advance_supervised → advance_checkpoint → BrokerCheckpointV2::from_bytes` pendant revalidation/canonicalisation JSON; RSS observé environ 24 MB. La correction GEL4 retire le second parse V2 redondant dans le loader; strict Deserialize, canonical record complet, validation causale, CAS, filesystem, caps et durabilité restent actifs. Le gain est une mesure hôte de ce test ciblé; elle ne qualifie pas le coût sur d’autres plateformes/profils.

**Limite d’archive GEL3/GEL4 :** l’ancien test avait SHA `e0f59bc0ea90f80e3bca46555fc8b7438872d5bfdf56186f3e458d33db72039a` dans les manifestes historiques, mais son source exact n’avait pas été copié avant édition. Une reconstruction privée a été tentée sans toucher au worktree, sans retrouver ce SHA; reconstruction formattée conservée sous `gel4/source/broker_checkpoint.e0-reconstruction-c7f7f4e5.rs`, SHA réel `266965565c5b082c6b57c26ad1356b112050a82ed1680e0830f01fc940742a9e`. Les temps GEL3/GEL4 restent des observations journalisées, mais une reconstruction bit-à-bit de leur test source est **NOT_RUN**. La source exacte GEL5 est archivée et vérifiable.

## Sources, hashes et artefacts

Manifest final avant/après identique : `gel5/release-build-before.sha256` et `gel5/release-build-after.sha256`.

```text
 test  tests/broker_checkpoint.rs       050df934ae05f01290b87129fdff3cf95c788c084204679476f204a8d94ae9cb
 app   src/broker/checkpoint.rs         c300095c30e0a410856f953b2ad4a49addedebf357b33800a696705885e847ff
 app   src/broker/checkpoint/validation.rs 5733a07b6fc34ccdac6512fc366202291a61cb48f428ece3832ec9c486d3cf15
 app   src/broker/mod.rs                fcf105d4a83c416b9df1ffc68c050e1f04265dab53f86b57c69207bec8301aa4
 app   src/broker/journal.rs            f9b912457d294e1504b976347ea388e57b7a1f54cf7285b6c6f1a56b9649dbaa
 app   src/broker/journal/types.rs      8d8a117b3beafc6f548d14db7ae8d3a50b48f21cc5c95a7d5c71c2cbb579178b
 app   src/runner.rs                    f09e82f16296fb53b20656a64cdfa754636dd67a1dc72cc74ab86b6d7aebb3c9
 app   src/audit.rs                     9587b78cb74140d7dd0d4daf0af43605c0626c1aeabb6d02373bdf88025b0fb6
 app   src/audit/control.rs             f13a7d06a6a789aef92f524db337f5e3a1c717080891cb14b3dc73637d267447
```

Artefacts release finaux, copies privées mode 0600 sous `/private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel5/release-artifacts/` : `agentsandbox` SHA `b360af55dc6d6b75dd9d9f9ead2d59eecda0428cbe7f68b5cfa833b0a0911f98`; test executable SHA `e2966caede8524ea7eef8cd8682ea0b0512e82602874a495d2960bfd385d972e`; `libagentsandbox.rlib` SHA `68289f51d52eea893f91a52b7a25f632f71564fb12288d8e7b2ea3ce9042a7c5`. Le répertoire gel5 est 0700. Logs et manifests finaux sont dans `/private/tmp/ai-guard-broker-checkpoint-v2-qualification/gel5/`; GEL3 et GEL4 restent préservés séparément.
