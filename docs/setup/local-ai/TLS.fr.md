# Juge conteneurisé avec TLS vérifié

[English](TLS.md) · [IA locale](README.fr.md) · [Certificats de test](../../demo-pki.fr.md)

Choisir ce profil **à la place** du projet modèle HTTP. Gateway → relais Rust passe en HTTPS vérifié ; relais → `http://model:8080` utilise un réseau interne séparé et **n'est pas chiffré**. Aucun port modèle n'est publié. Ce profil vise une installation locale de confiance avec avis modèle consultatifs, pas un service d'inférence d'entreprise authentifié. Le transport du relais d'entrée contrôle/dashboard reste inchangé.

Choisir explicitement entre volumes POSIX locaux neufs (ci-dessous) et [binds hôte](#alternative--binds-hôte-sur-filesystems-supportés). Les projections hôte Docker Desktop peuvent être refusées par le lecteur strict ; ne pas affaiblir ses contrôles ni changer silencieusement de profil. Le parcours volumes importe des snapshots sans monter les sources TLS hôte chez les consommateurs. Qualification et écarts restent dans [engineering](../../engineering/CURRENT_STATE.md) ; ces instructions seules ne qualifient ni runtime, ni Podman, cloud, SSO ou qualité modèle.

## Préparer les images et sources privées

Suivre d'abord le [guide poste de travail](../local/README.fr.md) : initialiser la configuration privée hors checkout et définir `AIGUARD_CONFIG`. Conserver mêmes compte moteur, contexte et endpoint partout. À la racine du dépôt, construire ou importer les images de confiance avant activation :

```sh
cargo build --release --locked
AIGUARD_BIN="$PWD/target/release/agentsandbox"
"$AIGUARD_BIN" build --runtime docker --source .
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml build model
docker image inspect --format '{{.Id}}' agentsandbox-control:local
docker image inspect --format '{{.Id}}' ai-guard-local-ai:qwen2.5-0.5b-q4km
```

Relever les deux identifiants exacts `sha256:`. Pour un daemon Docker Desktop délibérément non rootless, ajouter `--allow-rootful-runtime` aux commandes Rust `build` et `model-tls provision` ; cela reste une dégradation de sécurité explicite. Avec un programme moteur/endpoint Unix approuvés particuliers, passer systématiquement `--runtime-program /absolute/path/to/runtime` et `--runtime-endpoint unix:///absolute/path/to/socket`, et configurer Compose sur cette même autorité. Aucun socket moteur n'est monté dans une application.

Créer des chemins **neufs** ; le sous-shell s'arrête sur erreur et refuse l'écrasement :

```sh
(
  set -eu
  umask 077
  mkdir -m 700 "$AIGUARD_CONFIG/local-judge-tls-volumes"
  ./target/release/ai-guard-demo-pki --out "$AIGUARD_CONFIG/local-judge-tls-volumes/issued" --dns llm.ai-guard.test
  set -C
  cat .env.exemple.local-qwen-tls-volumes > "$AIGUARD_CONFIG/local-qwen-tls-volumes.env"
)
```

Vérifier issuer, SAN, validité et empreinte racine publics selon le [guide des certificats de test](../../demo-pki.fr.md). L'outil local donne sept jours de validité leaf par défaut, sans installation de confiance système ni renouvellement/révocation automatiques. Pour une autre PKI, utiliser une identité serveur approuvée ; ne jamais importer de clé privée d'AC. Les trois entrées doivent être privées0600 sous des parents privés0700, appartenir au compte de contrôle sur un filesystem supporté, sans liens ni ACL non supportées. Ne pas relâcher ces originaux pour permettre leur lecture en conteneur.

Le nom serveur est un alias DNS ASCII minuscule, avec des labels de1 à63 caractères et253 caractères au total au maximum. Les IP, labels vides, tirets initiaux/terminaux, `localhost` et ses sous-domaines sont refusés. La topologie fournie réserve aussi `model`, `gateway`, `dashboard`, `ingress` et `local-ai-ingress` ; réutiliser ces noms de service rendrait le routage ambigu. Employer le même nom accepté dans le certificat et les sélecteurs.

Adapter le nouveau `local-qwen-tls-volumes.env` : `AGENTSANDBOX_CONFIG_DIR` correspond à `init` ; remplacer `AI_GUARD_LOCAL_AI_IMAGE` par l'identifiant modèle `sha256:` inspecté. Garder contexte512, un slot d'inférence, CPU2 et mémoire/mémoire+swap modèle1 000 000 000octets sauf reconfiguration et mesure délibérées. Le relais possède son propre budget192MiB. Taille des poids ou santé réussie ne prouvent ni pic RAM ni qualité du jugement. Ne jamais reconstruire le Dockerfile par défaut sur un tag modèle personnalisé.

## Préparer explicitement des volumes neufs

Remplacer la valeur publique ci-dessous par l'identifiant contrôle inspecté. Le répertoire de sortie ne doit pas exister ; son parent est privé. Cette commande prépare matériaux et sélecteurs sans lancer modèle, gateway, worker, réseau ou requête de décision :

```sh
AIGUARD_CONTROL_IMAGE_ID="sha256:replace-with-inspected-control-image-id"
"$AIGUARD_BIN" model-tls provision \
  --ca-file "$AIGUARD_CONFIG/local-judge-tls-volumes/issued/root-ca.pem" \
  --cert-file "$AIGUARD_CONFIG/local-judge-tls-volumes/issued/server-chain.pem" \
  --key-file "$AIGUARD_CONFIG/local-judge-tls-volumes/issued/server.key" \
  --server-name llm.ai-guard.test \
  --image "$AIGUARD_CONTROL_IMAGE_ID" \
  --output-dir "$AIGUARD_CONFIG/local-judge-tls-volumes/selection" \
  --runtime docker
```

La réussite publie ensemble `manifest.json` et `volumes.env` dans le nouveau répertoire `selection`. Conserver le manifeste privé avec la configuration. Le fichier de sélection contient exactement :

| Variable | Consommateur |
|---|---|
| `AI_GUARD_LOCAL_AI_TLS_CA_VOLUME` | Volume AC externe : gateway et relais, lecture seule à `/run/ai-guard-trust/root-ca.pem`. |
| `AI_GUARD_LOCAL_AI_TLS_IDENTITY_VOLUME` | Volume identité externe : relais seulement, lecture seule à `/run/ai-guard-tls/server-chain.pem` et `server.key`. |
| `AI_GUARD_LOCAL_AI_TLS_SERVER_NAME` | Alias réseau relais, URL HTTPS gateway et URL de santé relais ; validé contre le certificat. |
| `AI_GUARD_LOCAL_AI_TLS_NETWORK` | Nouveau réseau interne `ai-guard-judge-UUID`, créé ensuite par le projet Compose modèle. |
| `AI_GUARD_CONTROL_IMAGE` | Identifiant immuable inspecté de l'image contrôle utilisée pour préparateur, relais et pile contrôle. |

Aucune clé, credential, source TLS ou empreinte de clé privée dans ces sélecteurs. Ni modèle ni workers ne reçoivent ces volumes. Le gateway ne reçoit jamais l'identité serveur. Les clés privées d'AC restent chez l'opérateur, sans import. Audit, runner et viewer restent séparés des matériaux TLS modèle.

Échec ou timeout conserve volumes neufs et identifiants de diagnostic. Arrêt ou publication non confirmé reste **inconnu**, sans activation. Conserver toutes les ressources retenues et examiner le runtime exact enregistré ; ne pas fabriquer de sélecteur, adopter un volume existant, réparer des droits, réessayer la même sortie ou effacer des preuves pour obtenir une réussite. Une tentative ultérieure emploie nouvelles sources/sortie et nouveaux volumes après rapprochement. Installations et volumes antérieurs sont préservés.

## Lancer et diagnostiquer

Arrêter d'abord uniquement un éventuel projet modèle HTTP ou bind actif avec sa configuration originale ; ne pas lancer un second modèle. Rapprocher les opérations actives avant modification de la pile contrôle. Conserver le **nom original du projet contrôle** et son volume d'audit : les commandes utilisent `ai-guard-local`. Passer env de base **en premier**, puis `volumes.env` publié, qui fournit les cinq sélecteurs. Ne sourcer aucun fichier. Retirer les réglages `AI_GUARD_*`/`JEV_*` hérités conflictuels : le shell peut primer sur l'interpolation Compose. Choisir exactement une surcharge juge.

```sh
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local-ai-tls-volumes -f deploy/local-ai/compose.tls-volumes.yaml config --quiet
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local-ai-tls-volumes -f deploy/local-ai/compose.tls-volumes.yaml up --no-build --pull never -d
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local-ai-tls-volumes -f deploy/local-ai/compose.tls-volumes.yaml ps
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls-volumes.example.yaml config --quiet
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls-volumes.example.yaml up --no-build --pull never -d
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls-volumes.example.yaml exec -T gateway agentsandbox healthcheck --ca-file /run/ai-guard-trust/root-ca.pem --url https://llm.ai-guard.test:8443/health
```

Le DNS d'exemple est `llm.ai-guard.test` ; pour un autre nom préparé, employer ce même nom dans l'URL de santé manuelle. Alias Compose, URL gateway et healthcheck automatique du relais interpolent le sélecteur. `up --no-build --pull never` exige les images locales exactes préparées ; image absente signifie refus, sans téléchargement ni reconstruction à l'activation.

Le projet modèle crée `judge_link` avec `internal: true` ; la surcharge contrôle rejoint ce réseau existant. Son `external: true` désigne le propriétaire dans un autre projet Compose, pas un accès Internet. Vérifier réellement attribut Internal et participants : gateway=control+judge_link, relais=judge_link+inference, modèle=inference seulement ; aucun worker. Les deux volumes TLS doivent être les volumes locaux du manifeste, montés en lecture seule chez les consommateurs prévus, jamais des binds. Consulter `ps` et si nécessaire un inspect moteur borné sans révéler de données privées. Ne jamais publier clés brutes, fichiers sources ou diagnostics arbitraires.

Le diagnostic dans le gateway vérifie le vrai HTTPS et la santé du modèle amont. `/healthz` du gateway seul ne sonde pas cette route. Le relais accepte seulement `GET /health` et `POST /v1/chat/completions` JSON non streaming sans querystring :64KiB requête/réponse, une inférence,429 immédiat à saturation, lecture corps2s/amont5s et aucun retry. AC absente, mauvais SAN, expiration, OOM, modèle non sain, filesystem refusé ou réseau absent restent des échecs explicites ; corriger la cause sans désactiver TLS ni élargir les réseaux. Modèle lent/malformé/indisponible ferme la décision. Allow local devient Review effectif ; Deny reste un refus. Aucun avis local ne lève un refus impératif Rust/audit/runtime.

Pour Podman, remplacer `--runtime docker` par `--runtime podman`, `docker` par `podman` et seulement le socle contrôle par `deploy/podman/compose.yaml`. Conserver ordre env, identifiants image, endpoints et noms de projets ; exiger un fournisseur Compose supportant ces options. Docker et Podman nécessitent des qualifications réelles séparées. Les observations Docker Desktop ne qualifient ni Podman, cloud, SSO, inférence intégrée ou sémantique modèle.

## Arrêter, reprendre, renouveler et retirer

Suspendre les admissions et rapprocher les opérations actives avant arrêt du contrôle. Employer mêmes deux fichiers env, projets et fichiers à chaque commande de cycle de vie, surcharge gateway comprise. Pour un arrêt/reprise planifiés :

```sh
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls-volumes.example.yaml stop
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local-ai-tls-volumes -f deploy/local-ai/compose.tls-volumes.yaml stop
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local-ai-tls-volumes -f deploy/local-ai/compose.tls-volumes.yaml start
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls-volumes.env" --env-file "$AIGUARD_CONFIG/local-judge-tls-volumes/selection/volumes.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls-volumes.example.yaml start
```

Refaire les deux `ps` et le diagnostic HTTPS dans le gateway après reprise. `stop` conserve conteneurs, volumes et réseaux. Une panne ferme les décisions concernées ; ne pas changer silencieusement de moteur. Si les conteneurs doivent être recréés, employer les mêmes commandes `up --no-build --pull never -d`. Ne pas retirer le réseau juge tant qu'un gateway l'utilise.

Pour renouveler, préparer une nouvelle génération de sources privées et importer dans un autre répertoire de sortie neuf. Conserver anciens manifestes, images, audit et sources. Arrêter/rapprocher l'activité concernée ; sélectionner le nouveau `volumes.env`, recréer relais et gateway avec mêmes projets, puis vérifier HTTPS. Les fichiers sont lus une fois ; aucun rechargement à chaud, rotation ou révocation automatique promis. Un retour délibéré sélectionne manifeste/images compatibles précédents et recrée les deux consommateurs ; vérifier validité et santé avant reprise. Une identité expirée ne permet pas de retour utilisable.

Le retrait est une action opérateur après examen rétention/sauvegarde : identifier les volumes exacts de cette génération dans son manifeste, vérifier qu'aucun conteneur ou autre installation ne les utilise, retirer seulement ses conteneurs arrêtés et réseau inutilisé par le `down` Compose correspondant **sans `-v`**, puis retirer uniquement ces deux noms de volumes TLS approuvés avec le `volume rm` ciblé du moteur. Ne jamais utiliser `prune`, `down -v`, supprimer un volume audit/workspace ou retirer automatiquement sources/manifestes. Si propriété, arrêt, publication ou usage reste inconnu, conserver la ressource et garder le nettoyage ouvert.

## Alternative : binds hôte sur filesystems supportés

Le [profil modèle bind](../../../deploy/local-ai/compose.tls.yaml), la [surcharge gateway](../../../deploy/docker/compose.jev-local-tls.example.yaml) et l'[exemple env](../../../.env.exemple.local-qwen-tls) existants restent une alternative explicite **uniquement lorsque le filesystem réel du conteneur est accepté par le lecteur TLS strict**. Les droits POSIX hôte seuls ne qualifient pas une projection Docker Desktop. Ne jamais réessayer un bind refusé en affaiblissant les contrôles.

Préparer un répertoire neuf distinct avec la même procédure PKI de test :

```sh
(
  set -eu
  umask 077
  mkdir -m 700 "$AIGUARD_CONFIG/local-judge-tls"
  ./target/release/ai-guard-demo-pki --out "$AIGUARD_CONFIG/local-judge-tls/issued" --dns llm.ai-guard.test
  mkdir -m 700 "$AIGUARD_CONFIG/local-judge-tls/mounted-tls"
  install -m 444 "$AIGUARD_CONFIG/local-judge-tls/issued/root-ca.pem" "$AIGUARD_CONFIG/local-judge-tls/mounted-tls/root-ca.pem"
  install -m 444 "$AIGUARD_CONFIG/local-judge-tls/issued/server-chain.pem" "$AIGUARD_CONFIG/local-judge-tls/mounted-tls/server-chain.pem"
  install -m 444 "$AIGUARD_CONFIG/local-judge-tls/issued/server.key" "$AIGUARD_CONFIG/local-judge-tls/mounted-tls/server.key"
  set -C
  cat .env.exemple.local-qwen-tls > "$AIGUARD_CONFIG/local-qwen-tls.env"
)
```

Seules les trois projections sont0444 sous leur parent privé0700 ; originaux0600 conservés. Ne jamais monter le répertoire d'émission. Adapter `AI_GUARD_LOCAL_AI_TLS_MATERIAL_DIR` au nouveau `mounted-tls` et sélectionner les identifiants contrôle/modèle inspectés dans cet env. Le profil bind utilise le SAN fixe `llm.ai-guard.test`. Employer un seul env (`local-qwen-tls.env`), projet modèle `ai-guard-local-ai-tls`, fichier modèle `deploy/local-ai/compose.tls.yaml` et surcharge contrôle `deploy/docker/compose.jev-local-tls.example.yaml`. Appliquer la même procédure config/lancement/arrêt/reprise ci-dessus avec ces substitutions explicites ; ne pas passer `volumes.env`. Son diagnostic dans le gateway utilise `--ca-file /run/ai-guard-trust/local-judge-ca.pem`. Vérifier même URL HTTPS et frontières réseau avant admissions. Le renouvellement prépare de nouvelles projections et recrée relais/gateway ; conserver anciens matériaux, images et audit. Cette alternative n'importe ni ne nettoie le profil volumes.
