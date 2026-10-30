# Diagnostic TUI Codex hors ligne — commandes locales

## Résultat

**BLOCKED avant ouverture du TUI; les commandes `!` et slash sont NOT_RUN.** Une seule invocation de worker jetable a été tentée, sans prompt positionnel, sans clé/API envfile, sans cache hôte, avec Docker `network=none` et un fournisseur custom sans `env_key`. Le client a quitté avant l’ouverture du composeur : il signale que `CODEX_HOME=/tmp/ai-guard-offline-codex-home` n’existe pas, puis `Error finding codex home`. Aucun caractère clavier ni prompt naturel n’a été envoyé; aucune requête modèle n’a été intentionnellement déclenchée. Le réseau du conteneur était désactivé. Le test ne prouve donc pas le comportement local de `!`, `/warnings` ou `/debug-config`, et ne permet pas d’attribuer la cause du blocage précédent.

La correction de préparation la plus étroite serait de ne pas fournir le `CODEX_HOME` personnalisé (le conteneur n’a aucun montage de home hôte) ou de s’assurer qu’un répertoire privé existe avant le démarrage. Aucune seconde session n’a été lancée dans ce lot.

## Configuration et identité

- Broker : `/tmp/ai-guard-interactive-qualified.hO9L2x/agentsandbox`, SHA-256 `7a86a4ffb494c706de197931ecfbd00da20be23263ce0d64bd60b5e692e1e3ed`.
- Image Codex déjà qualifiée : `agentsandbox-worker:qa-codex-0.165.0-20261030`, ID `sha256:42cfc041ee5daa0128da53da45c580e66aa1c17839b5013f053f7221d7179cab`; version connue `codex-cli 0.165.0` depuis la vérification hors ligne précédente.
- Session runner `3dcaf667-6fe7-4afd-b790-48a0ddbe5a1e`; worker `544c5d29-403f-4c87-88fa-fe0501c6b00b`; tentative `ff67caab-7cbc-4403-acde-12b6e65c7b57`.
- Invocation extérieure : `run --runtime docker --runtime-program /opt/homebrew/bin/docker --runtime-endpoint unix:///Users/louis/.docker/run/docker.sock --allow-rootful-runtime --profile observe --network none --provider generic --interactive --timeout 90` avec l’image ci-dessus et deux fichiers canaris texte seulement.
- Programme du worker : `/usr/bin/env CODEX_HOME=/tmp/ai-guard-offline-codex-home codex --no-alt-screen --strict-config --model gpt-6-luna --sandbox danger-full-access --ask-for-approval never -c 'model_provider="ai_guard_offline"' -c 'model_providers.ai_guard_offline.name="ai-guard offline provider"' -c 'model_providers.ai_guard_offline.base_url="http://127.0.0.1:9/v1"' -c 'model_providers.ai_guard_offline.requires_openai_auth=false' -c 'model_providers.ai_guard_offline.wire_api="responses"'`.
- Environnement appelant : `CODEX_API_KEY`, `OPENAI_API_KEY` et `OPENAI_BASE_URL` retirés; aucun `--secret-env-file`. Le fournisseur custom n’a pas de `env_key`; aucun faux serveur n’a été démarré.
- Docker 29.2.1; gateway privée `127.0.0.1:48793`, arrêtée proprement après les vérifications.

## Faits observés

- Le premier état TUI n’a produit aucune sortie (0 octet). Le poll suivant a produit 382 octets et le message d’erreur sur le chemin `CODEX_HOME` manquant; le retour final du runner a produit 529 octets. Les seules données conservées en mémoire étaient bornées; aucun transcript brut n’a été écrit.
- Résultat runner : exit 1, `timed_out=false`, `changed_files=0`, `output_captured=false`; le conteneur a été supprimé, sans erreur de nettoyage. Les deux reçus canaris étaient absents des sorties observées.
- Statut worker terminal, phase de reçu initiale, quota libéré, nettoyage confirmé. Audit valide, 24 événements, tête `b1a66245296df131e35ab471717e457faac63e3762a8158c7ef77abe5f59569d`.
- Usage : exactement une allocation observée correspondant à la tentative, `billable=false`, runtime Docker, méthode 1. Le quota final est nul.
- Aucun changement du projet copié; aucun export n’était nécessaire. Aucun modèle, endpoint fournisseur, cache hôte ou credential n’a été utilisé.

## Lecture documentaire (build exact non confirmé)

