# P1 — autorité de décision et persistance terminale

**LOT CLOS AU PÉRIMÈTRE NOMMÉ : code gelé et relu, trois tests réels distincts et neuf contrôles navigateur PASS sur Docker rootful/macOS ARM64.** Chaque étape est identifiée séparément des snapshots intermédiaires, sans modifier les dossiers antérieurs. La suite normale reste à 147 PASS/0 FAIL/3 ignorés ; les trois essais réels ont leurs commandes explicites distinctes. Aucune qualification globale n'est déduite de ce dossier.

## Gel logiciel 147

Le dossier `software-rootgel-147/` conserve seize copies, [provenance](PROVENANCE_ROOTGEL.tsv) et [manifest](software-rootgel-147/SHA256SUMS.txt). Les [treize empreintes source/binaire](software-rootgel-147/SOURCE_SHA256SUMS.txt) concordent au contrôle ; l'ancienne image contrôle ne contient pas nécessairement ces changements.

| Preuve du gel | Portée |
|---|---|
| [Fmt](software-rootgel-147/rust-fmt.log), [Clippy](software-rootgel-147/rust-clippy.log), [suite](software-rootgel-147/rust-tests-147.log), [release](software-rootgel-147/rust-release.log) | Fmt exit 0 constaté sans sortie, Clippy 2,39 s, **147 PASS/0 FAIL/3 ignorés**, release 17,04 s. Les ignorés réels ne deviennent pas des succès. |
| [Rapport rétention complété](software-rootgel-147/runtime-retention-report-rootgel.md), [14 sous-cas GREEN](software-rootgel-147/runtime-invalid-reference-14-green.log), [22 ciblés](software-rootgel-147/runtime-retention-targeted-22-green.log), [Clippy ciblé](software-rootgel-147/runtime-retention-clippy.log) | Aucun token/fallback par nom après référence create inutilisable. Les PENDING auteur sont historiques, remplacés par la revue finale ci-dessous ; aucun moteur qualifié par ces fixtures. |
| [RED post-traitement/CLI](software-rootgel-147/postprocessing-result-secret-red.log), [RED stdout](software-rootgel-147/terminal-output-boundary-red.log), [RED projection JSON](software-rootgel-147/adapter-decoded-secret-boundary-red.log), [RED clés JSON](software-rootgel-147/json-key-secret-red.log) | Assertions de perte de résultat et fuite de secret connu/fragments, sans valeur canary affichée ; corrections incluses dans les 147 tests. |
| [GREEN neuf cas CLI](software-rootgel-147/runner-nine-scenarios-green-before-json-key-fix.log) | Harnais à neuf cas PASS avant la dernière correction de clé JSON ; les deux tests de frontières JSON et ce harnais passent ensuite dans la suite complète finale. Ne pas compter neuf cas comme neuf tests indépendants. |
| [Rapport autorité API](software-rootgel-147/decision-event-authority-report.md), [revue](software-rootgel-147/review-decision-event-authority.md) | Prévention future des namespaces réservés relue ; ancien historique toujours non authentifiable rétroactivement. |

Le gel sépare le terminal connu de `postprocessing_error` et du cleanup `UNCONFIRMED`, rend l'acquittement `error_audit` explicite, arrête les helpers sans ACK et laisse les changements inconnus. La chaîne complète d'erreur est masquée avant retour CLI ; les sorties le sont avant troncature ; le JSON est décodé et masqué avant projection, clés incluses.

## Revue finale et réel du même code

Deux copies distinctes dans `review-and-real-rootgel/`, [manifest](review-and-real-rootgel/SHA256SUMS.txt) :

- [Revue finale](review-and-real-rootgel/review-runtime-retention-final.md), source `/tmp/ai-guard-review-runtime-retention.md` : PASS ciblé, pas de P0/P1 bloquant restant identifié. Code et preuves fournis relus indépendamment ; aucun nouveau probe de code cible ou moteur exécuté personnellement par le reviewer.
- [Docker réel final](review-and-real-rootgel/terminal-persistence-real.log), source `/tmp/ai-guard-p1-security-real-terminal.log` : 1 PASS/0 FAIL/0 ignoré en 1,25 s, vrai gateway/SQLite avec refus HTTP terminal par middleware. Instance arrêtée et volume retenus ; réouverture DB et intégrité couvertes par le test relu. Répertoire `ai-guard-terminal-audit-oViYdH`. Ce n'est ni une panne de disque ni le déploiement conteneurisé.

