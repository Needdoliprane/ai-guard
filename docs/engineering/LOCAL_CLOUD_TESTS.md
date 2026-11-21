# Recette trois workers — état et cible

La recette obligatoire sur le poste du mainteneur lance trois vrais workers en parallèle, A1/A2 chez A et B1 chez B, avec contrôle séparé. Trois à cinq workers suffisent à la validation fonctionnelle locale. La mesure de capacité à 100 workers actifs par tenant relève d’un environnement dimensionné pour cette charge. [Critères de la recette](TEST_STRATEGY.md)

Observer les trois workers actifs pendant une même période, avec leur identifiant d’exécution, un volume de travail propre et un résultat distinct pour chacun. Arrêter A1 pendant que A2 et B1 poursuivent leur exécution, puis vérifier leurs résultats et la portée des traces côté CLI et dashboard. Conserver les vérifications de rôles, de refus inter-tenant et du relais A1→A2 soumis à autorisation.

## Fin normale sur Docker Desktop

Le gel `434c1400` de la candidate a vérifié les trois workers A1/A2/B1 en parallèle sur Docker Desktop ARM64, avec deux tenants et 20,08 s de chevauchement. A1 s’achève pendant qu’A2 et B1 sont actifs. Les exports, quotas, allocations, chaînes d’audit et refus d’accès aux détails inter-tenant passent. L’arrêt explicite, le relais et le dashboard sous la même charge gardent leurs propres recettes. [Résultats et empreintes](evidence/2026-11-19/functional/native17-and-local-workers.md#trois-workers-a1-a2-et-b1-après-redémarrage)

Sélectionner une image QA locale par son SHA-256 complet. Elle fournit `/bin/sleep` et `/bin/tar`. Le répertoire `/workspace` existe dans l’image et appartient à `1000:1000`, comme dans le Dockerfile worker. Le volume neuf hérite de ce répertoire à l’import. Dans l’image de contrôle testée, `tar` a reçu un refus d’écriture en UID 1000. L’image QA préparée sans réseau à partir de Debian local a passé l’extraction sur un volume neuf.

```sh
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_RUNTIME_PROGRAM="$QA_DOCKER_BIN" \
AGENTSANDBOX_TEST_RUNTIME_ENDPOINT="$QA_DOCKER_UNIX_ENDPOINT" \
AGENTSANDBOX_TEST_IMAGE="$QA_WORKER_IMAGE_ID" \
  cargo test --locked --offline --test runtime_user_journey \
  real_cli_runs_three_independent_workers_across_two_tenants \
  -- --ignored --exact --nocapture
```

Sur Docker Desktop rootful approuvé, fournir `AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1` au processus de test. Les workers de la recette utilisent le profil strict et le réseau coupé.

Le [document racine](../../LOCAL_CLOUD_TESTS.md) conserve une proposition détaillée, mais cite des commandes/tests et fonctions absents du checkout initial. **Ne pas exécuter ces commandes en supposant qu'elles existent.** Les nouveaux tests `tenant_api`, `tenant_storage` et `gateway_tenancy` couvrent l'identité/base d'audit A/B ; ils ne lancent pas de worker. Le seul smoke moteur livré initialement est `runtime_smoke`, à exécuter explicitement selon ses prérequis.

Le registre, les quotas, le ledger d'allocation non facturable et le dashboard partagé existent désormais ; leurs preuves logicielles et parcours moteur ciblés sont dans [CURRENT_STATE.md](CURRENT_STATE.md). Ils ne constituent pas la recette complète à trois workers. Identité worker, relais/grants/révocation, déploiement de cette recette et vérification de ses douze critères, dont la visibilité dashboard, restent ouverts. Chaque scénario réel conserve son moteur, version, OS/architecture, mode rootless/rootful, image/digest, commandes, observations, erreurs et volumes de preuve. Aucun volume existant n'est supprimé pour faire passer la recette.

Docker, Podman et Kubernetes ont des qualifications séparées. Un daemon absent bloque seulement la preuve correspondante. Une configuration cloud ou une fixture ne prouve pas le lancement d'un pod ni l'application d'une politique réseau.
