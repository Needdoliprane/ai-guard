# Candidats de fusion documentaire — broker et exigences SSO du mainteneur

Préparation privée uniquement, base commune c4c4d2a7560d31768da08d8ad9335f4f08667344. Aucune édition de main ou du worktree broker-service, aucun stash/checkout, Cargo/test, runtime, fournisseur, commit ou push. Root reste propriétaire de toute intégration et du checkpoint isolé codex/broker-service ; main devra être réconcilié ultérieurement selon la stabilité des éditions du mainteneur.

## Contenu et méthode

- `main/` : neuf fichiers exacts lus sur main, y compris AGENTS complet avec son bloc mémoire transitoire et docs/sso.md. `main-read.sha256` permet de détecter toute modification ultérieure de ces fichiers main.
- `ours/` : leurs homologues disponibles sur broker-service, plus SSO_OIDC.md technique. Les absences de docs/sso.md dans ours et de SSO_OIDC.md dans main/base sont conservées comme telles.
- `base/` : huit fichiers communs extraits avec git show c4c4d2a:path. Ces trois snapshots sont privés et sans droit d'écriture (fichiers0400/répertoires0500).
- `merged/` : dix fichiers candidats, tous et seulement AGENTS/docs. `candidate.sha256` est externe au répertoire candidat. `from-ours.diff` décrit le delta depuis les copies de notre worktree ; c'est un diff de revue, pas une autorisation d'application sur main.

Fusion réelle à trois versions par `git merge-file --diff3 -p` : AGENTS, IMPLEMENTATION_PLAN, PRODUCTION_READINESS et TEST_STRATEGY fusionnaient automatiquement ; quickstart, ARCHITECTURE_CONTRACT, DECISIONS et VALIDATION avaient chacun un conflit éditorial. Les résolutions combinent les exigences du cahier mainteneur et les détails techniques existants, sans choisir un côté en bloc. Les ajouts répétitifs SSO dans les gates/plan ont été rassemblés en une entrée et un suivi détaillé. VALIDATION conserve intégralement le bloc broker et le lot documentaire SSO main ; les vérifications277liens de ce dernier sont explicitement datées d'avant fusion, sans prétendre les avoir rejouées.

docs/sso.md est **byte-identique au snapshot main**. AGENTS reprend les exigences canoniques main ; seul le bloc `<claude-mem-context>…</claude-mem-context>` est exclu du candidat Git. Ni le fichier main ni sa copie privée exacte n'ont été nettoyés. SSO_OIDC.md reste le contrat technique, avec hiérarchie explicite vers docs/sso.md et AGENTS ; son inventaire d'absence de code OIDC/PKCE/Keycloak est conservé.

## Table de préservation

