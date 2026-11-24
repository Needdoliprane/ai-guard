# Raccordement batch Kubernetes à Source17

Proposition à arbitrer le 24 novembre 2026, à partir du candidat `a7dbb869`.

Le driver Kubernetes utilise `SessionLedgerV2` pour réserver et obtenir les permis avant effet. Les routes de cycle de vie refusent l’accès Source17 avant lecture du corps. Le port de fédération Source17 ne couvre donc pas les opérations nécessaires au lancement Kubernetes.

Étendre le port broker Source17 aux opérations Kubernetes V2 existantes, avec les mêmes requêtes, reçus et règles de rejeu. L’identité d’exécution authentifiée détermine le tenant et l’incarnation du broker. Dans une même transaction tenant, revalider source, grant, politique et réservation, puis écrire la projection et l’audit avant de répondre. Démontrer la compatibilité avec le format 17. Toute extension du format ou des capacités durables est soumise à arbitrage.

Le broker appelle le driver existant. Celui-ci conserve les commandes d’effet uniques, leur observation par UID et la réconciliation. Le parcours batch retient durablement le permis avant effet, arme l’échéance avant de lever le verrou de planification et commence avec le réseau coupé. Le résultat déclaré par le worker précède la constatation terminale de Kubernetes et le règlement.

## Premier test hors ligne

Exercer le broker avec un gateway Rust, un Store SQLite Source17 et un pair Kubernetes local. Exiger une création du pod, une levée du verrou, un attachement et une trame de démarrage par tentative. Après la sortie déclarée, tenir le pod non terminal et vérifier que le quota reste retenu. Constater ensuite l’état terminal du même UID, vérifier le reçu durable et le retour du quota à sa valeur initiale. Le rejeu du règlement conserve le même reçu, sans second démarrage ni seconde libération.

Le passage sur un cluster réel est une validation distincte, avec identité de broker bornée et stockage RWOP dédié.
