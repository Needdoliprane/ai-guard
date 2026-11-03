# Suivi de l'audit indépendant — 29 octobre 2026

## Correctifs initiaux de l'audit en cours — 3 novembre 2026

L'audit courant reste ouvert. Ses sources sont `main@c4c4d2a` et
`codex/broker-service@d6cb136` ; les correctifs repartent de
`origin/codex/broker-service@0a41aac192f4139073a43830dbb71df01cb87abf`.
Les pièces originales de `~/security-audit-skill/ai-guard/run-1/` restent en
lecture seule. Les sévérités fournies sont provisoires ; les constats futurs
seront rapprochés du code courant, y compris des lots déjà intégrés.

Trois worktrees dédiés portent les lots export, tenants et ingress ; un
quatrième porte leur intégration. Le checkout principal et les travaux
`enforcer-full3`/`broker-supervision` ne sont pas modifiés. La page OIDC
`complete`, déjà corrigée par `96fe9be`, n'est pas reprise.

| Lot | Code revérifié et critère | État constaté |
|---|---|---|
| A — export | Métadonnées actives filtrées à l'écriture, contrôles/limites conservés, fichiers 0644 et scan brut distinct. | Deux RED comportementaux, puis 11 fonctions ciblées GREEN ; revue indépendante acceptée. Code intégré au candidat `357b6e0`, guides EN/FR synchronisés. |
| B — panne d'un tenant | Erreur voisine isolée, store propre fermé et refus des collisions préservés ; projection humaine partielle explicite. | Deux RED comportementaux, puis 7 tests Rust et 24 tests frontend GREEN ; TypeScript PASS. Revue indépendante acceptée, manifeste et guides EN/FR corrélés ; intégré dans `b4c2fee`. |
| C — ingress | Pool upload distinct, lecture bornée avant acquisition de l'échange ; corps vides admis sans occuper le pool upload. | RED avec 64 uploads actifs et GET 429 ; deux régressions et six canaris ingress GREEN. Trois tests model_ingress locaux supplémentaires sélectionnés par le filtre, sans modèle ; revue indépendante et guides EN/FR acceptés, intégré dans `57fae74`. |

Les tests de reproduction de l'audit passent lorsque le défaut existe : ils
ne constituent pas des régressions sans inversion des assertions. Une erreur
de compilation, zéro test sélectionné ou un dépassement du harnais n'est pas
un RED comportemental. Les tests ciblés sont confiés à Luna ; chaque lot
exige aussi une revue indépendante avant intégration, commit et push.

Pour B, la conception suit la correction proposée dans le rapport provisoire :
une erreur voisine ne choisit aucune identité et ne rend pas ses credentials
authentifiables ; toute collision lue dans un store sain reste refusée, y
compris après mutation du provisioning. Une absence de collision dans une base
illisible ne peut pas être prouvée. Cacher volontairement un binding dans une
telle base nécessite déjà l'accès aux fichiers du contrôle de confiance, qui
permet aussi de supprimer ce binding. Cette limite sera conservée dans la
preuve et la revue, sans prétendre à une vérification exhaustive d'une base
indisponible. Un index construit au démarrage ne suffirait pas à suivre les
mutations ultérieures. Les tests du candidat B préservent cette distinction ;
sa revue et son intégration ne qualifient pas la latence des scans voisins.

### Lot A — résultat intégré au candidat

Source d'implémentation `54546d5`, reprise avec traçabilité dans `357b6e0`.
Les deux RED ont exécuté chacun une fonction et échoué sur la présence de
`.git` après extraction, respectivement par le chemin public et le chemin
autorisé. Les erreurs antérieures de préparation du sandbox sont conservées
comme `NOT_RUN` dans les preuves, sans être comptées comme reproductions.

Après correction, commandes ciblées hors ligne :