## Nouveau déploiement QA — rejeux ciblés PASS

Image `agentsandbox-control:qa-20261029-security-09c7b861`, ID `sha256:b4239720250bd8a0e05c478d5bd289726041ea753dd701fa4780e795ae9150f2`, uniquement sur la pile QA existante. [Build](deployment-security-09c7b861/control-build.log), [Compose](deployment-security-09c7b861/compose-up.log), [état trois services](deployment-security-09c7b861/deployed-health.log), [audit avant](deployment-security-09c7b861/audit-before-deploy.json) et [après](deployment-security-09c7b861/audit-after-deploy.json) sont copiés des sources indiquées dans la [provenance finale](PROVENANCE_FINAL.tsv), avec [manifest du déploiement](deployment-security-09c7b861/SHA256SUMS.txt). Les JSON d'audit sont identiques : 102 événements/même head ; volumes conservés.

- [Smoke du nouveau candidat](deployment-security-09c7b861/deployed-smoke.log) : 1 PASS/0 FAIL/0 ignoré en 3,62 s, refus sans conteneur/volume, original préservé et export neuf ; demandes successives, pas une recette trois workers.
- [Navigateur du nouveau candidat](deployment-security-09c7b861/deployed-browser.log) : exit 0 et neuf contrôles PASS, Node 24.22.2/Chrome 156.0.8078.48 ; QA déclarée séquences 125/136, export SHA-256 `18d75544ef6cb2031b715d3a5c9bedf12103178b8c7e811077f081e77df5510f`. Aucun effet runtime ou fournisseur déduit de ces événements.
- [Audit après les essais](deployment-security-09c7b861/audit-after-tests.json) : valide, 137 événements, head `c876b11546452d5e2567beab4922ad7de506dd4df330d3238a7c99c198390a37`. Croissance attendue par les nouvelles écritures, distincte du contrôle 102 avant/après déploiement.
- [Versions](deployment-security-09c7b861/runtime-version.log) : Docker CLI 29.2.1/Engine 29.9.2, VM/image Linux ARM64 et Compose 5.6.1. Aucun succès de l'ancien candidat n'est transféré.

## Confinement courant et couverture du candidat

Le [rejeu de confinement courant](final-candidate-coverage/runtime-confinement.log) compte 1 PASS/0 FAIL/0 ignoré en **3,78 s**, Docker rootful : restrictions/allocations, réseau none selon les contrôles réalisés, timeout, copie/export et rejet du callback. Session `cec5c1c4-51ad-4117-89df-9cd4e0a2fe29`, volume nommé et répertoire `ai-guard-runtime-confinement-WZzcGd` conservés. Ce test d'adaptateur direct ne qualifie pas à lui seul l'audit E2E. Les trois tests ignorés de la commande normale ont ainsi leurs trois exécutions courantes distinctes : confinement, terminal refusé, smoke.

Le [manifest complet de 78 fichiers](final-candidate-coverage/COMPLETE_SOURCE_SHA256SUMS.txt) couvre sources/configurations/tests Rust et front, images, déploiement, Cargo/lock/toolchain, politiques et schémas. Le [contrôle au gel](final-candidate-coverage/source-verification.log) retourne 78 OK, exit 0 ; ce contrôle de hashes n'est pas un test produit. [Manifest des copies](final-candidate-coverage/SHA256SUMS.txt). Il n'atteste ni reproductibilité des paquets APT, ni reconstruction byte-identique d'image. Les sources de ce dossier et des designs/revues sont dans [PROVENANCE_FINAL.tsv](PROVENANCE_FINAL.tsv).

## DESIGN — continuité du prochain lot

Les [trois conceptions archivées](design-next-not-implemented/README.md), avec leur [manifest](design-next-not-implemented/SHA256SUMS.txt), préparent admission fraîche, projections transactionnelles, consommation de start, identité d'autorité et reprise. Leurs routes/DTO/tables/primitives futurs sont **non implémentés** et leurs tests futurs **NOT_RUN**. La description P1 historique qu'elles contiennent ne remplace pas les preuves finales réalisées ci-dessus.

## Snapshot avant correctifs finaux

