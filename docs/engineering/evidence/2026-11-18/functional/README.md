# Preuves fonctionnelles du 18 novembre 2026

Le candidat part de `c4c4d2a7` et inclut les modifications locales de cette passe. Hôte macOS ARM64, Docker Desktop 4.97.0, moteur Linux ARM64 29.9.2, Compose 5.6.1. Pile Compose dédiée sur deux ports loopback, base SQLite neuve et exception rootful explicite pour les workers. Les services existants ont été conservés.

Les commandes de rejeu, les prérequis et les harnais sont réunis dans le [guide E2E](../../../../../tests/e2e/README.md).

## CLI et navigateur sur les mêmes sessions

| Case | Batch | TTY à deux tours |
|---|---|---|
| Docker · Codex API · `gpt-6-luna` | **PASS** avec Codex CLI 0.165.0. [CLI](docker-codex-api-cli.json), [Chrome](docker-codex-api-browser.json) | **FAIL** à l’échéance. Le harnais a détecté un reçu sur deux. [CLI](docker-codex-api-tty-cli.json), [État terminal](docker-codex-api-tty-terminal.json) |
| Docker · Claude API · `claude-haiku-5-5` | **PASS** avec Claude Code 2.1.140. [CLI](docker-claude-api-cli.json), [Chrome](docker-claude-api-browser.json) | **FAIL** à l’échéance. Le harnais n’a détecté aucun reçu avant l’arrêt. [CLI](docker-claude-api-tty-cli.json), [État terminal](docker-claude-api-tty-terminal.json) |
| Docker · Codex abonnement et Claude abonnement | **BLOCKED** en l’absence de profils d’abonnement préparés. [Codex](docker-codex-subscription-preflight.json), [Claude](docker-claude-code-subscription-preflight.json) | **BLOCKED** pour la même raison |
| Podman · Codex et Claude · API et abonnement | **BLOCKED** pour l’environnement d’essai local. Podman 6.1.4 a répondu `rootless=true` sur un socket Unix privé vers la VM, puis l’import de l’image a échoué pendant la saturation du stockage. | **BLOCKED** |
| Kubernetes · Codex et Claude · API et abonnement | **BLOCKED** sur le checkout principal. Le cluster Docker Desktop local répond en v1.36.1 Linux ARM64. Le parcours de lancement fournisseur manque. | **BLOCKED** |

Pour Docker batch, chaque client a créé un `result.txt` au contenu exact dans son volume de session. Le projet original est inchangé et le résultat est vérifié dans une destination neuve. Les deux états étaient terminaux, avec quota libéré et conteneur nettoyé. Chrome 157.0.8103.79 a retrouvé les décisions, résultats, changements et exports de ces sessions. Le téléchargement JSONL était égal au journal complet filtré de chacune. Les identifiants et empreintes figurent dans les preuves. Les modèles ci-dessus sont ceux demandés et liés à la décision.

En TTY, l’état terminal indique un dépassement de durée, le code 137, le nettoyage du conteneur et la libération du quota. Le pilote confirme la restauration des modes du terminal. La recette ne localise pas encore la cause du dialogue incomplet entre l’état de l’invite native, la saisie et la lecture de l’écran. Aucun appel fournisseur supplémentaire pour rejouer l’échec. [Intégrité après les essais TTY](docker-audit-after-tty.json)

La première lecture Chrome des deux échecs expirait à cause d’un sélecteur du harnais. Après correction et déploiement du bundle `index-DHFtuHpt.js`, Chrome a lu les mêmes traces, affiché **Délai dépassé** et le motif durable, puis exporté leur JSONL sans relancer les sessions. L’image du dashboard utilisée pour ce rejeu est `sha256:c3f8e144fd57a73903ab3c6d5929f10035ca6eda22282e0556b6c9391ac2305f`. [Échec Codex affiché](docker-codex-api-tty-timeout-browser.json) · [Échec Claude affiché](docker-claude-api-tty-timeout-browser.json)

