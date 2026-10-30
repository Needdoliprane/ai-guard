# Store supervision v3 — validation ciblée

Date : 2026-10-30. Vérification Rust/SQLite réelle, sans conteneur worker, modèle, fournisseur ni appel API externe. Les tests utilisent des bases SQLite temporaires et les routes Axum en mémoire.

## Résultat

| Statut | Cible | Résultat |
|---|---|---:|
| PASS | `tests/supervision_store.rs` | 9/9 |
| PASS | `tests/supervision_contract.rs` | 5/5 |
| PASS | `tests/worker_registry.rs` | 36/36 |
| PASS | `tests/audit_security.rs` | 3/3 |
| PASS | `tests/audit_integrity.rs` | 12/12 |
| PASS | `tests/storage_security.rs` | 17/17 |
| PASS | `tests/tenant_storage.rs` | 3/3 |
| PASS | `tests/decision_idempotence.rs` | 9/9 |

La commande groupée finale a exécuté 94 tests top-level, tous PASS. Les invocations subprocess affichées sous `storage_security` sont des enfants de son harnais et ne sont pas recomptées. Aucune suite globale ou runtime n’a été exécutée.

Commande groupée :

```sh
CARGO_INCREMENTAL=0 cargo test --locked \
  --test supervision_store \
  --test supervision_contract \
  --test worker_registry \
  --test audit_security \
  --test audit_integrity \
  --test storage_security \
  --test tenant_storage \
  --test decision_idempotence \
  -- --nocapture
```

Log final privé (mode 600) : `/tmp/ai-guard-supervision-store-final-group.nGgTF7/target.log`, SHA-256 `df396ab81cd744afab5c8f68ab108484857e40ca360e960cc88fd594448c46fd`. Manifestes source avant/après dans le même répertoire ; `diff` ne montre aucun drift pendant la commande. Le dossier est en mode 700.

La cible Store a également été exécutée seule après le dernier correctif de diagnostic : 9/9 PASS, log `/tmp/ai-guard-supervision-store-green-v6.5Ef0Gg/target.log`.

## Comportements couverts par les nouveaux tests Store

- Une base schema2 historique migrée vers schema3 conserve historique, quota et reçus, mais exige la maintenance après migration et après réouverture. Les admissions/claims nouveaux et le bind d’un worker legacy `Created` sont refusés sans append.
- Une base neuve migrée au schema3 avant toute activité accepte l’admission supervisée ; deux handles créés avec `Store::sibling()` partagent la lease, les admissions identiques se rejouent de façon idempotente et une divergence de contenu conflicte.
- Une autorité opaque est toujours permise par le contrat legacy v2, mais refusée par `SupervisedAdmissionV1` avant quota.
- L’ouverture indépendante d’un clone SQLite cohérent, sauvegardé après armement et avant claim tandis que l’original reste vivant, est inactive. Le claim clone reçoit `SupervisionInactive` sans nouvel événement/reçu ; l’original émet une fois puis rejoue `AlreadyConsumed`. La réouverture indépendante de l’original est aussi inactive et refuse un nouvel identifiant d’opération.
- Un restore SQLite en ligne dans le même fichier, vers une tête antérieure capturée avant deux commits concurrents via siblings, conserve le même device/inode mais est détecté au premier accès. L’erreur indique le rollback sous le watermark ; le Store et tous ses siblings deviennent unhealthy.
- `ImportVerification` en attente empêche `CreateIssued`. Un mismatch après un succès antérieur, et un succès après un mismatch, empêchent tous deux `CreateIssued` sans append. Les refus persistent après réouverture en maintenance.
- Le chemin HTTP d’une mutation legacy vers une base schema3 historique réouverte renvoie HTTP 503 avec le code `supervision_inactive` et n’ajoute aucun événement ni opération.
- Échec de capacité d’audit : l’armement est refusé avec la tête, l’opération et le quota inchangés. Une divergence de projection latch le Store unhealthy.

## Défauts reproduits avant correction

- Import : une vérification V1 correcte suivie d’une vérification V2 contradictoire sous un autre UUID permettait `CreateIssued`. RED conservé dans `/tmp/ai-guard-supervision-store-red-anysuccess.MEZEB0/target-red.log` (exit 101 attendu).
- Clone pré-claim : après sauvegarde SQLite réelle et fermeture des connexions de sauvegarde, les claims dans l’original et dans le clone renvoyaient tous deux `EmitOnce`. RED conservé dans `/tmp/ai-guard-supervision-store-red-clone.R91wGr/target-red.log` (exit 101 attendu).
- Le premier lot groupé a exposé une régression de diagnostic dans 4 tests `audit_integrity` : un test échouait sur `integrity().unwrap()` (ligne 62), et trois échouaient sur `head().unwrap()` (lignes 34, 46 et 151) avec une lease invalidée avant le diagnostic forensic. Core a corrigé ces lectures sans modifier les assertions. RED conservé dans `/tmp/ai-guard-supervision-store-regressions.wTbhtc/target.log`.

