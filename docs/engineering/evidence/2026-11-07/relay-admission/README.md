# Admission durable du relais — 7 novembre 2026

**PASS logiciel ciblé : trois parents et compilation toutes cibles sur le gel 71.**
Le contrôle réserve un canal puis consomme un claim exec distinct une seule fois,
avec journal typé dans le Store tenant et audit avant réponse. Ce résultat ne
raccorde pas encore le propriétaire du canal au port runtime ; leurs factories
restent fermées et aucun gate de production n'est clos.

## Source et contrat

Base `d550c347cfe2f12756419202af58f2d05e46d0f1`, candidat testé
`ea5896ea0c8700c6a2c023aabaff6b8d9c8a9b73`, gel privé
`relay-admission-fix71`. Le code provient de `09fd28a`, avec correction
`e578e3c` après revue indépendante : `execution_arms.payload_json` conserve un
`ArmV1` direct, et le nouveau lecteur suit ce format réel.

Le [manifeste sélectionné](source.sha256) couvre les treize fichiers du lot
(douze fichiers Rust de code/tests et le
[contrat d'admission](../../../RELAY_ADMISSION.md)), ainsi que `Cargo.toml` et
`Cargo.lock`. Les douze fichiers Rust et Cargo restent identiques au gel 71 ;
seule la documentation est finalisée après les tests. Le manifeste complet
du gel 71, `relay-admission-fix71-source.sha256`, a pour SHA-256
`8b04ffb62fe217509be04fcc0e14f9cad0f017e3b05ad965db1bbf3fee630402`.
Avant cette mise à jour documentaire, les quinze entrées sélectionnées
correspondaient à ce manifeste complet, contrat compris.

Le journal versionné `relay_channel.v1` utilise la table `events` existante,
hors des parseurs historiques `control.*`. Le nouveau namespace est réservé
aux mutations typées, sans schéma 11. L'attribution est explicitement celle
d'un initiateur technique `legacy_runner` ; les faits runtime sont des rapports
du broker, pas des inspections indépendantes du gateway.

## Reproduction et résultats

Le RED 69 précède l'implémentation. Il porte sur `841272d`, avec un seul parent :
après préparation réussie du worker Running et de ses ancrages UTC, la route
d'admission absente renvoie HTTP 404 au lieu de 201. L'assertion échoue à
`tests/support/relay_admission_cases.rs:177` : 0 PASS, 1 FAIL, RC 101,
0,20 s de test et 131,69 s au total. Une compilation ratée ou une erreur de
fixture n'est pas substituée à ce RED.

| Gel / sélection | Résultat observé |
|---|---|
| 69, parent réservation/révocation seul, source `841272d` | RED HTTP 404 contre 201 ; un échec attendu. |
| 71, `worker_relay_api`, filtre `relay_admission_cases` | 3 PASS, 0 FAIL, 0 ignoré ; 40 filtrés, non comptés PASS ; RC 0 ; 1,02 s de test, 127,33 s au total. |
| 71, `check --all-targets` | RC 0 ; 68,07 s au total ; aucun test exécuté par cette commande. |

Les argv ci-dessous sont consignés par l'agent Luna exécutant les essais ; les
journaux de sortie ne les réimpriment pas tous. Ils sont reproduits comme
commandes exécutées, sans nouvelle exécution lors de cette finalisation :

```sh
cargo test --test worker_relay_api --locked --offline relay_admission_cases::relay_admission_reserved_replays_and_revocation_blocks_claim_without_worker_terminal -- --exact --test-threads=1 --nocapture
cargo test --test worker_relay_api --locked --offline relay_admission_cases -- --test-threads=1 --nocapture
cargo check --all-targets --locked --offline
```

Les trois fonctions uniques vérifient les comportements suivants sur vrais
routeur Axum et Store SQLite, avec faits runtime synthétiques :

| Parent dans `relay_admission_cases` | Portée exercée |
|---|---|
| `relay_admission_reserved_replays_and_revocation_blocks_claim_without_worker_terminal` | Réservation, replay sans nouvel événement, révocation bloquant le claim, annulation non claimée encore possible ; worker Running et quota conservée. |
| `relay_admission_claim_is_single_use_and_unknown_requires_exact_worker_terminal` | Claim frais puis replay avec même reçu et sans nouvel événement ; second claim distinct refusé ; état inconnu empêchant fausse clôture et nouvelle epoch ; clôture après terminal exact persistant, malgré révocation ; intégrité et reopen en supervision inactive. |
| `relay_admission_guards_roles_tenant_and_reserved_namespace` | Viewer, workload et faux credential refusés avant lecture du corps ; tenant B refusé sur l'admission de A sans mutation de sa base ; champ tenant libre refusé ; faux événement du namespace interdit sur les deux routes génériques. |

La compilation toutes cibles conserve les avertissements existants :
61 bibliothèque, deux intégration SSO et deux bibliothèque-test. Ce résultat
n'est ni un Clippy strict, ni un build sans avertissement, ni une suite complète.

## Journaux, conservation et environnement

Les journaux privés parent sont identifiés par leur nom et leur SHA-256 :

| Pièce | SHA-256 |
|---|---|
| `relay-admission69-red-am.log` | `1dec4bba75f6439a76721a3cb8dfb834002920c6f7d0c62c58f2b14b09865032` |
| `relay-admission-repro69-source.sha256` | `2919fa9f1fe0ad029757c73326225e5914ffdf943c0291fd8dd156a2069151bd` |
| `relay-admission71-am.log` | `59e1bac33526e69a786cd6318587a5ba39996a67944e07e1c732c5bc992cd296` |
| `relay-admission71-check-am.log` | `8cc9ab2bc03f06802a27294121475bc8f3474ede4d571aaae53eab65ae753759` |
| `relay-admission-fix71.tar.gz` | `a34a80924f18d4898c6cdb5457e03e6dc23be545f8fe73ca09bc4972338fd628` |
| `qa-am-inspect.json`, initiale | `2443fb42b14fb5ebf3a23502526a87bbef4e61f91b43b9ecdbc75d503cb0f1a0` |
| `qa-am-preflight.log` | `0b7da35cb6a0145170f874f17a5a6af738d8f39540fd82d7c56788ca742071cb` |
| `qa-am-final-inspect.json`, terminale | `46846debe2b85c74eabbd83f12d9622f3b22c1e6baecaf63b7f651feabd21e0a` |

Les sorties 69/71 et manifestes ont été lus et leurs empreintes revérifiées.
L'archive 71, son manifeste complet, les deux journaux 71 et l'inspection finale
sont conservés hors du scratch et hors distribution dans le checkpoint privé
`relay-admission-runtime-checkpoint-20261107-2308`, avec `SHA256SUMS`.
Les pièces 69, l'inspection initiale et le preflight restent dans le registre
privé parent `ai-guard-cold-drain-logs-20261106.nXx9XJ` ; aucun lien public
stable vers ces emplacements privés n'est annoncé.

QA **AM**, Linux ARM64, image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
Rust 1.99.1, UID 1000:1000, capacités supprimées, NNP/seccomp, réseau coupé avec
loopback privé, racine/outils/registry/sources en lecture seule. Écritures
limitées au scratch de 4 Gio et 65536 inodes ; quatre CPU, 8 Gio sans swap,
256 processus, fichier de 512 Mio, nofile 1024 et core 0. Ces valeurs sont les limites
du harnais, pas une mesure de consommation du produit. Cargo travaille hors
ligne, avec cibles propres aux gels. L'environnement AN ultérieur ne reçoit
aucun crédit pour ces essais.

Les commandes 71 ont terminé avec RC 0 avant l'arrêt ultérieur du harnais AM.
L'inspection terminale constate `FinishedAt:2026-11-07T22:45:45.941383544Z`,
RC 137 et `OOMKilled:false`. Le parent attribue cet arrêt au watchdog puis
constate la suppression normale du conteneur QA vers 23:04 UTC. Ce terminal du
harnais n'est ni un résultat de test additionnel ni un terminal de worker
produit. Sources et journaux sont conservés séparément du scratch supprimé.

## Limites et travaux ouverts

La revue source bornée sur `e578e3c` n'établit aucun défaut bloquant résiduel
dans son périmètre ; elle n'est pas un audit complet. Les trois parents ne
qualifient pas tous les champs négatifs, les transitions ni les plafonds du
contrat. Le SDK HTTP composé, la panne d'audit ciblée et l'ouverture par
l'ancien lecteur `d550c34` restent **NOT_RUN**. Leur compilation ne vaut pas
leur validation fonctionnelle.

Le permis runtime doit encore lier le claim frais à la dernière observation
locale, l'échéance monotone et le `RoleLease` retenus. La preuve opaque de
jonction/RootRetirement manque : la clôture Store d'un canal déjà claimé exige le terminal
exact du worker persistant. Factories runtime/propriétaire fermées, transport
exec non raccordé, capacité 100, compaction/rétention, recette A1/A2/B1,
Docker/Podman/Kubernetes réels et fonctionnement après panne restent ouverts.
Aucun modèle, fournisseur, frontend, campagne E2E générale ou infrastructure
distante n'est crédité à ces résultats. Aucun gate n'est fermé.