```text
cargo test --offline --locked --test audit_workspace_export
cargo test --offline --locked --lib workspace::export_policy_regression_tests::authorized_export_filters_host_metadata_while_scan_keeps_observed_files -- --exact
```

Résultats naturels RC0 : six fonctions d'intégration, une fonction d'export
autorisé, puis quatre canaris exacts `workspace::export_durability_tests::`
sur les barrières succès, faute avant publication et faute après publication.
Onze fonctions uniques PASS ; aucun test ignoré additionné. Le canari de
profondeur hors sélection fsync n'a pas été exécuté dans cette campagne.

La revue indépendante confirme filtres, validation et consommation des entrées
ignorées, manifeste limité aux fichiers écrits, scan brut conservé et ordre
flush/mode/fsync/publication. L'avertissement est émis sur le stderr du
contrôle, sans noms ni contenus ; sa livraison au client IPC/dashboard n'est
pas démontrée. Sa branche d'échec IO reste `NOT_RUN`, avec propagation avant
publication vérifiée à la lecture. Aucun assainissement universel des projets
n'est annoncé. Les guides locaux EN/FR et opérations décrivent ces limites.

Preuves privées préservées sous
`/private/tmp/ai-guard-audit-corrections-20261103.Wv9egg/test-a-retry.T2xgVv9X/` :
snapshots RED/GREEN distincts et rapports `report/red-results.txt` et
`report/green-results.txt`. Les trois empreintes GREEN correspondent aux
fichiers intégrés ; `Cargo.lock` reste
`915e887ad3869bc20ed82cabad59ad16c2c58d9d26a2ac8acef9ae50c6afff98`.
Tests Linux ARM64 dans Docker isolé, Rust1.99.1 ; aucun payload Git exécuté,
worker réel, Podman, macOS, build release ou qualification production déduit.

Le lot A et ses guides ont été poussés dans `197e394`, référence distante
vérifiée. Son worktree a été fermé par `git worktree remove`, sans force,
après inventaire suivi/non suivi/ignoré vide, équivalence du patch intégré et
absence de consommateur ouvert. La référence source `54546d5` et les preuves
hors checkout restent conservées ; aucun volume ni donnée utilisateur supprimé.

### Lots B et C — résultats intégrés au candidat

B : source `f1b4122`, intégration `b4c2fee`. Les deux RED ont observé 503 au
lieu de200 pour B, après corruption de l'audit A puis sous verrou SQLite A.
Après correction, `cargo test --offline --locked --test tenant_api` passe4/4,
dont le refus503 pour le store propre verrouillé et la complétude rétablie
après reprise. Trois filtres exacts passent chacun1/1 :

```text
--test human_access credential_collisions_between_tenants_and_classes_never_select_a_tenant
--test oidc_gateway_flow oidc_subjects_read_only_their_durable_tenant_even_with_a_forged_tenant_claim
--test sso_cli signed_native_session_reads_exports_resumes_and_rotates_without_token_files
```

Ces filtres sont exécutés avec `-- --exact`, pas comme une suite SSO entière.
Le scénario OIDC vérifie aussi l'admission B malgré A indisponible, les
contextes partiels et le refus503 d'un sujet dont les appartenances restent
inconnues. Les collisions ajoutées après création du gateway restent refusées.
Frontend : quatre fichiers ciblés, 24/24 PASS, et TypeScript5.9.3 `--noEmit`
RC0 ; aucune vérification navigateur/DOM ou SSO réel déduite. Les nouveaux
guides et le contrat API exigent la mise à jour coordonnée des clients.

Preuves B : `test-b/reports/baseline-red.txt`, `green.txt` et
`green-source-hashes-final.txt` sous le répertoire privé des corrections.
Le manifeste complet est une vérification APRÈS tests, corrélée aux sept
empreintes annoncées avant exécution et au worktree ; aucune preuve antérieure
n'a été fabriquée. La copie GREEN est protégée de l'UID1001 par propriété1000,
répertoires0555/fichiers0444 et parent1777 appartenant à root (sticky bit,
caps effectives nulles, `no-new-privileges`). L'entrée RED bind readonly reste
inchangée. Les tentatives de préparation ratées restent séparées des résultats.
Les scans O(n) et attentes SQLite voisines subsistent : ce lot ne qualifie
ni l'équité ni la latence ; le lot D doit traiter cette exécution synchrone.

