# Historical relay event reader — verified software evidence

12 November 2026, 21:24 UTC. [Exact results and identities](TESTS.json)

The targeted reader correction passes **13 unique parents, 0 failed and 0 ignored**, followed by `cargo check --all-targets` RC 0, on the frozen source below. The nine admission parents are included and were rerun within these thirteen; the earlier nine-test campaign is a separate result, not an additional count. This page records verified software behavior without asserting integration or publication of the reader change.

## Change and observed behavior

In `src/audit/relay.rs`, `kind LIKE 'control.relay.%'` becomes `kind GLOB 'control.relay.*'`. The case-sensitive selector retains the exact canonical prefix. Existing strict parsers still reject an unknown canonical suffix. No kind, payload, hash, schema or projection is normalized or rewritten.

The historical regression commits `Control.relay.x`, `CONTROL.relay.x`, `CoNtRoL.ReLaY.x` and `Fixture.MixedCase` through the real historical ordinary writer. Direct relay validation then succeeds; the existing Store reopens healthy with valid integrity. The head sequence/hash, values of all eleven event columns and the three initially empty relay projection counts remain unchanged. This is a stored-value comparison, not physical SQLite/WAL file identity.

The canonical canary separately submits `control.relay.x` and `control.relay.X` to the real validator inside open checked transactions. Both return `unknown authoritative relay event`; actual rollback preserves the snapshots and a healthy reopen. **No invalid canonical row is committed.**

| Selected coverage | Unique parents |
|---|---:|
| Historical compatibility and canonical validator/rollback | 2 |
| Admission regressions, helper units and D5/relay canaries, rerun on this source | 9 |
| Typed relay revocation/replay and canonical-time neighbors | 2 |
| **Total executed on this source** | **13** |

The typed neighbors use real Store/SQLite operations. Their runtime facts are synthetic; they do not launch a worker. Full selectors and each one-test oracle are in [TESTS.json](TESTS.json).

## Source and execution evidence

The source contains **2,605 regular files**: Git `702f35ba205d0e67cfb3c5b6d5dc8fcc77eacb6b` plus the exact reader and historical-test postimages. Of the 2,604 Git files, 2,603 remain unchanged; one reader file is replaced and one test file is added. Source archive SHA-256: `94e6e30dc9e57d646ccaf41cb7e9d568148bc9181044f180656889751be7e24b`. Reader postimage: `71cf8042ff937d1b51d99379cac25e604b88af0cff420180d4856d1b6a9d24d9`; unchanged historical test: `af4824bdb0e36fb60057b2b2753f8dbc41718c8be2157088a19e1399140bccd5`. No additional SSO candidate or new Claude overlay is included.

The admitted Linux ARM64 software QA used an offline/frozen build, 4 CPU, 9 GiB RAM, swap 0 and a private bounded scratch area. Cargo JSON identified four actual libtest artifacts. A fresh listing preceded each exact single-thread parent; each completed with status 0 and one PASS/zero FAIL/zero ignored. Commands executed inside that QA were:

```sh
cargo test --frozen --offline --lib --test event_kind_ingest --test d5_control_audit --test worker_relay_api --no-run --message-format=json
cargo check --frozen --offline --all-targets --message-format=json
```

The all-target check compiled its targets; the thirteen selected parents were the tests executed. Root verified 471 raw evidence records, 44 static artifacts and all 2,605 post-run payloads/modes. The post-run source export SHA-256 is `d3cce5ad480f697788023754e8a470b75a58d28b2a7728fbd85bd249c06a203d`. Orchestration and exact-container cleanup returned 0; the container was removed and no OOM event occurred. The measured 7,346,417,664-byte peak belongs to compilation/QA, not worker usage. Holder exit 137 was observed without an established cause.

## Historical evidence and limits

The separate earlier reproduction reached the final historical assertion with `unknown authoritative relay event`, an unhealthy reopen and the same integrity error, after real ordinary commits and unchanged snapshots. The canonical canary passed. That RED remains attached to its original source and receipt. Independent review `9e957100…` is GO_SOURCE for the one-literal correction and protected test bytes; the current execution provides the new software result. [Earlier namespace evidence](../event-kind-namespace/README.md) remains dated and unchanged.

Run-2 **D4**, fingerprint `audit-ingest-reserved-kind-case-bypass-relay-validator-latch`, and **RC-P6-R06** now have verified targeted software coverage. **Global closure is not claimed.** Historical report results remain unchanged. Arbitrary corruption repair, operator recovery, power/storage faults, other platforms, real workers/engines, IdP/providers/models, network confinement, load, Clippy strict, the full test suite and production qualification are outside this campaign. No production gate is closed. See the [audit follow-up](../../../AUDIT_FOLLOWUP.md), [run-2 register](../../../AUDIT_RUN_2_20261110.md) and [risk follow-up](../../../RISK_COVERAGE_20261112.md).

---

# Lecteur historique des événements relais — preuve logicielle vérifiée

12 novembre 2026, 21:24 UTC. [Résultats et identités exacts](TESTS.json)

La correction ciblée du lecteur passe **13 parents uniques, aucun échec ni ignoré**, puis `cargo check --all-targets` RC 0, sur le gel ci-dessous. Les neuf parents d’admission sont inclus et ont été réexécutés dans ces treize ; la campagne antérieure de neuf tests reste une preuve distincte, sans ajout au compteur. Cette page décrit le comportement logiciel vérifié sans annoncer l’intégration ou la publication du correctif lecteur.