Les erreurs intermédiaires de compilation/fixture étaient distinctes des défauts produit : cap `max_events` trop bas pendant `Prepare`, comparaison d’une tête `i64` comme tuple, et fixture HTTP liée au tenant par défaut. Les corrections limitées au harnais ont été suivies des résultats PASS ci-dessus ; elles ne sont pas qualifiées comme des RED comportementaux.

## Sources vérifiées

Manifestes avant/après de la commande finale : `/tmp/ai-guard-supervision-store-final-group.nGgTF7/source-hashes-before.txt` et `source-hashes-after.txt` (identiques). SHA des sources de production :

- `src/audit.rs` `bba9534c4f37e247d7497cc3961f40c12e9712d5e557e88d6b7970915e2029ca`
- `src/audit/control.rs` `2d80c7855dc3e4ab0622c4a41076607ad008b8a107a683dd48efaac60cc895c1`
- `src/audit/supervision.rs` `e5fe84ede6ae136e4298c6323aa928dd22b485725cb4b024412ef6aa1053600e`
- `src/audit/storage.rs` `ac381df73de99def79aa441853c44ac4ac7b1265d776e2c684e27587490b493c`
- `src/control.rs` `afe40dc8477fbb19be6de784a91c135230d2723fef5135499ae9ba7656657d8f`
- `src/control/supervision.rs` `1a65c3cd5e9bc10cc8ec98df832a073a0fed1f24ae9fdff45a3631aec2f2484c`
- `src/api.rs` `4db5d205e0571a5843e6b92edda61c727c3c14f296d6bbba3631f3d71116e393`
- `tests/supervision_store.rs` `9eaf15f96d47730513e154f1d2962fe88ebc0851c0ee94037801097571eb3537`

## Limites et statuts

- **PASS** : cible Store et régressions ciblées listées ci-dessus.
- **NOT_RUN** : full test suite, Docker/Podman runtime, workers réels, moteurs/modèles/fournisseurs, E2E/browser, fmt/clippy final et build release. Root attend la revue indépendante du code avant les vérifications statiques et l’artefact final.
- Cette validation démontre des contrats SQLite en processus, dont la détection de rollback pendant qu’une lease vivante existe. Elle ne qualifie pas de fencing après redémarrage, reprise autonome, broker/IPC, exécution helper réelle, ni D5.

## Addendum — vérification après sérialisation du diagnostic (2026-10-30)

Le lot initial de 94 tests a été exécuté sur `src/audit.rs` SHA `bba9534c4f37e247d7497cc3961f40c12e9712d5e557e88d6b7970915e2029ca`. Un reviewer a ensuite relevé une course dans l’invalidation diagnostic du backing. Le core a sérialisé `verify_diagnostic_backing` avec le même verrou de lease, dans l’ordre db→lease, sans réactivation de lease ni changement d’autres fichiers applicatifs.

Après ce delta, la commande ciblée a repassé les 24 tests concernés : `supervision_store` 9/9, `audit_integrity` 12/12, `audit_security` 3/3 — tous PASS. Il s’agit d’une vérification chevauchante des 24 mêmes tests, **pas de 24 tests supplémentaires à additionner aux 94**. Les autres 70 tests ont passé sur le code précédent ; ce delta se limite à la sérialisation du diagnostic backing. Aucun stress test ou hook de production n’a été ajouté.

Commande :

```sh
CARGO_INCREMENTAL=0 cargo test --locked \
  --test supervision_store \
  --test audit_integrity \
  --test audit_security \
  -- --nocapture
```

Log privé : `/tmp/ai-guard-supervision-postmutex.sxvTg8/target.log`, SHA-256 `58265163df73c4100477dff3a275c9c803c808e403e773833107640fc216b400`. Les manifestes avant/après sont identiques. Le SHA actuel de `src/audit.rs` est `112b1044b2be813f47a3ab6adcfda94942fee05702bbb04d57313b99dfe877a3`; les autres fichiers suivent le manifeste `source-hashes-after.txt` du répertoire de log.

La restauration SQLite en place à même device/inode reste détectée par le Store et tous ses siblings deviennent invalides ; la cible de régression rollback est restée PASS après le delta.