C : source `80fa688`, intégration `57fae74`. Le RED a gardé64 uploads en
lecture et observé429 pour le monitor attendu200. GREEN :
`cargo test --offline --locked --lib ingress::admission_tests:: -- --nocapture`
passe2/2 ; le filtre `ingress::tests::` passe les six canaris visés et trois
`model_ingress::tests::` supplémentaires par correspondance de sous-chaîne
(neuf fonctions au total, aucun vrai modèle ni accès externe). Aucun rejeu
inutile après ce constat ; les comptes restent distincts.
Preuves : `test-c/baseline-corrected/` et `test-c/green/` dans le même
répertoire privé. Les empreintes GREEN sont
`3019844dd3074e33ce1b0e4f8bee3a17a601f4c98aac475c1900455616f53f6d`
pour `src/ingress.rs` et
`4278626804449bab857178712bc39181661da0c798822b8763ebc28a621def5c`
pour la régression. Les preuves RED et les erreurs de montage `noexec`
`NOT_RUN` restent préservées séparément.

C ne neutralise pas toute saturation : des requêtes avec corps peuvent encore
recevoir429 du pool upload ; requêtes complètes/SSE et connexions lentes ont
leurs autres limites. Le runner conserve son arrêt sur erreur de suivi. La
recette historique d'arrêt utilisait un moteur fixture ; aucun worker réel ni
retry60s n'est qualifié ici. Un contrôle composé du candidat intégré reste
distinct des tests exécutés sur chaque gel de lot.

### Préparation vérifiable des régressions

Pour C, la régression a été renforcée et relue indépendamment : un compteur
RAII prouve que **tous** les corps entrés en lecture restent incomplets avant
et après chaque oracle HTTP, client de contrôle et SSE. Toute expiration,
même d'un seul upload, rend le harnais inconclusif au lieu de produire un faux
GREEN en libérant un slot. Le fichier préparé
`tests/ingress_admission/slow_uploads.rs` a pour SHA-256
`216dd33af61d68a36bddbc498e178a82d8340c57a03826063c0d96fb9d5c7663`.
Ce gel préparatoire a produit le RED attendu : 64 lectures actives, GET monitor
429 attendu200, RC101 sans timeout ni OOM. Après correction, le nouveau test
SHA-256 `4278626804449bab857178712bc39181661da0c798822b8763ebc28a621def5c`
garde 16 uploads actifs et observe 48 refus par le budget dédié ; monitor,
client de contrôle, POST vide et SSE passent. Les anciens snapshots restent
distincts, sans effacer leurs preuves.

Pour B, deux tests du **nouveau contrat de complétude frontend** ont pu être
exécutés sans dépendance téléchargée, sur le parser inchangé de `0a41aac` :

```text
node --test --test-isolation=none tests/human-identity-coverage.test.mjs
Node v24.22.2 ; RC 1 ; 2 tests ; 0 PASS ; 2 FAIL attendus (RED).
```

Le parser perd `contexts_complete=false` et accepte l'absence du champ au lieu
de refuser une réponse incompatible. Ce RED prépare la projection partielle
requise par le correctif B ; **il ne reproduit ni ne confirme le défaut
backend de collision/disponibilité**. Le gateway de base `0a41aac` n'émet pas
ce champ. Le candidat B associe désormais DTO et parser stricts ; le nouveau
contrat exige des versions coordonnées du gateway, du dashboard et des clients
CLI stricts. Les quatre fichiers de tests frontend ciblés passent ensuite
24/24, RC0 ; TypeScript5.9.3 `--noEmit` passe également. Navigateur/DOM et
qualification SSO globale restent `NOT_RUN` pour ce lot.

