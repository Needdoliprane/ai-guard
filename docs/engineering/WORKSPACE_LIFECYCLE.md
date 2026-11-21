# Cycle de vie des branches, worktrees et caches

Tenir les affectations, preuves d'intégration et opérations de fermeture dans le [registre des travaux](WORK_TRACKING.md).

## Ouvrir et exploiter une ressource

1. Relever l'état Git et les worktrees existants avec `git worktree list --porcelain`. Inscrire le lot, son propriétaire, son périmètre, la branche et son commit de base, le worktree, la destination d'intégration et les emplacements des preuves et caches. Réutiliser une ressource disponible et compatible quand elle suffit.
2. Affecter une cible de compilation mutable à chaque checkout. Documenter le propriétaire d'une cible Cargo, d'un cache front ou d'un cache de build d'image. Réutiliser des dépendances communes en lecture si leur identité, leur environnement de build et leurs consommateurs sont connus. Préserver les caches actifs et les binaires utilisés par un processus, un gardien ou un rejeu en cours.
3. À un changement de propriétaire, une suspension ou un abandon, relever le dernier commit, les modifications locales et les usages encore actifs. Noter la reprise ou la décision de conservation attendue. Vérifier ces faits à la reprise d'une session. Une date ancienne déclenche une vérification.

## Établir l'intégration et préparer la fermeture

1. Inscrire la branche de destination et son commit d'intégration. Vérifier la présence du travail sur cette destination et ses tests pertinents. Pour une fusion conservant l'historique, établir l'ascendance du commit source exact. Pour un squash ou un rebase, rapprocher les changements attendus des commits d'intégration et expliquer les différences. Conserver dans un bundle Git vérifié, hors du worktree, les commits source qui deviendraient inaccessibles.
2. Examiner séparément les modifications suivies, indexées, non suivies et ignorées. Confirmer l'intégration ou la conservation vérifiée de chaque élément utile. Préserver les preuves et commandes de rejeu, la configuration privée, les credentials et certificats, les données persistantes et les binaires encore nécessaires. Vérifier leur accès et leurs empreintes selon leur nature. Pour chaque preuve à garder, inscrire l'identifiant ou le chemin et l'empreinte de son archive durable, hors du worktree et des caches purgeables. Indiquer comment le prochain propriétaire pourra y accéder, avec les droits existants si la preuve est privée.
3. Affecter la fermeture à un seul responsable. Rapprocher le worktree et ses chemins de compilation des agents, sessions, processus, configurations et dépendances qui les utilisent. Préserver le checkout principal, les références pérennes et les ressources actives. Une dépendance, une preuve manquante ou une incertitude impose une ligne de clôture avec propriétaire et condition de reprise.

Pour un lot abandonné avant intégration, consigner la décision du mainteneur et conserver les commits et changements locaux dans une archive vérifiée avant toute fermeture autorisée. Inscrire l'adresse de l'archive et la procédure de reprise dans le registre.

## Fermer les ressources intégrées

Le responsable revalide les références et les consommateurs juste avant chaque suppression. Si l'état a changé, il reprend les vérifications du périmètre concerné.

1. Supprimer le worktree secondaire intégré avec `git worktree remove <chemin>`. Traiter tout refus de Git à partir des fichiers et usages observés. Pour les métadonnées de worktrees déjà absents, exécuter `git worktree prune --dry-run --verbose` juste avant la purge avec la même configuration et le même filtre d'expiration. La purge exige une liste entièrement vérifiée et dans le périmètre convenu.
2. Supprimer la branche de travail intégrée avec `git branch -d <branche>` une fois ses worktrees fermés. Si un squash ou un rebase empêche la suppression ordinaire, conserver la preuve d'intégration et vérifier le bundle, l'absence de checkout et l'autorisation applicable avant `git branch -D <branche>` sur cette seule référence. Pour supprimer une branche distante, obtenir l'autorisation du mainteneur pour le dépôt et la référence exacts. Conditionner la suppression sur le serveur au SHA vérifié avec `--force-with-lease=refs/heads/<branche>:<sha>` et vérifier la disparition de cette référence. Préserver une référence qui a avancé, les branches pérennes et les tags de release.
3. Purger les caches historiques régénérables devenus inutiles. Identifier le chemin ou l'identifiant exact, sa provenance, ses consommateurs et la commande de régénération. Utiliser la commande ciblée de l'outil sur la ressource vérifiée. Préserver les caches partagés actifs, les preuves, les volumes et les données persistantes. Mesurer le gain disque sur le même volume avant et après la purge et indiquer le gain observé séparément de la taille logique des fichiers.
4. Revérifier les références et le chemin de chaque ressource retirée. Enregistrer la date, les références d'intégration et d'archive, les suppressions confirmées, les éléments conservés et l'espace réellement récupéré. Le lot atteint l'état clôturé après retrait de ses ressources de travail intégrées. Un retrait restant conserve son état, sa condition et son propriétaire dans le registre.
