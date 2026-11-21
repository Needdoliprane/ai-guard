# Checklist de qualification et de publication

Cette checklist prépare une décision sur une candidate identifiée ; elle ne déclare aucun gate réussi et n'autorise pas une publication. Le périmètre global reste le produit complet local-first/cloud. Une preuve locale restreinte n'autorise pas à annoncer cloud, Kubernetes ou capacité 100 qualifiés. Autorités : [AGENTS.md](../../AGENTS.md), [gates G01–G12](PRODUCTION_READINESS.md), [stratégie de tests](TEST_STRATEGY.md), [état courant](CURRENT_STATE.md) et [preuves courantes](VALIDATION.md).

## Dossier de candidate

Le [programme V1](V1_DELIVERY.md) regroupe les travaux, les matrices et les vérifications d’audit qui alimentent cette checklist.

- [ ] Identifier révision/empreinte, date UTC, responsables réels, artefacts distribués, checksums, versions/digests et entrées de build.
- [ ] Vérifier Dockerfiles et instructions de construction/distribution par le client, y compris préparation/import hors Internet selon le profil annoncé. Aucune image publique préconstruite du projet n'est obligatoire ; traçabilité, versions et preuves concernent les images réellement construites et testées.
- [ ] Fixer les profils annoncés et la matrice Docker/Podman × Linux x86_64/macOS ARM64 ; conserver séparément recette trois workers A1/A2/B1, charge 100 et Kubernetes/cloud.
- [ ] Lier chaque exigence à sa procédure, son résultat attendu/constaté, ses artefacts masqués, sa revue et ses limites. Les comptes/accès externes sont autorisés, jamais inventés.
- [ ] Vérifier les parcours d'installation, diagnostic, identité, lancement/refus, observation, examen/export, arrêt et reprise ; erreurs, données inconnues/périmées, accessibilité et lisibilité sous charge font partie de l'acceptation.

## Revue des gates