Exécution par Luna dans une copie publique minimale et un conteneur dédié :
réseau/IPC désactivés, racine/source en lecture seule, UID1000, aucune
capability, `no-new-privileges`, CPU1, RAM512MiB, PID64, fichiers8MiB,
scratch64MiB/8192inodes, environnement vide complété par allowlist, délai45s.
Aucun secret, socket moteur, montage original, fournisseur, installation ou
E2E. Conteneur attribué arrêté/supprimé après le résultat naturel ; preuves
privées conservées dans `/private/tmp/ai-guard-luna-front-EHNOca/proofs`.
SHA-256 du parser :
`65edb8ad1027664f2d37b226ac7f2e46e4c994218dc4716a0ddf80ccd9c9bff5` ;
du test :
`e3378e86bd74712939fd7ede9bb6a39fbbd812f0bfcd89d4d67dfbb344919a64`.

### Dépendance de validation

Le sandbox utilise une image Rust locale Linux/ARM64, sans réseau,
secrets, socket moteur ou montage du dépôt original, avec source et toolchain
en lecture seule et scratch borné. La préparation hors ligne du cache échoue :
`cargo vendor --offline --locked` manque `android_system_properties 0.1.6`, et
`cargo metadata --offline --locked --filter-platform aarch64-unknown-linux-gnu`
manque `linux-raw-sys 0.12.1`. Les images locales sondées ne contiennent pas de
cache Cargo exploitable. Ce blocage historique a été levé après l'accord
explicite du mainteneur : copie privée des seuls répertoires publics de
registre, puis `cargo fetch --locked --target aarch64-unknown-linux-gnu` depuis
un environnement parent vide. Seul `linux-raw-sys 0.12.1` a été téléchargé ;
la résolution complète Linux `--offline --locked` passe sans modifier le lock.
Cache privé : `/private/tmp/ai-guard-linux-cache-20261103.DOKqBt/` ; aucun
credential/configuration du home n'a été copié. Les builds/tests restent sans
réseau, avec cache et cible propres à chaque lot, jamais sur l'hôte.

Après redémarrage signalé par le mainteneur, Docker29.9.2 annonce 8CPU et
`12585238528` octets de RAM ; le nœud Kubernetes local est Ready, mémoire
`12290272Ki`, stockage `129759836Ki`. Cela confirme le réglage annoncé12GoRAM/
128Godisque, pas un adaptateur Kubernetes ai-guard. Le MacBook Air M3/16Go est
sous pression : aucun dépassement de délai de harnais n'est une preuve de
vulnérabilité et aucun chiffre de performance n'est qualifié par ces tests.

Validation documentaire seule : `git diff --check` et vérification des liens
relatifs PASS (13 cibles présentes sur les deux documents). Le scan des deux
fichiers candidats ne détecte aucun chemin interdit ni secret connu. La revue
indépendante de la proposition est traitée : reprise conservatrice après
redémarrage et formulation conditionnelle du correctif C. Aucun de ces
contrôles ne remplace les reproductions et tests Rust encore `NOT_RUN`.

Analyse statique de formatage distincte : le `rustfmt --check --edition 2024`
de la toolchain macOS1.99.1 disponible passe sur `src/workspace.rs`,
`tests/audit_workspace_export.rs`, `tests/tenant_api.rs` et
`tests/ingress_admission/slow_uploads.rs` (trois commandes RC0, aucune édition
ni exécution du code projet). Les sondes dans l'image Linux ont en revanche
montré l'absence du composant rustfmt : formatage Linux `NOT_RUN`, sans
installation. Ces lectures statiques ne débloquent pas la compilation ou les
reproductions comportementales Rust dans leur sandbox.

