# Checklist de qualification et de publication

Cette checklist prépare une décision sur une candidate identifiée ; elle ne déclare aucun gate réussi et n'autorise pas une publication. Le périmètre global reste le produit complet local-first/cloud. Une preuve locale restreinte n'autorise pas à annoncer cloud, Kubernetes ou capacité 100 qualifiés. Autorités : [AGENTS.md](../../AGENTS.md), [gates G01–G12](PRODUCTION_READINESS.md), [stratégie de tests](TEST_STRATEGY.md), [état courant](CURRENT_STATE.md) et [preuves courantes](VALIDATION.md).

## Dossier de candidate

- [ ] Identifier révision/empreinte, date UTC, responsables réels, artefacts distribués, checksums, versions/digests et entrées de build.
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
| G06 | Audit/usage durables, idempotence/conflits, saturation, schéma/migrations, sauvegarde/restauration et rétention. Allocation `billable=false` tant que la facturation n'est pas qualifiée. |
| G07 | Fournisseurs versionnés et sessions réelles, sous-agents réellement exposés, provenance et limites de médiation visibles. |
| G08 | API et dashboard réels, permissions/clients, sécurité HTTP/SSE, export, états d'erreur et accessibilité. |
| G09 | Pannes/reprises gateway, dashboard, broker et moteur ; réconciliation sans perte silencieuse ni suppression de preuves. |
| G10 | Ressources, débit, latence et saturation mesurés ; charge 100 workers réels distincte de la recette trois workers et du synthétique. |
| G11 | Infrastructure et transports applicables, identités de workload, TLS/mTLS et réseau cloud effectivement appliqués ; [guide PKI](INFRASTRUCTURE_PKI.md). |
| G12 | Licence, notices, redistribution, maintenance, contribution/support, signalement privé, dépendances/SBOM/provenance et procédures install/upgrade/rollback. |

## Conditions de publication

- [ ] Chaque gate applicable porte `PASS` avec preuve acceptable ; toute non-applicabilité est justifiée pour le profil avant essais. `FAIL`, `BLOCKED`, `NOT_RUN` et test ignoré ne sont pas des succès.
- [ ] Les défauts bloquants des revues indépendantes sont traités et les régressions pertinentes rejouées. Aucun mock permanent, document de fonction future ou total de tests ne remplace une fonction exigée.
- [ ] Les limites et écarts du périmètre publié sont cohérents entre README, contrats, installation et preuves ; aucun ancien découpage MVP/V2 ne retire une obligation.
- [ ] Le texte [Apache-2.0](../../LICENSE), retenu par le mainteneur, les notices et droits de redistribution sont vérifiés sur les artefacts.
- [ ] Le canal privé de signalement des vulnérabilités est décidé et documenté avant publication. **Ce canal reste à déterminer ; aucune adresse n'est fournie par cette checklist.**
- [ ] La décision finale nomme ses responsables réels, sa candidate et son périmètre, sans mention « production-ready » au-delà des preuves. La publication et les opérations externes disposent du mandat correspondant.

Les rapports historiques restent conservés et identifiés. Le dossier final indique changements, commandes/résultats constatés, critères acceptés, limites et blocages précis, sans joindre de secrets ni de clés privées.
