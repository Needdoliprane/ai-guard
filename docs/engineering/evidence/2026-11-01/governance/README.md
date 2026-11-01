# Cohérence de la gouvernance — 1er novembre 2026

Relecture parallèle au chantier du journal sur la base `f6f93b3`. Les quatorze brouillons du checkout principal restent inchangés par rapport à leur copie d'intégration. Aucune exigence produit supprimée, aucun code applicatif modifié par ce lot documentaire.

La [revue indépendante](independent-review.md) relève puis vérifie le traitement de trois contradictions : administration UI encore facultative dans couverture/architecture/acceptation ; signatures d'audit seulement éventuelles dans l'exploitation ; ancien nombre de vues dans la couverture. Les contrats canoniques [SSO](../../../../sso.md) et [IHM](../../../../ui.md) restent les références ; les copies racine d'architecture et de couverture restent identiques à leurs copies sous `docs/`. Les deux guides d'exploitation gardent leurs différences historiques, avec des liens adaptés à leur emplacement.

Le contrôle des liens a aussi révélé une ancienne cible relative manquante dans `operations.md` racine ; elle est corrigée vers `docs/engineering/CLOUD_TENANCY.md`. Les résultats Root finaux sont dans [checks.txt](checks.txt) : existence des cibles relatives uniquement, sans validation exhaustive des ancres ni des pages HTTP. Aucun test Rust, frontend, navigateur, moteur ou fournisseur n'est nécessaire ou exécuté pour ces seuls changements de texte.

**PASS documentaire ciblé.** Identité locale nominative, OIDC/PKCE Web/CLI, découverte Keycloak, administration unifiée et points de contrôle signés restent à implémenter et qualifier. Aucun gate de production n'est fermé.