Les huit anciens gardiens de tests orphelins cités par le mainteneur ont été
identifiés par PID, propriétaire, naissance, programme et rôle, puis arrêtés
par SIGTERM ciblé avec son autorisation. Aucun volume, worker, credential,
preuve ou binaire conservé n'a été supprimé ; aucun autre processus n'a été
arrêté. Ce nettoyage manuel n'est pas une preuve de supervision autonome.

### Frontière de comportement et suite

Le runner conserve son arrêt ciblé sur erreur de suivi. La
[proposition de fenêtre transitoire](GATEWAY_TRANSIENT_FAILURE_PROPOSAL.md)
est soumise séparément ; elle n'est ni approuvée ni implémentée.
Les lots D/E/F attendent l'intégration de B et leurs propres reproductions.
L'outbox et les autres candidats non établis restent hors correction sur la
seule base de ce rapport provisoire.

**Les trois lots A/B/C sont intégrés au candidat, après reproductions,
tests ciblés et revues indépendantes. Aucun test worker/runtime réel exécuté
pour ces lots, aucune qualification production déduite.** Les résultats historiques ci-dessous
conservent leurs dates et leurs périmètres.

---

L'[audit importé](evidence/2026-10-29/independent-audit/REPORT.md) porte sur `/private/tmp/ai-guard-audit-20261029.n8ZxPB`,169 fichiers ; manifest annoncé `77876e6de40964096f90f458b07b872b7848f1a05257e2f763e5f57679641d34`. Ses originaux restent hors dépôt et inchangés. La copie de `findings.json` conserve **0 confirmé,19 needs_validation,0 rejeté**. Le workflow [Cloudflare security-audit](https://github.com/cloudflare/security-audit-skill) distingue confirmation, fait restant à vérifier et candidat réfuté ; aucune sévérité formelle n'est ajoutée ici.

Le checkout a évolué indépendamment. Le candidat209 et ses essais Docker/navigateur ne sont pas une reproduction contrôlée des19 candidats. Ce suivi distingue le delta du code, les preuves de développement disponibles et la validation de sécurité manquante. Une correction n'authentifie pas rétroactivement l'instantané ni ses données. Le statut BLOCKED de l'audit concerne son environnement de validation, pas l'ensemble des travaux produit autorisés.

## Rapprochement en cours