Les deux parcours batch ont aussi été relus sur ce bundle avec les mêmes UUID et empreintes JSONL. La pile dédiée a ensuite été arrêtée en conservant sa base et ses volumes de preuve. [Codex relu](docker-codex-api-browser-final-bundle.json) · [Claude relu](docker-claude-api-browser-final-bundle.json)

Le harnais TTY a ensuite été durci sur un terminal synthétique. Un déplacement du curseur ne doit jamais concaténer deux fragments en un faux reçu. Un composeur occupé peut ignorer une écriture même si le premier reçu est visible. Les indices de disponibilité des deux clients natifs et le rejeu fournisseur à deux tours restent ouverts.

L’image de contrôle Linux ARM64 est `sha256:7eef21307f6fb07c98d2c8dd2efd436d5731564282c0aadc74ba1f1143b331dc`. L’image worker fournisseur Linux ARM64 est `sha256:1bf2e06bc934d4e9eb3d87f207029e2b1c30986f59137b4c8e6810612fd1e632`. Le hash du binaire CLI hôte est dans les résultats. Les appels API étaient réels sur des projets synthétiques, avec le réseau `bridge` explicitement sélectionné. Le filtrage sortant, le mode rootless Docker et Linux x86_64 restent à qualifier. Voir la [recette et la matrice requise](../../../TEST_STRATEGY.md#parcours-utilisateur-avec-les-fournisseurs-réels).

## Cycle de vie Docker

L’essai réel `runtime_smoke` a vérifié UID 1000, refus sans création, copie/export, allocation observée non facturable, réconciliation sans doublon et masquage sur données synthétiques. Dans une recette séparée, la CLI a été tuée lorsque le moteur confirmait que le worker dormant était actif. La reprise a retrouvé ce worker, l’arrêt a produit un terminal durable et le quota a été libéré. Ce parcours prouve la reprise **manuelle** de la même instance. Après redémarrage du gateway dédié, la chaîne de 217 événements a conservé le même hash de tête. [Reprise](docker-crash-recovery.json) · [Intégrité](docker-audit-after-restart.json)

Le harnais de rejeu a reproduit la reprise sur une pile dédiée neuve. Il a observé le même worker, conteneur et claim de démarrage après SIGKILL de sa CLI, puis la libération du quota et un audit de 19 événements inchangé après redémarrage du gateway. [Rejeu automatisé](docker-crash-replay.json)

La mesure de l’échéance autonome échoue sur le binaire actuel. Le SIGKILL de la CLI intervient avant 7,98 s pour une limite worker de 15 s. Docker observe le même conteneur encore actif après 20,01 s. La reprise ciblée qui suit libère le quota, puis le gateway redémarré retrouve le même terminal, l’empreinte du journal et une allocation observée non facturable. [Échéance et reprise](docker-crash-autonomous-deadline.json)

Dans le checkout isolé, le raccordement `run` au broker passe neuf tests du binaire avec fixture IPC et 21 tests du service. Le dernier essai Docker de la composition précédente s’est arrêté lors de la création du volume. L’échéance physique avec le nouveau point d’entrée reste à tester. [Preuves logicielles](broker-run-software.json) · [Diagnostic moteur](native-broker-composition.json)

La CLI a démarré et arrêté deux fois une pile distincte sur deux ports loopback. Entre les deux démarrages, le volume et l’empreinte d’audit sont restés identiques. Dans un essai séparé, le démarrage s’est terminé avec une erreur quand le dashboard sain a refusé le jeton viewer de la recette. [Cycle de vie](docker-control-stack-replay.json) · [Refus viewer](docker-control-stack-viewer-denied.json)

## Sauvegarde de migration SQLite

Sur macOS 27.2 ARM64, le test CLI a migré une base synthétique du schéma 1 au schéma 2. Un nouveau gateway, démarré sur une copie privée de la sauvegarde fermée, a retrouvé l’empreinte et le nombre d’événements d’origine par `verify-audit`. Le registre a signalé `migration_required` avec HTTP 503 et les octets de la sauvegarde demeurent identiques. La preuve couvre la lecture du journal historique. La restauration des droits de démarrage d’une base v2 reste ouverte. [Données de l’essai](sqlite-migration-backup-recovery.json)

## Rejeu du dashboard avec un worker terminal

Chrome a vérifié dix contrôles sur les services Rust natifs et un worker Docker terminal. Les événements de recette sont marqués `declared`. L’allocation du worker est `observed` et `billable=false`. Le scénario du worker absent a conservé un HTML et une capture PNG privés, avec les deux jetons absents des octets des fichiers. Le harnais de préparation avait interrompu la commande `run` après 120 secondes pendant l’observation finale du workspace. À ce stade, le terminal du worker était durable et le quota libéré. L’export a d’abord été refusé sur une autre origine du gateway, puis a réussi sur l’origine enregistrée pour la même session. Le fichier exporté est identique au fichier de référence et les six nouveaux événements se rapportent à l’export. [Preuve](web-dashboard-live.json)

## Décision

- **Rust déterministe.** Le parcours Docker hors ligne exerce l’allowlist et un refus sans conteneur ni volume. Les tests logiciels de politique vérifient aussi le profil de revue et la liaison à l’approbation exacte.
- **Approbation réelle sous Docker.** Une demande hors allowlist en profil `review` renvoie le SHA-256 à confirmer et ne réserve aucun worker. La même demande avec le hash complet produit un `allow` audité puis un fichier dans la copie isolée. [Preuve](docker-review.json)
- **Jev simulé.** Le vrai client Rust interroge un serveur HTTP loopback. Les cas passent pour l’accord, le veto, `review`, la confiance sous seuil, le HTTP 503, le délai dépassé et la panne de connexion. Le refus impératif local n’émet aucun POST. La réouverture SQLite conserve les reçus et le rejeu ne réinterroge pas le service. [Test](../../../../../tests/jev_contract.rs)
- **Jev TypeSafe réel.** Un seul jugement borné, via le gateway Rust et `POST /v1/systemone`, pour une commande synthétique `/bin/echo sandbox-ok`. L’alias `jev-latest` a retourné `jev-1.14.0` avec `allow`. La règle Rust `exact_allowlist` est restée la décision effective. La base a été rouverte avec audit valide et aucun worker créé. [Preuve](jev-real.json) · [Contrat TypeSafe](https://docs.typesafe.ai/api/system-one)
- **Qwen local réel.** Deux classifications hors ligne sur Docker `--network none`, sans exécuter les commandes décrites. Qwen2.5-0.5B-Instruct Q4_K_M, poids SHA-256 `74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db` et image `sha256:d08b6e1aa127dcdd58ddc913b935cca384534824932a841d9b524edd87135042`. Contexte 512, un slot, deux CPU et 64 tokens de sortie maximum. Le pic cgroup était de 872 747 008 octets pour une limite effective de 999 997 440 octets, avec zéro OOM et sans swap. Le modèle a répondu `review` pour la lecture et `deny` pour la suppression synthétique testées. Cet essai précède le raccordement au gateway et laisse ouverte la qualification sémantique. [Lecture](qwen-read.json) · [Suppression](qwen-remove.json) · [Mémoire](qwen-memory.txt) · [Essai historique](../../2026-10-30/local-judge/README.md)
- **Qwen via le gateway Rust.** Sur le candidat suivant du même checkout, l’adaptateur `local_openai` a obtenu un avis `review` du Qwen réel et enregistré un refus effectif. Le pic cgroup était de 313 745 408 octets sous une limite de 999 997 440, avec zéro OOM. Aucun jeton supplémentaire n’a été traité sur le refus impératif. Le reçu durable a été rejoué avec le modèle arrêté et sa trace relue dans Chrome. Une demande CLI fraîche sans modèle a été refusée avec un inventaire Docker inchangé. Le modèle reste consultatif. [Preuve intégrée](qwen-local-gateway.json)
