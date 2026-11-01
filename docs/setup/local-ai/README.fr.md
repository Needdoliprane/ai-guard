# IA locale : un petit backend d'inférence partagé

[English](README.md) · [Français](README.fr.md) · [Index de configuration](../README.fr.md)

Utiliser un modèle local pour l'inférence privée/hors ligne et l'analyse consultative. Le défaut est **Qwen2.5-0.5B-Instruct Q4_K_M**, servi par llama.cpp. C'est un modèle de génération de texte ; un modèle d'embeddings seul ne remplace pas une API conversationnelle ou de génération de décisions.

Le backend peut servir les profils d'inférence d'entreprise et de juge local, avec appelants, autorisations, contexte et quotas séparés. Il ne devient pas une autorité d'autorisation. Politique Rust obligatoire, audit et restrictions runtime restent indépendants. Un avis modèle peut être incomplet ou erroné.

## Prérequis et fichiers fournis

Préparer le [socle local ai-guard](../local/README.fr.md), dont `agentsandbox-control:local`. Le projet modèle utilise le même binaire d'entrée Rust et son propre réseau d'inférence isolé. Aucun socket moteur, projet original ou credential du contrôle n'est monté dans le modèle.

| Fichier | Rôle |
|---|---|
| [`.env.exemple.local-qwen`](../../../.env.exemple.local-qwen) | Image, ports et défauts CPU/RAM/contexte publics. |
| [`deploy/local-ai/compose.yaml`](../../../deploy/local-ai/compose.yaml) | Modèle et relais Rust à destination fixe. |
| [`deploy/local-ai/Dockerfile`](../../../deploy/local-ai/Dockerfile) | Runtime llama.cpp épinglé et poids GGUF vérifié par checksum. |
| [Exemple combiné](../../../.env.exemple.local-keycloak-qwen) | Mêmes réglages modèle avec ceux de Keycloak local. |
| [Exemple TLS](../../../.env.exemple.local-qwen-tls) | Route HTTPS interne du juge conteneurisé ; suivre le [guide TLS](TLS.fr.md). |

