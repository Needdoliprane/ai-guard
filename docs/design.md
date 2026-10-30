# ai-guard — conception du produit

## Mandat et autorité

Construire un produit open source local-first, complet, soigné et exploitable, avec une déclinaison cloud utilisant majoritairement le même code. Le mandat actuel du mainteneur remplace l'ancien périmètre de démonstrateur ; la cible production doit être qualifiée, jamais déduite de la compilation. Voir [AGENTS.md](../AGENTS.md), [PROMPT_PRODUCTION.md](../PROMPT_PRODUCTION.md), le [contrat d'architecture](engineering/ARCHITECTURE_CONTRACT.md) et les [gates G01–G12](engineering/PRODUCTION_READINESS.md).

L'architecture est validée : Rust hors frontend ; frontend TypeScript statique servi par un dashboard Rust dans Docker/Podman ; contrôle séparé des workers ; broker Rust dans le périmètre de confiance ; SQLite distinct par client. Un changement structurel est soumis au mainteneur avant réalisation. Les [preuves courantes](engineering/VALIDATION.md) et l'[état courant](engineering/CURRENT_STATE.md) déterminent ce qui est effectivement disponible.

## Frontières de confiance

- Le gateway/collecteur porte politique, authentification, audit et API. Une identité authentifiée détermine le client ; aucun champ libre de requête ou label worker ne le remplace.
- Le dashboard sert le front et les lectures autorisées. Il n'obtient pas l'autorité runner ni le socket moteur. L'arrêt de l'IHM ne doit pas arrêter les contrôles.
- Le broker est un composant Rust de confiance sur le poste ou la plateforme. Il pilote le runtime par programme et arguments séparés. Aucune commande métier ne se replie sur l'hôte.
- Chaque worker non privilégié appartient à un seul client et utilise un volume de copie filtrée, jamais le projet original. Réseau coupé par défaut, capacités supprimées, racine en lecture seule et ressources bornées doivent être inspectés et testés par leurs effets.
- Le rôle worker n'accède qu'au relais autorisé. Chaque échange nécessite un grant dirigé source/destination/canal, révocable ; les échanges inter-clients sont refusés.

La cible est de 0 à 100 workers actifs par client. La recette locale standard utilise A1/A2 chez A et B1 chez B, en plus du contrôle. Ni ce scénario ni la charge 100 ne sont déduits d'une fixture. Voir [identité et stockage](engineering/CLOUD_TENANCY.md) et [recette locale](engineering/LOCAL_CLOUD_TESTS.md).

## Parcours et expérience attendus

Un utilisateur doit pouvoir préparer son installation, diagnostiquer les prérequis, s'authentifier, lancer une session autorisée, comprendre un refus, suivre ses événements, examiner ses changements, exporter vers une destination neuve, arrêter et reprendre après panne. Les permissions de la CLI, de l'API et de l'interface restent explicites. L'IHM consultative actuelle ne devient pas une administration simplement parce qu'un bouton est dessiné.

Chaque parcours traite chargement, vide, données partielles ou périmées, déconnexion, authentification refusée, permission insuffisante et erreur. Il indique la portée des données et une reprise utile. Aucun compteur ni statut de démonstration n'est injecté dans le produit. Une erreur de collecte n'est pas affichée comme zéro activité.

Le design reste simple et léger : navigation claire, langage cohérent, motifs de décision lisibles, détails progressifs, recherche et filtres stables, thèmes clair/sombre. Clavier, focus, contrastes, annonces d'état et petits écrans font partie de l'acceptation. Les tableaux et flux restent bornés et utilisables sous la charge annoncée. Voir les [critères d'acceptation](acceptance.md) et l'[interface existante](ui.md), sans confondre leur description et une preuve d'audit complet.

## Garanties et niveaux d'observation

`strict`, `review` et `observe` conservent leurs limites documentées dans l'[architecture](architecture.md). L'autorisation porte sur l'invocation soumise au broker ; toute promesse d'interception transitive exige un canal démontré avant l'action et des tests de contournement. Le parsing d'un journal fournisseur n'est pas une médiation.

Demande, autorisation, soumission, démarrage inspecté et résultat sont distincts. `observed`, `declared` et `inferred` restent visibles, avec la couverture native/générique. Les parents d'agents ne sont pas déduits d'un texte. Un champ inconnu reste inconnu. Les ressources réellement consommées ne sont pas déduites de limites configurées, et le temps d'allocation reste `billable=false` selon le [contrat d'usage](engineering/USAGE_METERING.md).

Une décision et ses traces obligatoires doivent être durables avant effet. Réservations, états, messages, grants et mesure terminale exigent transactions, audit et reprises idempotentes ; une divergence sous la même identité est un conflit. Le masquage précède persistance et diffusion. Sauvegarde/restauration, saturation, migrations et réconciliation ont des scénarios d'échec et des procédures propres.

## Livraison et maintenance

Le produit comprend code, images, configurations explicites, contrats, procédures d'installation/mise à jour, diagnostic, reprise, limites, compatibilité et contribution. La licence retenue est [Apache-2.0](../LICENSE). Le canal privé de signalement des vulnérabilités reste à déterminer avant publication ; aucune adresse ou autorité n'est inventée.

Docker et Podman exigent des preuves séparées sur Linux x86_64 et macOS ARM64. Kubernetes reste une cible d'adaptateur complète, avec identités, transport chiffré, réseau appliqué, stockage et reprise réels. Les comptes et infrastructures externes manquants bloquent leur qualification sans interrompre les travaux indépendants.

Le [plan d'implémentation](engineering/IMPLEMENTATION_PLAN.md) organise des lots relisibles. Il ne reporte pas les obligations en « V2 ». Seules les extensions réellement facultatives sont ainsi identifiées, avec justification. Les rapports historiques restent conservés ; les statuts `PASS`, `FAIL`, `BLOCKED`, `NOT_RUN` et `NOT_APPLICABLE` justifié s'appliquent à une preuve nommée et à son périmètre.
