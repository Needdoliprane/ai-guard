# Codex TUI réel — session bornée du 2026-10-30

## Résultat

**INCOMPLETE / FAIL du critère deux tours + export.** Codex TUI a démarré dans le worker Docker épinglé, avec l’image CLI `codex-cli 0.165.0`; l’interface affichait le modèle demandé GPT-6-Luna. Le premier tour a lu `FIRST_TURN.txt`, exécuté la création de `result.txt` avec la ligne `turn one: complete`, puis affiché le receipt propre à la fixture. Le TUI est revenu au composeur idle ; le receipt n’était pas dans le prompt utilisateur. Cela prouve un premier tour réel. Le compte/modèle servi ne sont pas indépendamment attestés.

Un unique second message utilisateur court a été saisi. Le composeur n’a pas présenté une transition d’activité/fin de tour sans ambiguïté ; aucun nouvel output TUI n’est apparu pendant environ 40 secondes alors que le registre restait running et quota-held. Aucun resend ni troisième message n’a été envoyé. Pour conserver la preuve, `worker-stop` a persisté une intention stop sur cette session possédée. Le runner s’est terminé en code 137 (`timed_out=false`, `changed_files=1`, `output_captured=false`, `postprocessing_error=null`); cleanup a supprimé le conteneur. La sortie terminale et le statut consignent l’intention stop, mais pas une mesure terminale clean avec code 0.

Au moment de l’arrêt, aucun export ni lecture post-stop n’a été effectué; la mission de suivi ci-dessous a ensuite autorisé explicitement ces lectures et l’export du premier tour. Donc le fichier final deux lignes reste **NOT_RUN** et le critère de conversation à deux tours reste **FAIL/INCOMPLETE**, même si l’audit, l’allocation et le quota ont depuis été vérifiés. Le résumé `changed_files=1` ne prouve ni contenu ni destination exportée.

## Environnement privé et sécurité

- Binaire broker `/tmp/ai-guard-interactive-qualified.hO9L2x/agentsandbox`; SHA-256 `7a86a4ffb494c706de197931ecfbd00da20be23263ce0d64bd60b5e692e1e3ed`.
- Image worker `agentsandbox-worker:qa-codex-0.165.0-20261030`; ID `sha256:42cfc041ee5daa0128da53da45c580e66aa1c17839b5013f053f7221d7179cab`.
- Gateway Rust privé lié à `127.0.0.1:48789`, SQLite/config/secrets sous `/tmp/ai-guard-codex-tui.nyY5EF`. Audit avant run : valide, un événement. Gateway arrêté proprement après l’arrêt de session.
- Session `f45f0e71-c364-4732-b567-fb02b6d1ffcf`, worker `558f4ce6-d7c7-4c70-8cdb-a8cf05ba9da2`, tentative `b305f754-3b1f-48d3-a431-926de246f950`. Preset interactif V3, profil observe, réseau bridge, timeout 300 s, limites Docker existantes.
- Une seule invocation broker, aucun retry. Auth était configurée par le custom provider `ai_guard_api` et `CODEX_API_KEY` via le fichier secret privé avec `--secret-env-file`; aucune valeur, `auth.json`, clé ou variable d’environnement complète n’a été lue/affichée/copiée dans le rapport ou l’argv. Premier tour réussi; le mode d’auth exact n’est pas attesté indépendamment. Aucun montage de `~/.codex` ni cache hôte.
- Projet fixture original : hashes initiaux des instructions dans `/tmp/ai-guard-codex-tui.nyY5EF/fixture-hashes-before.txt`; ils restent identiques. `result.txt` est absent du projet original. Les instructions restent dans le répertoire temporaire privé; l’unique destination d’export est /tmp/ai-guard-codex-tui.nyY5EF/export-first-turn et contient le résultat partiel du premier tour.
- Le TUI a montré un avertissement non inspecté. Pas de transcript/écran brut archivé ; logs du gateway et statut privé restent sous le répertoire protégé. Pas de modèle supplémentaire, browser, E2E ou changement source.

