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
| A — export | Le lecteur partagé réécrit les métadonnées Git et les modes. Filtrer les métadonnées actives à l'écriture, conserver les contrôles/limites et produire des fichiers 0644 ; scan brut distinct. | Régressions inversées préparées ; reproduction et correction `NOT_RUN`. |
| B — panne d'un tenant | Les scans de collision et de profil humain interrogent tous les stores. Une erreur voisine ne doit pas bloquer une identité saine ; son propre store reste fermé et les refus de collision doivent être conservés. | Régression existante et variante credential local B préparées ; reproduction et correction `NOT_RUN`. La liste de contextes partielle doit être signalée. |
| C — ingress | Le pool de 64 échanges est acquis avant lecture du corps. Des uploads incomplets ne doivent plus épuiser les échanges sans corps du contrôle. | Régression de contention préparée ; reproduction et correction `NOT_RUN`. La chaîne historique d'arrêt utilise un moteur fixture. |

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
mutations ultérieures. Les critères de B ne sont pas encore acceptés.

### Dépendance de validation

Le sandbox prévu utilise une image Rust locale Linux/ARM64, sans réseau,
secrets, socket moteur ou montage du dépôt original, avec source et toolchain
en lecture seule et scratch borné. La préparation hors ligne du cache échoue :
`cargo vendor --offline --locked` manque `android_system_properties 0.1.6`, et
`cargo metadata --offline --locked --filter-platform aarch64-unknown-linux-gnu`
manque `linux-raw-sys 0.12.1`. Les images locales sondées ne contiennent pas de
cache Cargo exploitable. **Exécution des reproductions : BLOCKED par les
dépendances Linux locales manquantes ; aucun repli de test sur l'hôte.**
Une autorisation précise de préparer un cache public verrouillé a été demandée
au mainteneur ; elle ne donne pas de réseau aux builds/tests.

Validation documentaire seule : `git diff --check` et vérification des liens
relatifs PASS (13 cibles présentes sur les deux documents). Le scan des deux
fichiers candidats ne détecte aucun chemin interdit ni secret connu. La revue
indépendante de la proposition est traitée : reprise conservatrice après
redémarrage et formulation conditionnelle du correctif C. Aucun de ces
contrôles ne remplace les reproductions et tests Rust encore `NOT_RUN`.

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

**Aucun lot correctif intégré, aucun test runtime réel exécuté pour ces lots,
aucune qualification production déduite.** Les résultats historiques ci-dessous
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
