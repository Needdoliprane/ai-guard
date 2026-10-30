# Recette trois workers — état et cible

La recette obligatoire est A1/A2 chez A et B1 chez B, avec contrôle séparé. Les douze critères sont énumérés dans [TEST_STRATEGY.md](TEST_STRATEGY.md). Elle ne requiert ni compte cloud ni cent conteneurs.

Le [document racine](../../LOCAL_CLOUD_TESTS.md) conserve une proposition détaillée, mais cite des commandes/tests et fonctions absents du checkout initial. **Ne pas exécuter ces commandes en supposant qu'elles existent.** Les nouveaux tests `tenant_api`, `tenant_storage` et `gateway_tenancy` couvrent l'identité/base d'audit A/B ; ils ne lancent pas de worker. Le seul smoke moteur livré initialement est `runtime_smoke`, à exécuter explicitement selon ses prérequis.

Le registre, quotas, relais/grants, usage, déploiement trois workers et parcours dashboard correspondants restent à implémenter. Chaque scénario réel conserve son moteur, version, OS/architecture, mode rootless/rootful, image/digest, commandes, observations, erreurs et volumes de preuve. Aucun volume existant n'est supprimé pour faire passer la recette.

Docker, Podman et Kubernetes ont des qualifications séparées. Un daemon absent bloque seulement la preuve correspondante. Une configuration cloud ou une fixture ne prouve pas le lancement d'un pod ni l'application d'une politique réseau.
