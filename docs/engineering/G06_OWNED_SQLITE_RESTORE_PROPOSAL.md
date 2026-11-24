# Remise en service d’une copie SQLite owned

Proposition à arbitrer à partir de `6c7397c0`. Elle concerne Store14 et Store15.

L’activation actuelle autorise une identité physique de fichier. Une copie restaurée conserve les faits qu’elle contient et reste en maintenance. Les commandes opérateur actuelles ne publient pas de clôture successorale pour une copie.

## Conditions de reprise

- Préserver l’activation précédente, son journal et les preuves disponibles. Conserver les identités logiques de l’installation, du tenant et des sources. Établir la nouvelle identité physique et vérifier l’union entière avant publication.
- Comparer la tête d’audit et les références de la copie au dernier état durable indépendamment vérifiable. Un historique manquant ou divergent maintient la copie en maintenance tant que les faits ne sont pas réconciliés. Une action inconnue reste inconnue.
- Rapprocher les opérations et claims en attente de leurs reçus et de l’autorité d’exécution. Les claims consommés vérifiés restent rejouables avec le même reçu. La copie ne réautorise aucun effet dont le résultat reste indéterminé.
- Prévoir une procédure opérateur explicite et rejouable. Vérifier la clôture complète et l’exclusivité de la source avant de publier l’activation successorale. Une interruption doit permettre de reprendre ou de rester en maintenance avec les preuves intactes.

## Preuves attendues

La clôture originale refuse la copie avant toute écriture durable. Un cas autorisé rejoint une tête vérifiée, retrouve les reçus et le quota, et active une seule fois la nouvelle liaison. Une copie plus ancienne, un claim non réconcilié et une interruption avant publication restent en maintenance. La procédure doit ensuite être vérifiée avec les commandes opérateur sur un environnement de recette.