## Changement et comportement observé

Dans `src/audit/relay.rs`, `kind LIKE 'control.relay.%'` devient `kind GLOB 'control.relay.*'`. Le sélecteur sensible à la casse conserve le préfixe canonique exact. Les parseurs stricts existants refusent toujours un suffixe canonique inconnu. Aucun kind, payload, hash, schéma ou projection n’est normalisé ou réécrit.

La régression historique commit réellement `Control.relay.x`, `CONTROL.relay.x`, `CoNtRoL.ReLaY.x` et `Fixture.MixedCase` par l’ancien writer ordinaire réel. La validation directe du relais réussit ensuite ; le Store existant rouvre sain avec une intégrité valide. La séquence et le hash de tête, les valeurs des onze colonnes d’événements et les comptes des trois projections relais initialement vides restent identiques. La comparaison porte sur les valeurs stockées, pas sur l’identité physique des fichiers SQLite/WAL.

Le canari canonique soumet séparément `control.relay.x` et `control.relay.X` au vrai validateur dans des transactions checked ouvertes. Les deux rendent `unknown authoritative relay event` ; le rollback réel préserve les snapshots et la réouverture saine. **Aucune ligne canonique invalide n’est committée.**

| Couverture sélectionnée | Parents uniques |
|---|---:|
| Compatibilité historique et validation/rollback canonique | 2 |
| Régressions d’admission, unités du helper et canaris D5/relais, réexécutés sur ce gel | 9 |
| Voisins relais typés de révocation/replay et de temps canonique | 2 |
| **Total exécuté sur ce gel** | **13** |

Les voisins typés utilisent de vraies opérations Store/SQLite. Leurs faits runtime sont synthétiques ; ils ne lancent aucun worker. [TESTS.json](TESTS.json) donne les sélecteurs complets et chaque oracle à un test.

## Source et preuves d’exécution

La source contient **2 605 fichiers réguliers** : Git `702f35ba205d0e67cfb3c5b6d5dc8fcc77eacb6b` plus les postimages exactes du lecteur et du test historique. Sur les 2 604 fichiers Git, 2 603 restent identiques ; un fichier lecteur est remplacé et un fichier de test est ajouté. SHA-256 de l’archive source : `94e6e30dc9e57d646ccaf41cb7e9d568148bc9181044f180656889751be7e24b`. Postimage du lecteur : `71cf8042ff937d1b51d99379cac25e604b88af0cff420180d4856d1b6a9d24d9` ; test historique inchangé : `af4824bdb0e36fb60057b2b2753f8dbc41718c8be2157088a19e1399140bccd5`. Aucun candidat SSO supplémentaire ni nouveau lot Claude n’est inclus.

La QA logicielle Linux ARM64 admise utilise une compilation offline/frozen, 4 CPU, 9 Gio de RAM, swap 0 et un scratch privé borné. Le JSON Cargo identifie quatre véritables artefacts libtest. Une liste fraîche précède chaque parent exact à un thread ; chacun termine au statut 0 avec un PASS, zéro FAIL et zéro ignoré. Commandes exécutées dans cette QA :

```sh
cargo test --frozen --offline --lib --test event_kind_ingest --test d5_control_audit --test worker_relay_api --no-run --message-format=json
cargo check --frozen --offline --all-targets --message-format=json
```

Le check toutes cibles compile ses cibles ; les tests exécutés sont les treize parents sélectionnés. Root vérifie 471 pièces brutes, 44 artefacts statiques et les 2 605 contenus/modes après exécution. SHA-256 de l’export source postrun : `d3cce5ad480f697788023754e8a470b75a58d28b2a7728fbd85bd249c06a203d`. L’orchestration et la fermeture du conteneur exact rendent 0 ; le conteneur est retiré et aucun OOM n’est observé. Le pic mesuré de 7 346 417 664 octets concerne compilation/QA, pas l’usage d’un worker. Le holder termine à 137, sans cause établie.

## Preuves antérieures et limites

La reproduction antérieure distincte atteint l’assertion historique finale avec `unknown authoritative relay event`, une réouverture non saine et la même erreur d’intégrité, après de vrais commits ordinaires et des snapshots inchangés. Le canari canonique passe. Cette RED reste liée à sa source et à son reçu d’origine. La revue indépendante `9e957100…` est GO_SOURCE pour la correction à un littéral et les tests protégés ; la présente exécution fournit le nouveau résultat logiciel. Les [preuves antérieures de namespace](../event-kind-namespace/README.md) restent datées et inchangées.

Le constat run-2 **D4**, fingerprint `audit-ingest-reserved-kind-case-bypass-relay-validator-latch`, et **RC-P6-R06** disposent désormais d’une couverture logicielle ciblée vérifiée. **Aucune clôture globale n’est annoncée.** Les résultats historiques des rapports restent inchangés. Réparation de corruption arbitraire, reprise opérateur, pannes d’alimentation/stockage, autres plateformes, workers/moteurs réels, IdP/fournisseurs/modèles, confinement réseau, charge, Clippy strict, suite complète et qualification production restent hors de cette campagne. Aucun gate de production n’est fermé. Voir le [suivi des audits](../../../AUDIT_FOLLOWUP.md), le [registre run-2](../../../AUDIT_RUN_2_20261110.md) et le [suivi des risques](../../../RISK_COVERAGE_20261112.md).