Le dossier `intermediate-before-final-fixes/` conserve vingt copies de logs/rapports de `/tmp/`. La [provenance des copies](PROVENANCE_INTERMEDIATE.tsv) donne chaque nom source et destination. Son [manifest SHA-256](intermediate-before-final-fixes/SHA256SUMS.txt) fige ces octets ; un rapport source complété ultérieurement ne remplacera pas cette copie.

| Preuve | Résultat acquis et portée |
|---|---|
| [Suite intégrée intermédiaire](intermediate-before-final-fixes/rust-tests-145.log), [release](intermediate-before-final-fixes/rust-release-before-final-fixes.log), [Clippy](intermediate-before-final-fixes/rust-clippy-before-final-fixes.log), [fmt](intermediate-before-final-fixes/rust-fmt-before-final-fixes.log) | 145 PASS, zéro échec, trois réels ignorés ; release 19,25 s. Antérieurs aux dernières corrections fallback de référence et cleanup ambigu : aucun transfert de résultat au futur gel. |
| [Rapport ingestion](intermediate-before-final-fixes/decision-event-authority-report.md), [revue](intermediate-before-final-fixes/review-decision-event-authority.md) | Prévention des nouvelles ingestions réservées relue, 21 tests ciblés corroborés par le log intégré. Anciennes décisions potentiellement forgées indiscernables des décisions légitimes : aucun assainissement historique ni autorité de lancement prouvé. |
| [Premier Docker réel](intermediate-before-final-fixes/terminal-persistence-real-first.log), [rejeu avec réouverture DB](intermediate-before-final-fixes/terminal-persistence-real-reopen.log) | Un test PASS par essai, deux instances et volumes distincts retenus. Vrai Docker et vrai gateway/SQLite, middleware refusant HTTP terminal ; pas de panne physique de disque. Antérieurs aux correctifs finaux ; pas de qualification du gel à venir. |
| [Rétention auteur](intermediate-before-final-fixes/runtime-retention-report-before-final-review.md), [RED initial](intermediate-before-final-fixes/runtime-retention-initial-red.log), [GREEN initial](intermediate-before-final-fixes/runtime-retention-green-before-final-fixes.log) | Nouvelle API retained et cleanup explicite ; fixtures Rust. Le rapport décrit son gel antérieur, pas la clôture finale du lot intégré. |
| [Runner RED](intermediate-before-final-fixes/runner-terminal-red.log), [GREEN](intermediate-before-final-fixes/runner-terminal-green-before-final-fixes.log) | Ordre audit terminal/cleanup et conservation du résultat reproduits ; le verdict final était encore attendu à ce snapshot. |
| [Audit de cleanup RED](intermediate-before-final-fixes/cleanup-error-audit-red.log), [GREEN](intermediate-before-final-fixes/cleanup-error-audit-green.log), [ambigu GREEN](intermediate-before-final-fixes/cleanup-ambiguous-green.log) | Préserver le résultat métier malgré échec de rm puis de son audit ; une réponse rm perdue ne prouve pas que l'instance est encore présente. |
| [Observation inconnue RED](intermediate-before-final-fixes/runtime-uncertainty-red.log), [GREEN](intermediate-before-final-fixes/runtime-uncertainty-green.log) | Ne pas promouvoir un terminal manquant en code connu. Harnais, pas moteur réel. |
| [Référence create RED](intermediate-before-final-fixes/runtime-invalid-reference-red.log), [GREEN avant gel](intermediate-before-final-fixes/runtime-invalid-reference-green-before-rootgel.log) | Six cas négatifs ont révélé l'adoption par fallback de nom après référence acquittée invalide. Correction et revue finale à intégrer au gel. |

Les champs de cleanup doivent conserver l'incertitude : `UNCONFIRMED` après réponse de suppression ambiguë ; audit d'erreur `not_applicable`, `acknowledged` ou `unacknowledged`. Si ce dernier n'est pas acquitté, aucun helper suivant ne doit partir, `changed_files` reste inconnu, le terminal déjà observé est rendu et la CLI échoue. Ces critères, encore à confirmer à ce snapshot, sont relus sur le gel final ci-dessus, sans promettre une instance encore présente après une suppression dont la réponse est perdue.

La copie de preuves, les hashes et la lecture documentaire ne relancent ni test produit, ni moteur, ni opération de nettoyage. Registre, tentative/autorité runtime persistée, consommation durable, reprise après crash, quotas, relais inter-workers, usage, Podman, Kubernetes/cloud et qualification production restent hors succès de ce snapshot.
