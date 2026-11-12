# Configurer ai-guard

[English](README.md) · [Français](README.fr.md) · [Projet](../../README.fr.md)

Ce manuel décrit le contrat du produit complet. L'état d'implémentation et les preuves restent dans le [suivi d'ingénierie](../engineering/CURRENT_STATE.md). Choisir le contexte selon ses besoins d'identité et d'inférence ; topologie locale, identité ai-guard et authentification du fournisseur IA sont des réglages indépendants.

## Guides d'installation

| Guide | Éléments configurés |
|---|---|
| [Poste local](local/README.fr.md) | CLI Rust, Docker/Podman, services de contrôle, premier worker, permissions, copie/export, données et reprise. |
| [Broker local de confiance](broker/README.fr.md) | Registre privé2, profil de lancement Docker, service/IPC, identités de replay, approbation, annulation, reprise et export. |
| [Identités workload et relais](workloads/README.fr.md) | Migrations5→6→7 explicites, credentials restreints, registre des brokers et sa vue humaine autorisée séparément, messages dirigés, budgets d'audit et maintenance. Transport runtime distinct. |
| [Fournisseurs IA](providers/README.fr.md) | Codex et Claude Code par abonnement ou API, sélection explicite, credentials privés et Jev optionnel. |
| [Accès local nominatif](identity/README.fr.md) | Provisioning privé des comptes humains, permissions explicites sur ressources et registre brokers, consultation authentifiée, export et reprise sans IdP. |
| [Vue de supervision unifiée](supervision/README.fr.md) | Page du dashboard nominatif réunissant brokers, workers et ressources autorisées : qui voit quoi, fraîcheur et couverture, recherche, corrélation, refus, budget de lecture et limites. Composant candidat. |
| [SSO ai-guard](sso/README.fr.md) | DEMO locale neuve : images propres, réseau d’identité inspecté, préparation du volume UID1000 et réouverture normale. Couvre aussi fournisseurs existants/entreprise, connexion Web, CLI au premier plan et reprise. Interopérabilité complète gateway/Web/CLI non qualifiée. |
| [Fournisseur Keycloak](keycloak/README.fr.md) | Projet fournisseur local séparé, clients publics Web/CLI, PKCE, claims et préparation de l'IdP d'entreprise. |
| [IA locale](local-ai/README.fr.md) | Image du petit modèle épinglée, limites mémoire/contexte, inférence interne et remplacement du modèle. |
| [Entreprise](enterprise/README.fr.md) | Services de contrôle partagés, frontières tenant, AC privées, inférence interne et exploitation. |
| [Autorité et inspection Kubernetes](kubernetes/README.fr.md) | Confiance HTTPS explicite et diagnostic Rust en lecture seule pour la cible Docker Desktop ; admission de workers indisponible. |
| [Points de contrôle signés de l'audit](audit-signatures/README.fr.md) | Commande hors ligne `ai-guard-audit-checkpoint` : clé de signature Ed25519, points de contrôle signés de la chaîne d'audit d'un tenant, ancre de confiance tenue de façon indépendante, détection de rollback, rotation de clé, export de preuve et diagnostic. Contrôle sur chemins de l'hôte uniquement ; le volume Compose n'est pas desservi. |
| [Diagnostic opérateur](diagnostics/README.fr.md) | `ai-guard-diagnose`, rapport en lecture seule avant lancement, hors ligne par défaut : dispositions, tenants et plan de capacité, références de credentials par métadonnées, sélection fournisseur, bundles d'AC et éléments non examinés. Il n'ouvre aucun fichier que les fichiers inspectés désignent comme credential et ne qualifie rien. |

Chaque guide couvre prérequis, configuration, lancement, accès attendus, diagnostic, arrêt et conservation des données. Versions anglaise/française et exemples évoluent ensemble lorsqu'un contrat change.

## Fichiers et consommateurs

| Fichier | Lecteur et usage |
|---|---|
| `.env.exemple.local` | Option Compose `--env-file`, socle Docker ou Podman. |
| `.env.exemple.local-rules` | Socle avec `deploy/docker/compose.jev-rules.example.yaml`, catalogue de décision Rust fini sans service modèle. |
| `.env.exemple.local-keycloak` | Compose du socle et projet Keycloak séparé. |
| [`.env.exemple.local-keycloak-sso`](../../.env.exemple.local-keycloak-sso) | Projet Keycloak séparé et `deploy/{docker,podman}/compose.sso.yaml` autonome ; sélectionner de nouveaux tags/projets et le nom exact du réseau d’identité externe inspecté. Le profil explicite de préparation initialise une seule fois son volume privé. |
| `.env.exemple.local-qwen` | Compose du socle et projet IA locale séparé. |
| `.env.exemple.local-qwen-tls-volumes` + `volumes.env` publié | Projet modèle TLS à volumes neufs et surcharge gateway ; suivre [la préparation explicite](local-ai/TLS.fr.md). |
| `.env.exemple.local-qwen-tls` | Alternative bind TLS explicite, seulement sur filesystem réellement supporté ; suivre [le guide TLS](local-ai/TLS.fr.md). |
| `.env.exemple.local-keycloak-qwen` | Réglages combinés ; utiliser les commandes séparées ci-dessous. |
| `.env.exemple.providers` | Modèle d'une **nouvelle entrée fournisseur privée** ; suivre son guide pour l'import explicite. Ni configuration Compose ni SSO. |
| `.env.exemple.entreprise` | Transport Compose du socle et override Jev optionnel ; identités, tenants et politiques d'inférence utilisent leurs configurations typées distinctes. |
| `<config-dir>/providers.json` | Sélection fournisseur Rust, créée/examinée avec `providers init/status` ; choix explicite via `--providers-file`. |
| JSON OIDC privé et ses [exemples publics](sso/README.fr.md#préparer-le-json-privé-du-gateway) | Gateway `--oidc-config` ; fichier versionné distinct, sans interpolation d'environnement. Initialisation explicite d'un audit neuf distincte de sa réouverture normale. |
| Fichiers privés runner/viewer/Jev | Références explicites du contrôle Rust, jamais environnement global des workers. |

Le profil volumes exige d’abord `model-tls provision` : passer son fichier de base puis le `volumes.env` publié avec deux options `--env-file`. Il ne fonctionne pas avec le seul exemple public ; ne pas inventer les sélecteurs. La CLI Rust ne source pas dotenv. Compose développe `${HOME}` et lit ces exemples seulement avec `--env-file`. L'environnement du poste peut remplacer des valeurs d'interpolation Compose ; examiner la configuration du terminal si une valeur diffère. Une option CLI configure le composant nommé, pas tous les services.

Les exemples publics décrivent les défauts locaux documentés ; compléter les sélecteurs obligatoires vides selon leur guide. Pour personnaliser, créer un nouveau fichier privé sous un autre nom et réutiliser ce chemin partout. Ne jamais écraser `.env`, credentials ou base existants. Une entrée fournisseur privée doit être0600 et son parent0700. Ne pas `source` une configuration téléchargée ou fournie par un projet.

Pour une installation SSO locale neuve, suivre la [procédure DEMO neuve](sso/README.fr.md#préparer-une-demo-locale-neuve). Son exemple env laisse volontairement vide le sélecteur de réseau d’identité avant inspection ; il ne démarre pas inchangé. Construire l’image de contrôle actuelle contenant `setup prepare` et l’image actuelle du realm, puis préparer le nouveau volume via stdin public. Aucun bind OIDC privé appartenant à l’hôte ou image publique préconstruite n’est requis. L’ancienne image de contrôle commençant par `954246…` ne contient pas ce préparateur.

## Combiner les contextes locaux

Construire ai-guard et initialiser le même répertoire privé que dans le [guide local](local/README.fr.md). Depuis la racine du dépôt :

```sh
docker compose --env-file .env.exemple.local-keycloak-qwen -p ai-guard-local -f deploy/docker/compose.yaml up -d
docker compose --env-file .env.exemple.local-keycloak-qwen -p ai-guard-keycloak -f deploy/keycloak-demo/compose.yaml --profile keycloak-demo-http up --build -d
docker compose --env-file .env.exemple.local-keycloak-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml up --build -d
```

Trois projets Compose fournissent un seul dashboard ai-guard. Les noms explicites conservent données et réseaux séparés. Ne pas fusionner ces fichiers Compose comme overrides. Les premières constructions acquièrent les images publiques et le modèle épinglé ; les préparer avant un usage hors ligne.

Ces anciennes commandes démarrent les projets séparés ; elles n'activent pas le SSO ai-guard, ne créent pas les memberships humains et ne raccordent pas le gateway au réseau d'identité. Utiliser les [fichiers SSO et leur préparation dédiée](sso/README.fr.md#préparer-une-demo-locale-neuve) pour une installation SSO neuve. Le parcours réel gateway/Web/CLI complet demande encore sa propre qualification.

| Accès | Défaut local |
|---|---|
| Dashboard | `http://127.0.0.1:4000` |
| Gateway | `http://127.0.0.1:8787` |
| Issuer Keycloak | `http://keycloak.ai-guard.test:18080/realms/ai-guard-demo` |
| Santé du modèle local | `http://127.0.0.1:18090/health` |
| Base locale Chat Completions | `http://127.0.0.1:18090/v1` |

Configurer le nom Keycloak selon son guide. Les ports publiés restent liés au loopback. Gateway, dashboard et modèle conservent leurs ports internes lorsqu'un port hôte change ; adapter l'URL gateway des commandes CLI et l'origine Web enregistrée dans Keycloak. Pour Keycloak, `AI_GUARD_DEMO_IDP_PORT` change le port interne et le port publié ; conserver le même issuer complet pour navigateur, CLI et contrôle. Rootless reste exigé par défaut.

## Arrêter et reprendre

Conserver **le même fichier env, nom de projet et chemin Compose** pour `ps`, `stop`, `start` et `logs`. Exemple :

```sh
docker compose --env-file .env.exemple.local-keycloak-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml stop
```

Répéter pour les projets à arrêter. Arrêter les workers actifs avec le broker avant leurs services de contrôle. `stop` conserve conteneurs et volumes nommés ; redémarrer un IdP ne réimporte ni ne remplace un realm existant. Changer de modèle demande un changement d'image délibéré, sans suppression des données d'identité ou d'audit.

Le parcours CLI `up/down` possède son propre projet et ses ports par défaut. Utiliser le parcours Compose direct de façon cohérente pour les exemples personnalisables ci-dessus ; ne pas mélanger les commandes de cycle de vie de projets distincts.

## Secrets, TLS et responsabilités

Créer les anciens credentials locaux ai-guard avec `init`, ou préparer une installation SSO neuve avec `setup prepare` selon son guide ; ne pas initialiser par-dessus un volume déjà préparé. Obtenir identité d'entreprise et credentials IA par les parcours opérateur/fournisseur choisis. Un administrateur Keycloak n'est pas automatiquement administrateur ai-guard. Le contrôle administre chaque rattachement tenant et permission. Un backend modèle partagé conserve des périmètres authentifiés, permissions et quotas distincts.

Pour préparer les certificats locaux, consulter le [générateur Rust d'AC](../demo-pki.fr.md). Garder les clés privées d'AC dans le contrôle de confiance ; transmettre uniquement la chaîne publique utile aux clients. Configurer SAN/issuer de façon cohérente et conserver la vérification TLS. Les exemples HTTP locaux utilisent des comptes publics de découverte ; ce ne sont pas des réglages de transport d'entreprise.
