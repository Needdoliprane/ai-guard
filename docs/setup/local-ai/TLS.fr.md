# Juge conteneurisé avec TLS vérifié

[English](TLS.md) · [IA locale](README.fr.md) · [Certificats de test](../../demo-pki.fr.md)

Choisir ce profil **à la place** du projet modèle HTTP. Il réutilise image épinglée et limites, sans deuxième modèle. Gateway → relais Rust passe en HTTPS vérifié ; relais → `http://model:8080` reste sur un réseau interne séparé et **n'est pas chiffré**. Aucun port modèle publié sur l'hôte. Relais d'entrée contrôle/dashboard inchangés. Ce profil vise une installation de confiance, pas un service d'inférence d'entreprise multi-utilisateur authentifié.

## Préparer les fichiers privés

Préparer binaires release, image contrôle et image modèle épinglée selon les guides locaux. Définir `AIGUARD_CONFIG` comme dans le guide poste de travail. À la racine du dépôt, créer un répertoire **neuf** ; ce sous-shell s'arrête sur erreur et refuse un répertoire existant :

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
)
(
  umask 077
  set -C
  cat .env.exemple.local-qwen-tls > "$AIGUARD_CONFIG/local-qwen-tls.env"
)
```

Vérifier issuer, SAN, validité et empreinte racine publics avant usage. L'outil de test donne sept jours de validité serveur par défaut ; aucun renouvellement ou service de révocation automatique. Originaux0600, parents hôte0700. Seules ces trois projections sont0444 sous leur parent privé0700 :

| Matériau | Consommateur |
|---|---|
| Racine publique | Confiance gateway ; diagnostic HTTPS du relais. |
| Chaîne serveur et clé leaf | Relais modèle seulement. |
| Clés privées d'AC racine/intermédiaire | Opérateur seulement, jamais projetées/montées. |

Ni modèle ni workers ne reçoivent ces fichiers. Ne pas monter le répertoire d'émission. Aucun magasin système ou DNS n'est modifié ; Compose fournit `llm.ai-guard.test`.

Adapter le nouveau fichier env : `AGENTSANDBOX_CONFIG_DIR` correspond à `init` ; `AI_GUARD_LOCAL_AI_TLS_MATERIAL_DIR` désigne `mounted-tls`, jamais `issued`. `AI_GUARD_LOCAL_AI_TLS_NETWORK` est identique dans les deux projets et distinct pour une autre installation. Les autres réglages modèle gardent leur sens HTTP. Le CLI ne source pas dotenv. Choisir une seule surcharge juge et retirer les réglages `JEV_*` de déploiement hérités conflictuels.

## Lancer et diagnostiquer

Si l'inférence HTTP tourne, arrêter seulement ce projet modèle en premier ; conserver images et données du contrôle. Garder le **nom original du projet contrôle** pour réutiliser son volume d'audit. L'exemple utilise le défaut documenté `ai-guard-local` :

```sh
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local-ai-tls -f deploy/local-ai/compose.tls.yaml config --quiet
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local-ai-tls -f deploy/local-ai/compose.tls.yaml up --no-build -d
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local-ai-tls -f deploy/local-ai/compose.tls.yaml ps
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls.example.yaml config --quiet
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls.example.yaml up --no-build -d
docker compose --env-file "$AIGUARD_CONFIG/local-qwen-tls.env" -p ai-guard-local -f deploy/docker/compose.yaml -f deploy/docker/compose.jev-local-tls.example.yaml exec -T gateway agentsandbox healthcheck --ca-file /run/ai-guard-trust/local-judge-ca.pem --url https://llm.ai-guard.test:8443/health
```

Si l'image modèle par défaut manque, construire explicitement `model` avant `up` ou importer l'image hors ligne. Ne pas reconstruire le Dockerfile par défaut sur un tag modèle personnalisé. Avec Podman utiliser `podman compose` et remplacer seulement le socle contrôle par `deploy/podman/compose.yaml`, mêmes env/projets. Support et effets réels exigent des qualifications Docker/Podman séparées.

Le projet modèle crée judge_link **internal** ; external côté gateway nomme son propriétaire Compose, pas un accès Internet. Vérifier réellement attribut Internal et participants : gateway=control+judge_link, relais=judge_link+inference, modèle=inference seulement ; aucun worker. Le relais d'entrée contrôle n'a aucun fichier TLS. Réseau absent ou mauvaise AC/SAN/validité restent des échecs explicites ; ne jamais ouvrir egress ou contourner TLS.

Le diagnostic s'exécute dans le gateway, vérifie HTTPS et joint la vraie santé modèle. `/healthz` du gateway seul ne sonde pas cette route. Seuls `GET /health` et `POST /v1/chat/completions` JSON non streaming sans querystring sont relayés. Limites :64KiB requête/réponse, une inférence et429 immédiat à saturation, lecture corps2s/amont5s, aucun retry. Avis lent, malformé ou absent ferme la décision ; Allow local reste Review effectif et n'autorise aucune exécution. Disponibilité du transport ne qualifie pas le jugement modèle.

## Arrêter, renouveler et reprendre

Employer mêmes env/projet/fichier modèle avec `stop` ou `start`. Conserver la surcharge gateway sur **chaque** commande du cycle contrôle. Une panne ferme les décisions concernées. Ne pas retirer un réseau partagé encore utilisé par le gateway ni supprimer les volumes d'audit.

Avant changement de certificats/politique, suspendre les admissions et rapprocher les opérations actives. Préparer un nouveau répertoire privé et sa référence env puis recréer relais et gateway ; fichiers lus une fois, sans rechargement à chaud. Refaire le diagnostic HTTPS dans le gateway. Conserver anciens matériaux/images/audit pour un retour délibéré. Ne pas désactiver TLS, élargir les réseaux ou changer silencieusement de juge. Vérifications exécutées et limites restent dans le [registre engineering](../../engineering/CURRENT_STATE.md).
