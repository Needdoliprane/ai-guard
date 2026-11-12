# Event-kind namespaces — bounded evidence, 12 November 2026

## Scope / Portée

**Candidate evidence; integration and historical-reader GREEN remain open.** The admission correction and the historical-reader reproduction are separate freezes based on public `77f7edd6da9a2895a1277754c6144f893bcb8aea`. Results below are transcribed from Root-verified receipts, whose SHA-256 values were checked during documentation review. No build, test, service, network or runtime was executed to write this summary. Private reports and raw execution artifacts are retained separately and are not published here.

**Preuves de candidats ; intégration et GREEN du lecteur historique encore ouverts.** L'admission et la reproduction du lecteur historique portent sur deux gels distincts issus de la même base publique. Les résultats viennent des reçus vérifiés par Root, dont les empreintes ont été contrôlées lors de la revue documentaire. Aucun build, test, service, réseau ou runtime n'a été exécuté pour cette synthèse. Les pièces privées et les traces brutes restent conservées séparément.

The [API contract](../../../../api.md#namespaces-réservés--reserved-namespaces) describes ASCII-case-insensitive admission at a root or dot boundary without normalizing ordinary kinds. HTTP ingestion and generic `Store::append` retain different namespace deny-lists. Separately, HTTP refuses four exact D5 literals, while generic Store append retains only its exact retirement-resume literal guard; these literal checks remain case-sensitive. Typed control delivery is preserved. No tenant, role, endpoint or configuration flag is added.

Le [contrat API](../../../../api.md#namespaces-réservés--reserved-namespaces) décrit la comparaison d'admission insensible à la casse ASCII, à la racine ou à une frontière point, sans normalisation des noms ordinaires. Les listes de namespaces API et Store restent distinctes. Séparément, HTTP refuse quatre littéraux D5 exacts, tandis que l'append générique du Store conserve seulement sa garde littérale exacte de reprise du retrait ; ces comparaisons restent sensibles à la casse. La livraison typée du contrôle est préservée. Aucun tenant, rôle, endpoint ou flag n'est ajouté.

## Observed results / Résultats constatés

| Freeze / Gel | Observed result / Résultat observé | Source SHA-256 | Root receipt SHA-256 |
|---|---|---|---|
| `qa-event-kind-admission-green-20261112-r1` | 9 exact parents PASS, 0 failed/ignored; build/list and all-target check RC 0 / 9 parents exacts PASS, aucun échec/ignoré ; build/liste et check toutes cibles RC 0 | `8c1b9e0c84078dbc62666c5f242f3690fed59087943f749bbaaf9438a2a947a0` | `890d1746cef7a770bb8b2094635c1b94f2902466af8f0f5d8af7f5491c6631c0` |
| `qa-event-kind-history-red-20261112-r1` | Canonical validator/rollback canary PASS, then historical compatibility EXPECTED_RED RC 101; all-target check NOT_RUN / Canari de validation/rollback canonique PASS, puis compatibilité historique EXPECTED_RED RC 101 ; check toutes cibles NOT_RUN | `3feb3bcb863619753730e6247eac9faeff651e2ba03e41c0b83e412f2be17d5c` | `dbf9d2346f58c466b6be69be84123d84c85ee3eb3c2f3de7736210f1537e5e72` |

The admission tests cover both existing HTTP aliases with the real router and private SQLite fixtures, refusal before audit mutation, ordinary mixed-case byte preservation after reopen, helper boundaries and three neighboring D5/relay canaries. They do not exercise a network listener or a worker. The nine results are **not** combined with the two historical parents into an eleven-test PASS. Exact selectors and receipt identities are in [TESTS.json](TESTS.json).

Les tests d'admission emploient le vrai routeur et des fixtures SQLite privées : deux alias HTTP existants, refus avant mutation de l'audit, conservation des noms ordinaires après réouverture, frontières du helper et trois canaris D5/relais voisins. Ils n'exécutent ni listener réseau ni worker. Les neuf PASS ne forment **pas** une suite de onze PASS avec les deux parents historiques. [TESTS.json](TESTS.json) conserve leurs sélecteurs et identités distinctes.

The historical freeze changes tests only. Its first parent sends unknown canonical `control.relay.x` and `control.relay.X` to the real validator in open transactions: both are refused, rolled back and followed by a healthy reopen. **No invalid canonical row is committed.** Its second parent actually commits ordinary names allowed by the old guard, including `Control.relay.x`, then observes `unknown authoritative relay event`, `reopenedHealthy=false` and the same integrity error on the unchanged reader. Head, all eleven raw event-column values and relay projection counts remain unchanged before the final failing assertion at `history_compat_tests.rs:163:5`.

Le gel historique ne change que les tests. Le premier parent soumet les noms canoniques inconnus au vrai validateur dans des transactions ouvertes : refus, rollback et réouverture saine, **sans ligne canonique invalide committée**. Le second commit réellement des noms ordinaires admis par l'ancienne garde, dont `Control.relay.x`, puis constate l'erreur du validateur, la réouverture non saine et l'erreur d'intégrité du lecteur inchangé. Avant l'assertion finale en échec, la tête, les onze colonnes brutes des événements et les comptes des projections relais restent identiques.

## Source and closure checks / Sources et fermeture

Each QA freeze contains 2,602 source files. Root compared every post-run payload and mode, verified the raw/static evidence manifests and removed the exact QA container with closure commands RC 0 and no OOM event. The retained holder exit is 137 on both freezes; its cause is not established. The admission receipt corrects a textual 2,601-file marker to the independently verified 2,602 files and a source-manifest typo to `f703715a592a27334ce4713f75a188db2ee5c5defed2dfd6c5f035ba5d2605de`. These are evidence annotations, not reruns or repaired test outcomes.

Chaque gel comprend 2 602 fichiers. Root vérifie contenus/modes après exécution, manifestes de preuves et retrait exact de la QA : commandes de fermeture RC 0, aucun OOM. Le holder termine à 137 dans les deux cas, sans cause établie. Le reçu d'admission corrige un marqueur textuel 2 601 en 2 602 vérifiés et une faute d'empreinte de manifeste. Ces annotations ne constituent ni relance ni modification des résultats.

## Open work / Travaux ouverts

The minimal reader candidate changes only `LIKE 'control.relay.%'` to case-sensitive `GLOB 'control.relay.*'` and preserves the two historical tests. It is **source-only; GREEN NOT_RUN** in this evidence set. It must preserve rejection of canonical unknown suffixes and ordinary historical rows, with no rewriting of payloads, hashes or projections. Admission PASS alone does not repair an existing unhealthy store and supplies no operator repair procedure.

Le candidat lecteur minimal remplace uniquement le sélecteur `LIKE` par `GLOB` sensible à la casse et conserve les deux régressions. Il reste **source seulement ; GREEN NOT_RUN** dans ces preuves. Il doit préserver le refus des suffixes canoniques inconnus et l'historique ordinaire, sans réécriture des payloads, hashes ou projections. Le PASS d'admission ne répare aucune base existante et ne fournit pas de procédure de réparation opérateur.

Independent source review is **GO_SOURCE** for the one-line reader candidate, postimage `71cf8042ff937d1b51d99379cac25e604b88af0cff420180d4856d1b6a9d24d9`, with review JSON SHA-256 `9e9571002beda9fa78705e9f432aa692b3539dc05a7d0c4be365b14e1ce0225e`. This source verdict supplies no reader or composition GREEN. Final integrated-source verification remains open.

La revue indépendante de source est **GO_SOURCE** pour le candidat lecteur à une ligne, avec la postimage et l'empreinte JSON ci-dessus. Ce verdict de source ne fournit aucun GREEN du lecteur ou de la composition. La vérification finale du gel intégré reste ouverte.

Run-2 **D4**, fingerprint `audit-ingest-reserved-kind-case-bypass-relay-validator-latch`, and **RC-P6-R06** remain **OPEN** pending reader GREEN and verification of the final integrated source. Other audit entries keep their existing statuses. Clippy strict, the complete suite, real workers/engines, providers/models, IdP/SSO, network confinement, load, release and all production gates are outside these results. No global qualification or gate closure is claimed.

Le constat run-2 D4 et RC-P6-R06 restent **OUVERTS** jusqu'au GREEN lecteur et à la vérification finale du gel intégré. Les autres constats gardent leur statut. Clippy strict, suite complète, workers/moteurs, fournisseurs/modèles, IdP/SSO, confinement réseau, charge, release et gates de production restent hors de ces résultats. Aucune qualification globale ni fermeture de gate n'est déduite.
