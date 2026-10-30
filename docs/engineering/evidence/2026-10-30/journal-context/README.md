# Contexte managed et plancher du journal — correctif du 30 octobre

Base `537f8d2c072139bb4ced0f3b7bd385f15fb1e9dd`. [Rapport des tests](tests.md), [revue indépendante](review.md), [cartographie ayant révélé le défaut](service-mapping.md). Les preuves du [journal initial](../broker-journal/README.md) restent inchangées.

## Défaut et correction

Le producteur `runner::managed::worker_spec` utilise `plan.operation_id` dans le spec. Le runtime recopie cette opération dans le handle ; la reprise existante vérifie aussi l'opération du plan. Le nouveau validateur du journal attendait pourtant l'UUID de tentative centrale, distinct, et sa fixture reprenait la même hypothèse incorrecte.

Les premiers PASS avec cette fixture ne prouvaient donc pas la compatibilité avec le runner. Après correction de la fixture selon le producteur, les [deux tests ciblés échouent à advance](red/retry.stdout) avec Conflict ; [stderr](red/retry.stderr), [RC101](red/retry.exit-code) et manifestes avant/après sont conservés. Le correctif applicatif change une seule comparaison vers `intent.operation_id()`. Il ne réécrit aucun handle, plan1/2/3, claim ou identifiant central. Les helpers gardent leur opération propre. Un checkpoint fabriqué avec l'UUID d'attempt à la place de l'opération est désormais refusé.

## Résultats

| Commande | Résultat constaté |
|---|---|
| `cargo test --locked --test broker_journal` | **PASS** : [12 fonctions](final/journal.stdout), RC0, dont les deux nouvelles ; aucun ignoré. |
| `cargo fmt --all -- --check` | **PASS**, [RC0](final/fmt-final.exit-code). |
| `cargo clippy --locked --all-targets -- -D warnings` | **PASS** après alias de type purement mécanique dans le test ; [diagnostic initial](final/clippy.stderr), [résultat final](final/clippy-retry.stderr). |
| `cargo build --release --locked` | **PASS**, [log](final/release.stderr), RC0 ; manifeste111fichiers [avant](final/release-before.sha256)/[après](final/release-after.sha256) identique. |

Le parcours est initial rev0 obligatoire (`managed=None`) → advance managed → load/reopen. Fournir un managed à accept rev0 est refusé sans marqueur accepté. Six incohérences parsables sont refusées sans avancer la tête : tenant, origine, spec hash, opération du handle, digest de handoff, identité worker du pending. Une origine de scope divergente refuse aussi accept.

Le test du plancher restaure les octets d'un ancien pointeur valide après deux révisions acquittées. Le writer vivant et son clone refusent load/reserve/advance. Après drop et reopen, le même ancien graphe reste lisible et rejouable : contrôle positif de validité du pointeur, sans anti-restore après redémarrage promis.

Environnement : macOS27.2/ARM64, Rust/Cargo1.99.1. Les12tests précèdent seulement l'alias de type du test demandé par Clippy ; aucun changement applicatif après eux. Les manifestes correspondants distinguent ces deux gels. Pas de nouvelle dépendance, pas de nouvelle suite globale ou de moteur pour ce correctif.

## Artefacts et limites

Artefacts privés conservés dans `/tmp/ai-guard-journal-context-qualified.j5GGRo`, fichiers600 :

- Binaire : `b884e84305f125cdaae610881ecc4e747f3ede0a5e634e4cf3391b5a01482ec6`.
- Rlib : `3c520d88d91cc2fd465d9435097c9c55cb02814ae18921a24ae31d10edaac670`.
- Source tar : `132a63f665a3847679acca6279b16dfe4a14de7cb6eef346a16f63d18f6ff966`.
- [Manifeste source111](source.sha256) : `96c711ac7ebad5fbd78e9cf5214ec24ef1ab5f8407e02c61c1351942400ff8d2`.

Les fichiers de preuve ont leur [manifeste séparé](SHA256SUMS). Les logs/rapports bruts sont copiés sans réécriture ; whitespace vérifié sur code, suivi et ce README.

**Portée :** Store SQLite réel pour le baseline batch/generic/None, mais autorité et observations runtime synthétiques ; aucun appel à worker_spec ni moteur par ces tests. La liaison d'identité est corroborée indépendamment par la source des producteurs. Ni preuve d'un worker démarré ni qualification nouvelle d'import/helper/fournisseur. **NOT_RUN** : Linux, Podman, moteur, modèle, navigateur/E2E, panne électrique, IPC/ordonnanceur/gardien, fencing/reprise du service et capacité100. Les canaris et le harnais Linux du lot précédent ne sont pas attribués à ce nouveau binaire. QA et facturation restent inchangées ; G09 demeure FAIL et aucun gate global n'est clôturé.