La première construction télécharge le runtime public et le poids de 491 400 032 octets. Révision et SHA256 sont fixés dans le Dockerfile. La construction demande `ADD --checksum` (frontend Dockerfile 1.6+) ; voir la [référence Docker](https://docs.docker.com/reference/dockerfile/#add---checksum). Préparer l'image sur une machine connectée, puis la transférer par le circuit habituel de distribution pour un usage hors ligne. Le démarrage du runtime ne télécharge aucun modèle.

## Configurer

| Paramètre | Défaut | Sens |
|---|---|---|
| `AI_GUARD_LOCAL_AI_IMAGE` | `ai-guard-local-ai:qwen2.5-0.5b-q4km` | Image construite localement contenant le modèle. |
| `AI_GUARD_LOCAL_AI_MODEL` | `qwen2.5-0.5b-q4km` | Alias modèle retourné par l'API. |
| `AI_GUARD_LOCAL_AI_PORT` | `18090` | Port hôte lié à 127.0.0.1. |
| `AI_GUARD_LOCAL_AI_MEMORY_BYTES` | `1000000000` | Mémoire et mémoire+swap demandées pour le conteneur modèle. |
| `AI_GUARD_LOCAL_AI_CPUS` | `2.0` | Limite CPU allouée, pas CPU consommé mesuré. |
| `AI_GUARD_LOCAL_AI_CONTEXT` | `512` | Taille du contexte ; l'augmenter accroît la mémoire nécessaire. |
| `AI_GUARD_LOCAL_AI_DEFAULT_TOKENS` | `64` | Longueur de prédiction par défaut, remplaçable par la requête. Ce n'est pas un quota obligatoire. |
| `AI_GUARD_CONTROL_IMAGE` | `agentsandbox-control:local` | Image du relais Rust de confiance. |

Le modèle utilise un slot, batch/ubatch 32, aucune couche GPU, UID 65532, rootfs en lecture seule, capabilities supprimées, no-new-privileges et limite de PID 64. Le relais possède une limite distincte de 192 MiB ; le budget modèle de 1 Go n'est pas la mémoire de toute l'installation. Les deux services désactivent les logs du moteur. Une configuration de ressources ne garantit pas qu'un modèle arbitraire tienne en mémoire. Le [réglage de requête llama.cpp](https://github.com/ggml-org/llama.cpp/blob/b11718/tools/server/server-context.cpp#L1801-L1802) prime sur le défaut de prédiction ; les quotas de sortie obligatoires relèvent de l'adaptateur Rust autorisé.

## Construire et démarrer

Depuis la racine du dépôt :

```sh
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml config --quiet
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml build model
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml up --no-build -d
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml ps
```

Attendre le healthcheck du relais, qui interroge le véritable endpoint de santé du modèle. Diagnostiquer l'état avant une inférence ; `running` ne suffit pas à établir la disponibilité.

```sh
./target/release/agentsandbox healthcheck --url http://127.0.0.1:18090/health
```

Pour le contexte combiné, remplacer seulement le chemin env par `.env.exemple.local-keycloak-qwen` et garder ce nom de projet modèle. Keycloak et ai-guard conservent leurs noms et réseaux propres.

## API et profils applicatifs

L'endpoint local est `http://127.0.0.1:18090/v1`, alias `qwen2.5-0.5b-q4km`. Ce profil expose **Chat Completions** ; « compatible OpenAI » ne signifie pas toutes les API OpenAI, Codex Responses, outils, images ou Claude Messages.

Requête manuelle facultative et bornée, avec uniquement un prompt synthétique public :

```sh
curl --fail --silent --show-error --max-time 30 --noproxy '*' \
  http://127.0.0.1:18090/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"qwen2.5-0.5b-q4km","messages":[{"role":"user","content":"Say hello briefly."}],"max_tokens":16,"temperature":0,"stream":false}'
```

Un profil d'inférence administré configure protocole exact, URL, modèle, référence de credential si nécessaire, chaîne publique d'AC pour HTTPS, destinations autorisées et limites par appelant. Sélectionner l'adaptateur Rust correspondant au protocole client ; ne pas rediriger Codex ou Claude vers une URL arbitraire en supposant une équivalence. Séparer appelants juge local et entreprise, même avec des poids communs. Abonnement fournisseur et SSO ai-guard restent indépendants du backend.

Le profil local ne demande aucun credential API modèle sur le loopback. Ne pas publier ce port sur le LAN ni reprendre ce transport comme endpoint d'entreprise multi-utilisateur. Le raccord entreprise ajoute authentification, autorisation et TLS selon le [guide entreprise](../enterprise/README.fr.md).

### Raccorder un juge local

Choisir explicitement le backend `local-openai` du gateway ; son URL comprend `/v1/chat/completions`. Il accepte une réponse assistant non streamée contenant exactement `outcome` (`allow`, `deny`, `review` ou `unknown`) et `reason`. Réponses malformées, tronquées et appels d'outils sont refusés. Un Allow local reste consultatif et devient Review effectif : il ne peut autoriser une exécution. Le [guide du moteur de décision](../providers/README.fr.md#choisir-le-moteur-de-décision) explique décisions effectives et provenance.

Pour un gateway sur le même hôte que la publication loopback du modèle, un contexte d'intégration isolé utilise son propre chemin d'audit privé et son port. Depuis la racine du dépôt, avec les variables `AIGUARD_BIN` et `AIGUARD_CONFIG` du guide local :

```sh
"$AIGUARD_BIN" gateway \
  --listen 127.0.0.1:18787 \
  --data "$AIGUARD_CONFIG/local-judge-audit.sqlite" \
  --runner-token-file "$AIGUARD_CONFIG/secrets/runner" \
  --viewer-token-file "$AIGUARD_CONFIG/secrets/viewer" \
  --policy "$PWD/policies/default.json" \
  --jev-backend local-openai \
  --jev-local-url http://127.0.0.1:18090/v1/chat/completions \
  --jev-local-model qwen2.5-0.5b-q4km \
  --jev-allow-local-http
```

Utiliser l'URL gateway explicite de ce contexte pour les appelants ; son historique d'audit est distinct. Conserver base et services de la pile existante. Retirer les réglages des autres backends Jev de l'environnement avant ce lancement. Ce point d'entrée modèle n'a pas de clé : l'option de fichier de clé locale est inutile. Un endpoint HTTPS authentifié peut utiliser un fichier privé de clé brute distinct via `--jev-local-key-file`.

Le loopback est relatif au processus appelant. Un gateway conteneurisé n'atteint pas la publication hôte avec son propre `127.0.0.1` ; ajouter le Compose modèle ne raccorde pas les applications. Un déploiement conteneurisé exige route explicitement approuvée et confiance HTTPS vérifiée. Ne pas ouvrir HTTP sans restriction ni désactiver TLS pour franchir cette frontière. Commandes hôte, contrats d'adaptateur, mesures du modèle et déploiement conteneurisé complet ont des preuves de qualification distinctes dans l'[état engineering](../../engineering/CURRENT_STATE.md).

## Remplacer le modèle et passer hors ligne

Pour un gateway conteneurisé, le [profil TLS local](TLS.fr.md) fournit une route avec AC privée, lancement, renouvellement et reprise. Il remplace le projet modèle HTTP sans lancer un deuxième modèle.

Choisir un modèle génératif/instruct d'au plus 4 milliards de paramètres et mesurer sa mémoire réelle avec la quantification et le contexte choisis. Construire une image distincte avec runtime, révision de poids, checksum et licences applicables épinglés. Conserver l'entrypoint `/app/llama-server` et le fichier `/models/model.gguf` pour ce Compose, ou fournir un Compose relu pour un autre runtime.

Régler `AI_GUARD_LOCAL_AI_IMAGE` sur l'image préparée et `AI_GUARD_LOCAL_AI_MODEL` sur son alias. Utiliser `up --no-build` ; construire le Dockerfile par défaut avec un tag personnalisé remplacerait son contenu par le modèle par défaut. Conserver l'image précédente pour un retour délibéré. Transférer les images via registre contrôlé ou archive avant de passer hors ligne ; changer un alias ne change pas les poids.

## Diagnostiquer et exploiter

Examiner `jev.error` et, lorsqu'il existe, `jev.diagnostic` dans la décision ou son contenu d'audit autorisé. Ce diagnostic décrit la phase `send`/`response_body` de l'endpoint configuré, une catégorie fermée et un statut HTTP observé. Un502 de relais ne prouve pas le statut initial du modèle. Ce diagnostic ne contient ni corps amont, URL, header ou credential ; la configuration de transport préexistante reste inchangée ; l'absence de diagnostic ne signifie pas une évaluation réussie. Voir le [contrat API](../../api.md#diagnostic-de-transport-jev--jev-transport-diagnostics).

L'adaptateur local demande au maximum256tokens de sortie, température0, sans streaming et formatJSON ; `--predict` reste le défaut serveur pour les appels sans limite explicite. Le fingerprint comprend ces paramètres fixes et le hash du prompt système. Un masque de secret qui modifierait le prompt fixe ou la structure JSON provoque `invalid_configuration` avant POST ; le contenu utilisateur reste masquable. Ces diagnostics ne changent ni délai5s, ni règles de retry/repli et ne qualifient pas le jugement du modèle. Préserver l'échec initial et diagnostiquer avant un nouvel essai explicite.

- **Image absente ou échec poids/construction :** préparer l'image exacte. Un checksum incorrect doit rester un refus.
- **OOM ou serveur non sain :** examiner état et limites mémoire ; réduire contexte/sortie/concurrence ou choisir un modèle plus petit. Augmenter la RAM est un choix explicite.
- **Protocole incompatible :** choisir adaptateur/modèle compatible ; aucun repli silencieux vers un fournisseur cloud.
- **Timeout :** borner requêtes et tokens. Le relais Rust borne aussi les attentes des en-têtes/corps ; une génération arbitrairement longue n'est pas promise par ce preset.
- **Nom TLS ou AC incorrecte :** corriger SAN/nom d'endpoint et chaîne publique ; ne jamais désactiver la vérification.

Arrêter ou reprendre uniquement ce projet :

```sh
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml stop
docker compose --env-file .env.exemple.local-qwen -p ai-guard-local-ai -f deploy/local-ai/compose.yaml start
```

Aucun volume mutable de données modèle n'est à supprimer. Conserver séparément images, audit ai-guard et données Keycloak. Un contrôle de disponibilité ne mesure pas la qualité du jugement ; un petit modèle ne doit jamais autoriser seul une action.