| Gate | Preuve à examiner avant décision |
|---|---|
| G01 | Contrats communs, versions supportées et identité des artefacts effectivement testés. |
| G02 | Confinement réel, restrictions effectives et isolation des clients/volumes/autorités. |
| G03 | Original inchangé, copie/export sûrs, destination neuve, concurrence et erreurs de filesystem. |
| G04 | Identité authentifiée, rôles, quotas, approbation unique, refus et panne d'audit sans effet. |
| G05 | Réseau et secrets réels, grants dirigés/révocables, refus inter-client et contournements. |
| G06 | Audit/usage durables, idempotence/conflits, saturation, schéma/migrations, sauvegarde/restauration et rétention. Preuves applicatives sur les [trois profils SQL](TEST_STRATEGY.md#stockage-sql-configurable--à-qualifier). Allocation `billable=false` tant que la facturation n'est pas qualifiée. |
| G07 | Fournisseurs versionnés et sessions réelles, sous-agents réellement exposés, provenance et limites de médiation visibles. |
| G08 | API et dashboard réels, permissions/clients, sécurité HTTP/SSE, export, états d'erreur et accessibilité. |
| G09 | Pannes/reprises gateway, dashboard, broker et moteur ; réconciliation sans perte silencieuse ni suppression de preuves. |
| G10 | Ressources, débit, latence et saturation mesurés ; charge 100 workers réels distincte de la recette trois workers et du synthétique. |
| G11 | Infrastructure et transports applicables, identités de workload, TLS/mTLS et réseau cloud effectivement appliqués ; documentation conforme au [guide PKI](INFRASTRUCTURE_PKI.md) incluant procédure d'intervention et rollback. |
| G12 | Licence, notices, redistribution, maintenance, contribution/support, canal privé de signalement des vulnérabilités, dépendances/SBOM/provenance et procédures install/upgrade/rollback. |

Le [contrat canonique d'identité et de configuration](../sso.md) et les [vues par permissions](../ui.md#vues-utilisateur-et-administration-unifiée--cible-à-livrer) font partie de cette revue : identité locale nominative hors ligne, support OIDC public PKCE S256 qualifié même si désactivé à l'installation, mapping et périmètres côté Rust, délégations/audit vérifiable, administration à 360° autorisée et inférence interne sans repli cloud. Examiner leurs [preuves spécifiques](TEST_STRATEGY.md#identité-individuelle-sso-et-découverte--à-qualifier), le profil Keycloak opt-in à quatre comptes et les procédures de découverte/raccordement entreprise. Les clés partagées et les fixtures ne satisfont pas ces exigences.

## Conditions de publication

Le passage du dépôt en public accompagne la première V1 fonctionnelle. Le mainteneur le décide sur une candidate identifiée après réussite de tous les parcours exigés et des gates applicables. Un cas obligatoire en échec, bloqué ou sans preuve maintient le dépôt privé. Le développement se poursuit après la V1.

- [ ] Chaque gate applicable porte `PASS` avec preuve acceptable ; toute non-applicabilité est justifiée pour le profil avant essais. `FAIL`, `BLOCKED`, `NOT_RUN` et test ignoré ne sont pas des succès.
- [ ] Les défauts bloquants des revues indépendantes sont traités et les régressions pertinentes rejouées. Aucun mock permanent, document de fonction future ou total de tests ne remplace une fonction exigée.
- [ ] Les limites et écarts du périmètre publié sont cohérents entre README, contrats, installation et preuves ; aucun ancien découpage MVP/V2 ne retire une obligation.
- [ ] Le texte [Apache-2.0](../../LICENSE), retenu par le mainteneur, les notices et droits de redistribution sont vérifiés sur les artefacts.
- [ ] Le canal privé de signalement des vulnérabilités est opérationnel, testé et documenté avant publication (procédure de réception, traitement et réponse). **Ce canal reste à déterminer ; aucune adresse ni protocole n'est actuellement fourni.**
- [ ] La décision finale nomme ses responsables réels, sa candidate et son périmètre, sans mention « production-ready » au-delà des preuves. La publication et les opérations externes disposent du mandat correspondant.

Les rapports historiques restent conservés et identifiés. Le dossier final indique changements, commandes/résultats constatés, critères acceptés, limites et blocages précis, sans joindre de secrets ni de clés privées.
## CI GitHub et brouillon de release

- Les pushes sur `dev` et `main` lancent les tests unitaires Rust et frontend, les régressions de l’agrégateur fournisseur et la compilation du frontend. La CI exécute aussi `agentsandbox version --json` et vérifie la révision embarquée avant qu’un run vert puisse autoriser un tag. Les recettes E2E sur moteur ou navigateur restent dans le guide de test local. [CI](../../.github/workflows/ci.yaml) · [Recettes E2E](../../tests/e2e/README.md)
- Depuis **Actions → Build tag and prepare GitHub Release → Run workflow**, choisir `main` et saisir le nouveau tag. La CI du dernier push sur le commit exact doit être terminée avec succès. Le tag doit correspondre aux versions Cargo et frontend du commit. Le workflow crée le tag, puis construit depuis sa référence. Un push Git de tag ne lance aucun build dans ce workflow. [Workflow manuel](../../.github/workflows/release.yaml) · [Déclenchement manuel GitHub](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow)
- Le tag annoté lie le nom, le commit et l’identifiant du run. Si l’accusé de création de la référence est perdu, relancer le job du même run. Le retry retrouve l’objet exact et vérifie l’annotation. Le build épingle le SHA de l’objet et celui du commit. Le brouillon contrôle aussi le SHA de l’objet avant sa création et chaque envoi. Une réponse perdue avant la création de la référence peut laisser un objet annoté sans référence. [API des tags GitHub](https://docs.github.com/en/rest/git/tags)
- Pour empêcher aussi la création des tags depuis Git par des comptes disposant de droits d’écriture, configurer une règle de dépôt qui réserve le préfixe de release à l’autorité autorisée. La configuration du workflow borne le chemin de build à son lancement manuel. [Règles de tags GitHub](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets#restrict-creations)
- Avant de créer une archive native, le workflow exécute `agentsandbox version --json` sur le binaire copié. Il vérifie le composant, la version demandée et la révision embarquée. Il vérifie aussi les sommes SHA-256 et l’unique fichier régulier `BUILD-INFO` de chaque archive. Les archives Linux x86_64, macOS ARM64 et du frontend sont jointes au brouillon avec `SHA256SUMS`. Le reçu d’envoi et l’inventaire final doivent correspondre à la taille et à l’empreinte de chaque fichier local. Le brouillon indique le commit et le lien du run CI. La publication demande la revue de la présente checklist. [API des fichiers de release](https://docs.github.com/en/rest/releases/assets)
- Si le build ou le dépôt d’archives échoue après création du tag, relancer les jobs échoués du même run dans le délai de rétention de sept jours. Un nouveau lancement refuse le tag existant. Pour le brouillon de ce run, la reprise vérifie le tag, le commit et le marqueur du run, compare les fichiers déjà présents aux artefacts vérifiés, puis joint les fichiers absents à l’ID exact du brouillon sur `uploads.github.com`. Publier après la fin du run. Une publication manuelle du même brouillon pendant une requête de dépôt peut la devancer et sera détectée au contrôle suivant. Un brouillon étranger, une release déjà publiée lors du contrôle, un dépôt incomplet portant déjà le nom attendu ou un fichier divergent bloque la reprise. Dans ces cas, conserver le brouillon et les preuves du run pour inspection.

 ## Dossier de candidate
