# Recette trois workers — état et cible

La recette obligatoire est A1/A2 chez A et B1 chez B, avec contrôle séparé. Les douze critères sont énumérés dans [TEST_STRATEGY.md](TEST_STRATEGY.md). Elle ne requiert ni compte cloud ni cent conteneurs.

Le [document racine](../../LOCAL_CLOUD_TESTS.md) conserve une proposition détaillée, mais cite des commandes/tests et fonctions absents du checkout initial. **Ne pas exécuter ces commandes en supposant qu'elles existent.** Les nouveaux tests `tenant_api`, `tenant_storage` et `gateway_tenancy` couvrent l'identité/base d'audit A/B ; ils ne lancent pas de worker. Le seul smoke moteur livré initialement est `runtime_smoke`, à exécuter explicitement selon ses prérequis.

Le registre, les quotas, le ledger d'allocation non facturable et le dashboard partagé existent désormais ; leurs preuves logicielles et parcours moteur ciblés sont dans [CURRENT_STATE.md](CURRENT_STATE.md). Ils ne constituent pas la recette complète à trois workers. Identité worker, relais/grants/révocation, déploiement de cette recette et vérification de ses douze critères, dont la visibilité dashboard, restent ouverts. Chaque scénario réel conserve son moteur, version, OS/architecture, mode rootless/rootful, image/digest, commandes, observations, erreurs et volumes de preuve. Aucun volume existant n'est supprimé pour faire passer la recette.

Docker, Podman et Kubernetes ont des qualifications séparées. Un daemon absent bloque seulement la preuve correspondante. Une configuration cloud ou une fixture ne prouve pas le lancement d'un pod ni l'application d'une politique réseau.

## Cible Kubernetes locale retenue le 3 novembre

Kubernetes intégré à Docker Desktop, un nœud, contexte explicite `docker-desktop`. Après la création annoncée par le mainteneur, serveurv1.36.1 et un nœud Linux/ARM64 Ready sont observés via deux lectures TLS. Le [relevé](VALIDATION.md#3-novembre--cible-kubernetes-docker-desktop-à-un-nœud) distingue cette disponibilité des contrôles d'isolation et workloads encore à qualifier. Ne pas remplacer ce choix par un autre cluster ni changer le contexte global pour les essais.

Compléter le relevé par CNI, stockage et autorisations, puis qualifier le même scénario A1/A2/B1 sur les vrais pods, avec refus inter-tenant, identité workload, révocation, quotas, audit, usage idempotent et reprise. Vérifier les effets des politiques réseau et des restrictions de volumes/processus ; la simple création des objets n'est pas un PASS. L'adaptateur broker Kubernetes reste à implémenter ; aucun pod ai-guard n'est lancé par ce contrôle de plateforme. Charge100 et panne de nœud/haute disponibilité ont leurs qualifications distinctes ; aucune réussite n'est déduite du cluster à un nœud.
