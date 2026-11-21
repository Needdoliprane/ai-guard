# Politique de sécurité

[English](SECURITY.md)

## Signaler une vulnérabilité

Signalez toute vulnérabilité en privé par les **GitHub Security Advisories** de ce dépôt : ouvrez l'onglet **Security** du dépôt et choisissez **Report a vulnerability**. Ne placez ni vulnérabilité ni détail de reproduction dans une issue, une pull request ou une discussion.

**État actuel du canal.** Le dépôt est privé. Les mainteneurs peuvent ouvrir dès maintenant des avis de sécurité en brouillon. Le signalement privé par des tiers sera activé lorsque le dépôt deviendra public ; d'ici là, un rapporteur doit demander à un mainteneur d'ouvrir un avis en brouillon et de l'y inviter. Aucune adresse e-mail, prime ni engagement de délai de réponse n'est proposé.

## Informations à fournir

*   Une description claire du problème et du composant concerné (gateway, broker, dashboard, image worker, archives de release, documentation).
*   La version (`agentsandbox version --json`) ou le commit, et le runtime (Docker, Podman) le cas échéant.
*   Les étapes de reproduction, avec des données synthétiques uniquement. N'incluez jamais de credentials, de jetons ou de données de clients réels.
*   Une évaluation de l'impact potentiel.

## Procédure de traitement

1.  **Réception.** Un mainteneur ouvre ou accepte l'avis en brouillon. Le rapport reste privé pour les participants de l'avis.
2.  **Accusé de réception.** Un mainteneur répond dans l'avis. Aucun délai fixe n'est promis.
3.  **Tri.** Le mainteneur reproduit le rapport sur le checkout courant et le rapproche des constats connus du [suivi d'audit](docs/engineering/AUDIT_FOLLOWUP.md). L'avis consigne le résultat : confirmé, informations demandées, doublon ou rejeté, avec le motif.
4.  **Correction.** Un problème confirmé est corrigé dans une branche privée de l'avis avec un test de régression, puis relu indépendamment.
5.  **Publication.** Le correctif est livré dans une release. L'avis est ensuite publié avec les versions touchées et corrigées ; le rapporteur est crédité sauf refus.

## Versions prises en charge

Seule la dernière release publiée est considérée pour les correctifs. ai-guard n'est pas qualifié pour la production ; voir les [gates de préparation à la production](docs/engineering/PRODUCTION_READINESS.md).

## Vérifier les artefacts de release

Les archives de release sont publiées avec `SHA256SUMS`, un SBOM CycloneDX et un fichier `BUILD-INFO` dans chaque archive qui consigne son tag, son commit et son run de workflow. **Rien n'est signé cryptographiquement** : les sommes détectent une corruption, pas une page de release compromise. La procédure de vérification est dans le [guide d'installation](docs/setup/release/install.fr.md).