| Exigence réelle à conserver | Origine exacte | Présence dans le candidat / statut |
|---|---|---|
| Utilisateur local nominatif hors ligne ; désactiver SSO ne désactive pas auth | main AGENTS, docs/sso.md | AGENTS inchangé hors mémoire ; sso byte-identique ; architecture, gates et SSO_OIDC nomment l'écart actuel, pas une fonctionnalité livrée |
| Traçabilité complète initiateur→demande→session→agent→délégation→sous-agent→commande→décision→broker→runtime→résultat/changements | main AGENTS, docs/sso.md | Cahier et gouvernance intacts ; plan IDN/TRC et matrice principale conservés ; initiateur/acteur et declared/vérifié distincts |
| Audit indépendant durable, chaîné, points de contrôle signés, export vérifiable | main docs/sso.md, plan TRC-03 | Cahier exact ; plan TRC-03 exact ; gates G06 et contrat technique mentionnent signatures/gestion des clés à concevoir et qualifier, sans prétendre que le chaînage existant les livre |
| Fournisseur OIDC configurable dont Keycloak, client public Code+PKCE S256 sans secret distribué | main AGENTS/docs/sso, notre SSO_OIDC | Canonique conservé ; contrat technique conserve deux clients Web/CLI, validation Rust, profils de token, state/nonce/verifier, redirects, JWKS et frontières réseau |
| Tenant/permissions administrés ; admin Keycloak n'est pas runner ; pas de token humain aux workers | main AGENTS/docs/sso, contrat technique | Exigences maintenues dans gouvernance, cahier, décision, architecture et contrat technique ; migrations sans réattribution fictive conservées |
| Sessions, rotation, révocation, logout et SSE ; pas de fallback silencieux | nos architecture/gates/SSO_OIDC et main matrice | Réunis dans architecture, gates et contrat technique, avec tests A/B et statut ABSENT/NOT_RUN |
| Essai Compose avec une seule clé du fournisseur choisi, cible~5min mesurée et non garantie | main AGENTS/docs/sso/quickstart | Copie exacte du cahier ; AGENTS, quickstart, décision et DEM-01 maintiennent la cible et distinguent cold build/téléchargement/démarrage présent |
| Démo opt-in loopback, Keycloak épinglé/realm/clients préparés, admin/admin et user/user | main AGENTS/docs/sso/DEM-02 | Toutes valeurs publiques et limitations conservées ; aucune adoption de compte/config existants, aucun compte de démo déclaré déjà livré |
| Deux procédures découverte/entreprise, réel Web+CLI+tâche non destructive+audit | main docs/sso et notre contrat technique | Cahier exact, plan DOC-01, matrice et procédures techniques conservés ; réel Keycloak/Web/CLI et mesure restent NOT_RUN |
| Sécurité/architecture Rust, pas de nouvel IdP métier/BFF/stockage implicite ni cloud autorisé | gouvernance et nos SSO_OIDC/architecture | Invariants conservés ; les décisions de transaction Web et d'accès réseau IdP restent à proposer/qualifier |
| Preuves broker40, source149, correction150 historique et gates ouverts | notre VALIDATION/IMPLEMENTATION_PLAN/TEST_STRATEGY ; preuves existantes hors candidat | Blocs broker conservés, mêmes liens et hashes ; G09 FAIL, 0/12 gates fermés ; bootstrap parent, runtime/TTY/D5/cancel retry/fsync/énumération non requalifiés |
| Aucune modification des149sources compilées/binaires | périmètre Root | Candidats limités à10documents ; vérification de checksum des149sources existantes conservée comme contrôle de lecture, aucun build ou nouveau binaire |

## Résolution des quatre conflits

1. Architecture : paragraphe canonique main conservé, puis détails techniques non redondants (clients distincts, validation/permissions Rust, session/SSE, migrations/A-B, absence de fallback/promotion et code absent).
2. Décisions : une seule décision SSO mainteneur, avec liens cahier/contrat technique/plan/matrice ; les avertissements techniques d'absence d'endpoint/flag/dépendance/implémentation et les guards avant contenu restent explicites.
3. Quickstart : cible mainteneur conservée, puis rappel factuel bref de l'auth actuelle runner/viewer et du contrat technique encore absent ; aucune commande future présentée comme exécutable.
4. VALIDATION : les deux entrées datées sont conservées, sans additionner des tests ni attribuer les preuves documentaires main au gel broker ou au présent candidat.

## Vérifications et usage

PASS de préparation : résolution sans marqueur de conflit ; absence du bloc mémoire dans merged et présence dans main ; cmp docs/sso.md identique ; diffcheck des candidats ; manifest main relu inchangé ;149checksums source de notre worktree inchangés. Aucun test applicatif nouveau et aucune qualification OIDC/runtime produite.

Relecture indépendante `/root/broker_outbox_v4/doc_merge_check`, en lecture seule : aucun manque substantiel ni contradiction matérielle trouvé. Elle confirme le cahier byte-identique, AGENTS limité au retrait mémoire, la hiérarchie technique et les absences explicites, ainsi que les40fonctions/149sources/gatesouverts dans VALIDATION/TEST_STRATEGY/IMPLEMENTATION_PLAN. Aucune exécution de test/code/runtime/service/réseau par ce relecteur.

Root peut comparer les hashes main juste avant intégration. S'ils diffèrent de main-read.sha256, refaire la recoupe des fichiers concernés plutôt que copier aveuglément. Une intégration sur la seule branche broker-service ne doit ni nettoyer AGENTS main ni toucher ses modifications SSO concurrentes. Les snapshots privés exacts restent conservés.