La documentation officielle indique que `!` exécute une commande locale sous la sandbox et les approbations actives; elle liste `/warnings` pour revoir les diagnostics conservés, mais pas de `/debug` générique. `/debug-config` traite des couches de configuration. Ces pages ne sont pas versionnées pour la CLI 0.165.0, et l’aide locale de cette version ne présentait pas ces commandes; leur disponibilité dans ce build reste non qualifiée. [Raccourcis CLI](https://learn.chatgpt.com/docs/cli/reference#interactive-shortcuts), [commande `/warnings`](https://learn.chatgpt.com/docs/developer-commands#inspect-warnings), [liste des commandes slash](https://learn.chatgpt.com/docs/developer-commands#built-in-slash-commands).

## Tentative hors ligne 2 — commandes locales observées

Cette section complète l’échec de préparation précédent; elle ne le remplace pas. Après validation read-only que `/home/agent` existe dans l’image et que la version est `codex-cli 0.165.0`, le worker a été lancé avec `HOME=/home/agent`, `CODEX_HOME` retiré, sans clé/envfile et avec `network=none`. Aucun prompt positionnel ni texte naturel n’a été envoyé. Le fournisseur custom `ai_guard_offline` était sans `env_key`, `requires_openai_auth=false`, avec base URL `http://127.0.0.1:9/v1`; aucun serveur n’était démarré.

### Observations

- **`!` local — PASS pour l’exécution/affichage local.** Après chaque texte `! /bin/cat PROBE_ONE.txt` et `! /bin/cat PROBE_TWO.txt`, une attente de 700 ms, puis un CR distinct, le TUI a affiché respectivement `LOCAL-CAT-RECEIPT-ONE-6V8Q` et `LOCAL-CAT-RECEIPT-TWO-2M9C`, puis le résultat `Worked for <1s` et le composeur `Ask Codex to do anything`. Ces reçus n’étaient pas dans les commandes tapées. `/status` a affiché `Token usage: 0 total (0 input + 0 output)`. Docker `network=none` empêchait un appel réseau extérieur; aucun prompt modèle n’a été soumis.
- **`/warnings` — PASS pour l’ouverture du panneau local.** Le panneau a été détecté après soumission de `/warnings`; le pied de page signalait `1 warning`. Le détail du diagnostic n’a pas été examiné ni conservé. Une touche Escape a été envoyée pour fermer le panneau.
- **`/exit` — sortie propre NOT_MET.** Le texte `/exit`, une attente de 700 ms, puis CR séparé ont été envoyés. Le delta suivant ne contenait que 31 caractères de contrôle/restauration terminal; aucun texte `Shutting down` n’a été observé. Le poll suivant a retourné le résultat runner après expiration de 90 s. La session avait affiché le compteur de jetons nul, mais aucun relevé d’écran frais ne permet d’établir si une modal `/status` était encore ouverte au moment de la saisie `/exit`. La raison de la non-fermeture propre demeure inconnue; aucun défaut broker n’en est déduit.
- Le runner a fini en exit 137, `timed_out=true`, `changed_files=0`, `output_captured=false`; le conteneur a été supprimé sans erreur. Ce timeout qualifie l’expiration du worker, pas la réussite du `/exit`.

### Identifiants et vérifications finales

- Session runner `9d65dfb7-4c8e-499f-b51a-8f6e5d056af4`; worker `7d46be0d-e7f9-48a1-a0bf-4be2704edb5a`; tentative `1eeba7ce-10c9-47c2-9e25-bbdfe83b8b98`.
- Broker SHA identique au gel précédent: `7a86a4ffb494c706de197931ecfbd00da20be23263ce0d64bd60b5e692e1e3ed`; image ID `sha256:42cfc041ee5daa0128da53da45c580e66aa1c17839b5013f053f7221d7179cab`.
- Projet fixture final: `PROBE_ONE.txt` SHA-256 `370b552a472f9c5a9c7a99207178222ae5fbfcc85f39192e5a1ad917277d5ff8`; `PROBE_TWO.txt` SHA-256 `8c31036de75b0f33594c003a3cc9c14665a3754cb08da26ab0bf5ecc9304f6ac`. `changed_files=0`; aucun export requis.
- Audit valide, 25 événements, tête `1ed417cc8a81cd97ce2c314aee61d4f73e8a287dc56a41c2a36c250bbc2d52ba`. Usage: une allocation observée correspondante, `billable=false`; statut final `quota_held=false`, cleanup terminé. Gateway arrêtée proprement.
- La collecte TUI est restée en mémoire et bornée à 6000 caractères; seuls états, longueurs et reçus canaris ont été rendus visibles. Aucun transcript brut n’a été écrit.

La première tentative de ce rapport reste documentée comme un échec de préparation dû au chemin CODEX_HOME absent. La seconde montre que les gestes `!` et `/warnings` fonctionnent localement dans cette image 0.165.0. La fermeture propre `/exit` demeure non qualifiée; aucun nouvel essai n’est inclus.
