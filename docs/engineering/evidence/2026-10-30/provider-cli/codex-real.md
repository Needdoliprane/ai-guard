# Essai réel ciblé Codex — 30 octobre 2026

## Résultat

- **PASS — refus policy** : l’exécution en profil `strict`, réseau `none`, a été refusée avant création du worker et avant appel fournisseur. L’audit enregistre `security.decision=deny` avec la règle `strict_default_deny` et le motif d’absence de l’allowlist exacte.
- **FAIL — première exécution Codex** : le processus Codex s’est terminé avec le code 0, mais aucun fichier n’a été créé (`changed_files=0`). Ses deux tentatives de commande ont échoué au démarrage du sandbox imbriqué `bwrap` : le noyau ne permet pas la création d’un namespace utilisateur non privilégié. Le premier export ne contenait donc pas `result.txt`. Le code de sortie 0 seul n’est pas un succès fonctionnel.
- **PASS — seconde exécution autorisée** : après diagnostic, une unique nouvelle tentative a été explicitement autorisée. Codex a créé `result.txt`; l’export neuf contient le fichier aux octets attendus et le README original conserve son SHA-256 initial. Le worker est arrivé à l’état terminal, a été supprimé proprement et sa réservation de quota est relâchée.
- **PASS — audit** : vérificateur final `valid=true`, 668 événements.
- **PASS — mesure d’allocation** : allocation observée `billable=false`. Elle mesure une allocation Docker, pas le prix, la facture fournisseur ni le CPU réellement consommé.
- **NOT RUN — Claude et terminal interactif** : aucune clé Claude n’était fournie et aucun TUI n’a été testé. `codex exec --json` valide uniquement le parcours non interactif, pas l’interaction TTY ni le courtage des entrées/sorties interactives.

## Configuration et bornes

Worker bâti depuis `images/worker.Dockerfile`, Codex CLI exactement `0.165.0`, tag isolé `agentsandbox-worker:qa-codex-0.165.0-20261030` (image `sha256:42cfc041ee5daa0128da53da45c580e66aa1c17839b5013f053f7221d7179cab`). Modèle demandé : `gpt-6-luna`. Le modèle est présent dans le catalogue CLI local, mais la réponse de l’adaptateur ne prouve pas indépendamment le modèle servi.

Les deux exécutions ont utilisé un projet temporaire minimal, timeout 300 s, profil observe, réseau Docker bridge explicitement observé, image et limites existantes (2 CPU, 1 GiB, 256 PID), secret injecté via le mécanisme existant sous le seul nom `CODEX_API_KEY`. La deuxième invocation a fixé le sandbox interne Codex à `danger-full-access` et `approval_policy=never`, sans modifier les contraintes du conteneur Docker. Cela permet le test dans le conteneur contrôlé, mais ne qualifie pas le mode imbriqué `workspace-write`. Aucun privilège, seccomp, sysctl, montage, réseau ou mécanisme d’authentification n’a été affaibli. Pas de preuve de médiation ou de contrôle des actions transitives.

L’export réussi est `/tmp/ai-guard-codex-real-run.rNPuqB/export-second`; il contient `README.txt` et `result.txt`. SHA-256 de `result.txt` : `71b2d747aae3bb4e7edd4038d029a5f273329d97001d3b14b70f2d8a2b883577`. SHA-256 du README original avant et après : `7ef29d760972b1e26a8271178786e44e2a10b11990290eea3af406a674e2cd11`.

Aucun code applicatif du dépôt n’a été modifié. Le test ne constitue pas une qualification générale de Codex, de l’interface interactive, de Claude, de la couverture d’observation ou de la sûreté des actions transitives.

## Artefacts à archiver

Archiver le rapport, `manifest.txt`, `proofs.txt`, la fixture et l’export réussi, `provider.summary.json`, `provider2.summary.json`, les événements normalisés, les événements d’audit de refus, `worker-status2.json`, `usage-query2.json` et `audit-final.json`. Garder l’ensemble privé avec permissions restrictives; ne pas archiver les fichiers de credentials ni les sorties fournisseur brutes. Index détaillé : `/tmp/ai-guard-codex-real-run.rNPuqB/proofs.txt`.