| Empreinte originale | État du code courant et prochaine preuve |
|---|---|
| `api/preauth-linear-tenant-credential-hash-amplification` | Correctif de développement : index privé partagé, digest d'entrée borné calculé une fois, pas de copie complète des secrets/politique au clonage de l'état. Guards typés avant JSON/query/path. RED4FAIL/2PASS puis51PASS ciblés, vraie réponse401 sans attendre le corps et revue du delta acceptée. [Preuves](evidence/2026-10-30/api-auth/README.md). Impact/charge DoS et transport lent non qualifiés ; verdict original needs_validation conservé. |
| `api/sse-graceful-shutdown-without-drain-deadline` | Correctif gateway/dashboard : drainage cinq secondes après signal, pattern ingress conservé, setup Unix fallible avant readiness. Vrais flux SSE maintenus >6 s avant SIGTERM puis sorties en5007/5010ms, SIGINT au repos vérifié ; revue et build PASS. [Preuves](evidence/2026-10-30/http-shutdown/README.md). IO/teardown bloqués, charge lente et autres environnements restent hors preuve ; verdict d'audit initial inchangé. |
| `api/viewer-synchronous-reads-control-store-contention` | Lectures Store synchrones sur les futures HTTP et connexion partagée. La nouvelle validation v2 ajoute du travail, sans preuve de contention mesurée ; admission et exécution bloquante à qualifier. |
| `audit/preexisting-database-permissions-not-enforced` | StoreStorage refuse parent non privé et main/WAL/SHM/journal non600/UID/lienunique/ACLadmise sans réparation. Gel327, tests macOS/Linux, ACL POSIX et deux UID réels, backup commun relu et maintenance QA explicite dans le [dossier SQLite](evidence/2026-10-30/sqlite-storage/README.md). Filesystems/ACL/UID non couverts et pannes restent ouverts ; verdict historique needs_validation inchangé. |
| `audit/sqlite-connection-path-reopen-identity-race` | Parents privés ancrés/revalidés, métadonnées par nom et HAS_MOVED obligatoire ; verrou distinct par parent et noms main ASCII évitent les collisions du namespace. Suppression du FD brut main après RED/GREEN F_GETLK : sa fermeture annulait les locks d'une autre connexion du même processus. VFS conserve ses FD actifs ; migration/backup utilisent le même garde. Handoff pathname contre sameUID/root/admin et substitution A→B→A non qualifiés ; [preuves et limites](evidence/2026-10-30/sqlite-storage/README.md), aucun verdict original réécrit. |
| `dashboard/static-root-symlink-follow-disclosure` | ServeDir configurable reste à examiner ; l'image QA readonly avec dashboard sans montage réduit certaines préconditions, sans qualifier tout front-dir hôte ou image dérivée. |
| `ingress:forward:shared-preauth-exchange-pool` | Pool global encore acquis avant lecture complète/authentification upstream. Timeouts bornent la rétention, sans démontrer disponibilité sous contention ; réservation du contrôle et test borné requis. |
| `operations/config-path-reopen-secret-substitution` | Initialisation par descripteurs puis montage Compose par pathname ; course de parent et identité du répertoire à analyser jusqu'au moteur, pas seulement jusqu'à canonicalize. |
| `operations/extended-acl-secret-projection-disclosure` | Bits de mode et propriétaire ne constituent pas une preuve portable contre une ACL étendue. Diff Linux/macOS et essai à deux UID requis ; aucune ACL existante supprimée. |
| `operations/jev-control-credential-reuse` | Correctif de développement gel235 relu : refus avant publication/init, avant client/startup et à l'assemblage GatewayState local/multi-tenant ; clé normalisée, politique conservée. Tests positifs/négatifs avec absence de connexion fournisseur. Verdict de l'audit original inchangé. |
| `redaction/json-object-keys-preserved` | Code modifié : clés JSON masquées avec self.text ; régressions antérieures disponibles. Relecture différentielle et limites de collision/projection en cours ; verdict original conservé. |
| `redaction/truncation-before-secret-masking` | Correctif de développement gel235 relu : union des occurrences littérales et suffixes connus sur octets originaux avant décodage, y compris périodicité/chevauchement ; métadonnées d'erreur CLI préservées, marqueurs privés fusionnés. Régressions et capture Docker synthétique passent. Les encodages/fragments arbitraires ne sont pas une garantie de cette API ; verdict original inchangé. |
| `runner/credential-env-path-reopen-substitution` | Lecture du fichier source liée à un FD O_NOFOLLOW/fstat ; ancienne course du lien final corrigée dans la source. Parents, snapshot et réouverture --env-file restent à qualifier. |
| `runner/decision-omits-known-worker-secret-redaction` | decide refuse une requête que son dictionnaire enrichi modifierait, avant HTTP. Tests d'autorisation existants ; ne pas confondre ce refus avec une garantie universelle sur secrets inconnus/encodés. |
| `runtime/abrupt-broker-stop-leaves-worker-unfenced` | Identités/checkpoints/verrou/reprise manuelle livrés ; aucune deadline autonome après mort du CLI. [Nouvelle preuve de développement Docker Desktop/macOS ARM64](evidence/2026-10-30/cli-crash/README.md) : SIGKILL du seul CLI, même worker encore Running après7s pour une limite5s, puis arrêt/réconciliation/audit/quota/usage vérifiés. Premier défaut de harnais conservé, unique retry corrigé. Supervision permanente approuvée mais non livrée ; cette reproduction sur un nouveau gel ne réécrit pas le verdict de l'audit original. |
| `runtime/doctor-effect-engine-identity-unbound` | Docker doctor/build/up/down utilisent désormais une cible Unix et un EngineID liés, revérifiés avant effet avec le même programme approuvé ; contexte modifié et ID divergent couverts par régressions. Le contrôle et l'effet restent deux connexions sans fencing atomique. Podman opérateur ne reçoit pas cette preuve d'identité par ce lot ; qualification distincte ouverte. |
| `runtime/path-resolves-project-executable` | Recherche PATH ambiant supprimée sur les parcours livrés. Politique2 : entrée d'installation connue ou approbation explicite absolue hors projet, chemin canonique ouvert par composants sans liens, FD du répertoire final retenu pour exec relatif, owner/modes/ACL vérifiés, hash borné, environnement reconstruit et invocation durable. Migration explicite readonly/auditée sans réarmer les claims. Gel261 et preuves Docker ciblées dans le dossier trusted-invocation ; aucune fermeture générale des plugins, dépendances, sameUID/root ou déplacements d'arbres de contrôle par un administrateur de namespace. Le verdict original reste needs_validation. |
| `runtime/worker-output-engine-log-retention-unbounded` | [Lot presetv2](evidence/2026-10-30/engine-logs-v2/README.md) : `--log-driver none` imposé et inspecté avant claim/start, compatible avec historiques v1. Régressions ciblées, revue et Docker réel canary/ACK503 PASS ; déploiement QA avec audit668 conservé puis690 après nouveau run v2. Baseline réelle : driver `local` avec rotation native, pas de rétention illimitée ni fuite confirmée sur l'instance testée. Garantie conditionnelle au broker/v2 inspecté ; anciens brokers, Podman/autres plateformes et pression disque générale hors preuve. Verdict historique needs_validation inchangé. |
| `runtime/workspace-volume-no-byte-quota` | Limites d'import/export et RAM ne limitent pas les octets/inodes écrits dans un volume. Quotas effectifs et réserve du contrôle restent obligatoires. |