## Limite opérationnelle

Le blocage précis est la confirmation d’envoi/état idle du second message depuis le composeur TUI : les caractères de soumission n’ont pas produit de confirmation utilisateur discernable. L’arrêt est conservé comme échec/incomplet. Ne pas déduire le succès du fichier créé au premier tour, de `changed_files=1`, du nom du modèle dans le footer ou de la seule intention de stop. Toute nouvelle invocation exige une correction/qualification du transport clavier ou un protocole TUI fiable, sans réutiliser ni relancer automatiquement cette session.

## Collecte read-only/export après arrêt

Après le stop, la même gateway privée a été redémarrée sur la même origine `127.0.0.1:48789`, la même DB SQLite et les mêmes fichiers de credentials locaux. Aucun `init`, nouvel worker ou redémarrage du conteneur n’a eu lieu.

- **PASS état terminal durable / quota** : lecture `worker-status` de la session originale renvoie terminal, `quota_held=false`, révision 9. Résultat durable : exit 137, timeout false, `changed_files=1`, capture sortie false, cleanup `removed`, aucune erreur cleanup/post-traitement. Sorties worker restent non capturées.
- **PASS intégrité d’audit** après lecture et export : valide, 33 événements, tête `d161fe7fd6f037e21ced477bc02aea00eddeb6b7102d317e3372c71c18498518`.
- **PASS allocation** : une seule allocation correspond au worker `558f4ce6-d7c7-4c70-8cdb-a8cf05ba9da2` et tentative `b305f754-3b1f-48d3-a431-926de246f950`; `billable=false`, méthode 1, runtime docker, provenance observed. Lecture `api/usage?limit=100`; résumé seulement, aucun corps API persisté.
- **PASS export d’une destination neuve** : commande `agentsandbox export-workspace --runtime-program /opt/homebrew/bin/docker --session f45f0e71-c364-4732-b567-fb02b6d1ffcf --output /tmp/ai-guard-codex-tui.nyY5EF/export-first-turn --config-dir /tmp/ai-guard-codex-tui.nyY5EF/config --gateway http://127.0.0.1:48789` a exporté trois fichiers vérifiés. `result.txt` contient exactement `turn one: complete\n` (19 octets, une ligne), SHA-256 `8142525ba5170feffdc28c24f22b31f8b911ea952142db58ca55a3371c910283`. Cela prouve l’effet du premier tour uniquement, pas le livrable deux lignes.
- **PASS original intact** : les deux fichiers d’instruction correspondent toujours aux SHA initiaux (`e59f5ccfd151d14452203208591353288a416ea25ae32762c80832120b47afcb`, `b135e0240968e53296bcb988d6d4cb0b289eed9f54afc53f836fe3feadbf3e74`); aucun `result.txt` dans le projet original. L’export est la nouvelle destination seulement.
- Le dernier octet destiné à soumettre le deuxième prompt était LF `0x0A`; auparavant un CR `0x0D` avait été envoyé et n’avait pas donné de transition visible. La documentation officielle décrit le submit par `Enter`/`Ctrl-M` dans le keymap TUI ([configuration de base](https://learn.chatgpt.com/docs/config-file/config-basic)). Ce fait ne démontre pas une incompatibilité broker et ne justifie pas de remplacer génériquement LF par CR. Le deuxième tour reste incomplet/ambigu.
- La gateway privée a été arrêtée après ces lectures. Aucun stop/delete additionnel de worker, retry, nouvelle session, ou lecture de transcript brut.

### Scan ciblé du secret

Un scanner Rust temporaire a lu la clé depuis le fichier privé en mémoire et vérifié le littéral sur ce rapport ainsi que les deux instructions et `result.txt` exporté : **4 fichiers inspectés, 0 hit**. Le scanner n’imprime ni la clé ni les contenus. Aucun transcript brut, écran d’auth ou JSON status complet n’est inclus au rapport.
