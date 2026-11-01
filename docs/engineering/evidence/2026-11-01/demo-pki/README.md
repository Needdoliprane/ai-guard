# Générateur d'AC local — bilan daté du 1er novembre 2026

Le générateur Rust reste livré dans `src/bin/ai-guard-demo-pki.rs`. Le [guide d'utilisation](../../../../demo-pki.md) décrit la configuration, les fichiers produits et les limites. Le mainteneur a demandé de conserver ses essais auxiliaires localement, sans les distribuer dans Git : `tests/demo_pki.rs`, captures, instruments et archives sont désormais ignorés. Aucun fichier de preuve n'a été effacé et aucun historique Git n'a été réécrit.

Résultats observés avant ce retrait du suivi : trois fonctions uniques PASS, connexion TLS réelle Rustls/reqwest avec chaîne racine/intermédiaire et refus du nom incorrect et d'une AC inconnue ; formatage et Clippy strict PASS. Quatre échecs antérieurs restent conservés dans les captures locales. Les archives source initiales comportaient des métadonnées AppleDouble ; une copie portable séparée a été vérifiée sur les163 contenus source, sans les remplacer.

Les détails historiques publiés existent au commit `2bcdb629abe879b8ceaa79ad81cb720cdc7e87ba`. Le manifeste source final a SHA256 `20c467f30834f36f82a7024f4e82c6796ac01014ea43786b27a299040cdd551f` et l'archive portable `02a13540baa87c0a32a5ffb3e014198a31bfaebbf3be8f3d083d02a00817f56b`. Ils identifient un gel, pas une qualification du HEAD courant.

Aucune confiance système, résolution DNS ou infrastructure externe n'a été installée. Release, autres plateformes et transports des services ne sont pas qualifiés par cet essai ; G11 reste ouvert. L'exclusion des instruments de Git ne transforme aucun NOT_RUN en PASS.