Les analyses détaillées, en lecture seule et sans exécution des probes, sont archivées : [runtime](evidence/2026-10-29/independent-audit/ai-guard-audit-runtime-delta.md), [filesystem](evidence/2026-10-29/independent-audit/ai-guard-audit-filesystem-delta.md), [secrets et statique](evidence/2026-10-29/independent-audit/ai-guard-audit-secret-delta.md). Les quatre candidats API/ingress sont relus par l'orchestrateur aux sources courantes ; leurs effets restent NOT_RUN.

Après ce rapprochement, les correctifs de développement capture/masquage et collision Jev/credentials sont intégrés au [gel235](evidence/2026-10-29/audit-followup/README.md), avec revue, vérifications logicielles et essais Docker nommés. Leurs tests ne changent pas les verdicts du rapport figé. L'image209 et ses preuves restent distinctes du nouveau candidat ; aucune qualification production globale n'est déclarée.

## Ordre de travail

1. Fermer le choix du programme moteur et les handoffs de fichiers de contrôle, sans croire qu'un simple chemin canonicalisé lie un effet à un inode.
2. Traiter les frontières de capture/masquage et la réutilisation Jev des credentials ; conserver des régressions pertinentes, sans publier de canary ou secret réel.
3. Implémenter la supervision durable du broker, avec armement de durée avant start et reprise sans réarmer un claim. Le design préparatoire n'est pas une fonctionnalité livrée.
4. Qualifier stockage/logs et contention/drainage ; poursuivre la recette trois workers, relais et cibles runtime/cloud. Aucune de ces exigences n'est rendue facultative.

Les corrections sont soumises à la même méthode : comportement reproduit si possible, test correctif, second regard, vérifications intégrées et preuves adaptées à la portée. Un refus d'exécuter une probe d'audit sans sandbox conforme demeure un manque de preuve ; il ne devient ni un rejet du candidat, ni une qualification du produit.
