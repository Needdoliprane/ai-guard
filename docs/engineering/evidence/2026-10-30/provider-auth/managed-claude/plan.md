# Plan — Claude API géré, image candidate, une invocation

Date : 2026-10-30

## Critère

Une seule invocation gérée `agentsandbox run` doit sélectionner explicitement `claude-code` + `api`, utiliser l’image locale exacte `agentsandbox-worker:qa-managed-providers-20261030-ccgpe3` (ID `sha256:069ba7b703077238f22edbefc9295a11cc07e71e8dd490b8de7ab81da51ffa38`) et le modèle exact `claude-haiku-5-5`. Attendu : nouvel `result.txt` au workspace racine, contenu ASCII exact `AI_GUARD_CLAUDE_MANAGED_API_OK_20261030_A1` sans newline final; aucun autre fichier/commande/réseau/sous-agent demandé; code de sortie, admission/audit, terminal, export neuf, original intact et quota nettoyé vérifiés. Une erreur fournisseur/modèle/credential/budget arrête la recette. Aucun retry.

## Préconditions

- Contrôle privé neuf sous `/tmp` mode 0700; config/tokens générés uniquement par `agentsandbox init`.
- Profil API privé créé par `providers init --env-file /tmp/ai-guard-anthropic-private.azwq8v/anthropic-api.env`; ne jamais lire, afficher, sourcer, copier, logger ou passer la valeur du credential. Ne pas employer le `.env` racine.
- Projet privé sans secret ni donnée, avec un seul input `task.txt`; `result.txt` absent avant le run.
- Gateway du binaire Rust `target/release/agentsandbox` (SHA-256 `bb48545a6c7f6af0ab9b2b1e4b509927682ae6354d7372a979df1135d4058047`), écoute loopback sur port disponible autre que 48787. Aucun paramètre Jev; aucun service QA partagé.
- Commandes d’état non fournisseur avant la seule invocation : `providers status --json`, santé gateway et vérification que l’image exacte est présente. Aucun appel de modèle lors d’init/status.

## Run borné

`agentsandbox run --config-dir <config> --gateway http://127.0.0.1:<port> --runtime docker --image agentsandbox-worker:qa-managed-providers-20261030-ccgpe3 --project <project> --profile observe --network bridge --timeout 75 --client claude-code --auth-mode api --client-profile claude-api --model claude-haiku-5-5 --prompt <prompt exact>`.

Le prompt demande un unique fichier `result.txt` avec le canary exact; aucune autre opération, commande shell, connexion réseau ou sous-agent. Aucun argv fournisseur manuel, mode Jev ou authentification par abonnement. Durée maximale runtime fixée à 75 s (inférieure à 90 s); surveiller le mur et arrêter/réconcilier à la première limite.

## Budget fournisseur et arrêt

L’aide officielle embarquée expose `--max-budget-usd`, mais `src/provider/bootstrap.rs` n’envoie pas cette option au preset batch et `run` n’expose pas de plafond monétaire. Ne pas modifier le code ou contourner le preset : le coût fournisseur n’a donc pas de borne établie. La recette n’utilisera qu’une invocation CLI, un prompt à effet unique, et s’arrêtera au premier échec sans retry. Si l’opérateur ou le fournisseur annonce un rejet de budget, ne pas relancer.

## Après le run

Garder les journaux privés mode 0600. Vérifier l’unique fichier/canary dans l’export vers une destination neuve, l’absence de résultat dans l’original, arrêt du worker et `cleanup` confirmé, quota actif à zéro, allocation mesurée (billable=false), intégrité audit après réouverture. Arrêter seulement le PID gateway créé pour ce dossier et conserver toutes les preuves. Aucune modification au dépôt.

## Addendum — premier échec de harnais avant l’appel modèle

La première commande `agentsandbox run` a affiché la résolution explicite Claude/API/`claude-haiku-5-5`, puis échoué sur `POST /api/events` avec connexion refusée. Le gateway de cette tentative avait été lancé en arrière-plan dans une commande de préparation; son processus a disparu à la fin de cette session d’outil. La trace `runner::managed::execute_plan` confirme que `command.requested` précède la décision et l’admission. Inspection factuelle : aucun gateway n’écoute; `docker ps -a` ne montre aucun nouveau conteneur; le seul conteneur du tag est l’ancien probe arrêté; aucun fichier de checkpoint de session n’a été créé. Aucun CLI Claude/API ni appel modèle n’a été lancé. L’échec est conservé au dossier `/tmp/ai-guard-managed-claude-api.2WicEb/`; binding/snapshot et lock privés non supprimés.

Le mainteneur a autorisé une unique tentative fraîche après correction du harnais, car l’échec précédent précède l’API. Cette tentative emploie un nouveau config-dir, de nouveaux tokens, un projet séparé avec input `task.txt` neuf et un nouveau canary B1; elle ne rejoue aucun claim ni workspace. Le gateway est servi dans une session foreground persistante dont le handle outil est stocké immédiatement, puis listener et healthcheck sont vérifiés immédiatement avant le seul run. Le même modèle explicite reste fixé. Si l’appel Anthropic atteint le fournisseur et échoue (modèle, credential, budget ou autre erreur), arrêt définitif sans retry.

## Résultat final de la tentative fraîche

Le run unique avec `--allow-rootful-runtime` explicite a terminé exit0, `timed_out=false`, `changed_files=1`, cleanup container `removed`. Image `agentsandbox-worker:qa-managed-providers-20261030-ccgpe3`, ID `sha256:069ba7b703077238f22edbefc9295a11cc07e71e8dd490b8de7ab81da51ffa38`. Le modèle demandé et résolu localement était `claude-haiku-5-5`; le modèle effectivement servi reste inconnu faute d’observation distincte. Export neuf vérifié : canary C1 exact, 42 octets sans newline, SHA-256 `cc2b93ab193564857b7236f4d1569949d368fc55374dd6976daad6fa334fb6b0`; input exporté byte-identique, original intact. Worker terminal, quota libre, pas d’OOM; ledger allocation observée, 5.923 s, `billable=false`. L’admission est auditée séquence4 avant start claim séquence13. Après arrêt puis redémarrage du gateway sur la même base, `verify-audit valid=true`, 36 événements, hash de tête `31274bcc970ee1c7c9beacc4dc7bdfda367c4c655b2cb9c9c22b3ba5bf4e24fa`; statut worker toujours terminal et nettoyé.

Les deux préflights sans appel fournisseur restent consignés dans leurs dossiers précédents. Aucun nouvel appel Claude n’a été effectué après la réussite. Aucun plafond monétaire n’est établi : le preset n’envoie pas `--max-budget-usd`; le timeout75 borne uniquement la durée worker.
