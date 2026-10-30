# Jev réel — un échange synthétique vérifié

**PASS limité au contrat, à la décision durable et à sa réouverture**, pas à la qualité sémantique générale. Le [smoke](real-smoke.md) utilise le client Rust produit, `https://api.typesafe.ai/v1/systemone`, le modèle demandé `jev-latest`, un état synthétique public très court et une question `choice`. Aucun fichier de projet envoyé, aucun worker ou programme métier lancé.

Un seul POST externe, sans retry ni header long contexte. Modèle retourné `jev-1.14.0`, `allow`, confiance 1,0 ; 475 tokens d'entrée et 39 de sortie déclarés par le service. La politique locale conserve l'autorité `deterministic/exact_allowlist` : Jev n'a pas élargi ses permissions. Un prévol refusé par la politique locale et un défaut du timer du harnais avant envoi sont conservés, sans compter comme requêtes Jev.

[Réponse](evaluation-response.json), puis [audit après arrêt](audit-integrity.json) : trois événements, chaîne valide, tête à la séquence 3, même hash que la décision retournée ; [addendum](audit-reopen.md). Le hash de la base est inchangé pendant la vérification hors ligne. La clé et les tokens n'apparaissent pas dans les preuves ; seuls les résumés publics sont copiés ici. Le gateway temporaire est arrêté.

Binaire `bb48545a6c7f6af0ab9b2b1e4b509927682ae6354d7372a979df1135d4058047`, [sources/binaire avant](product-before.sha256) et [après](product-after.sha256) identiques. Le dernier garde Jev du handoff legacy est postérieur à ce binaire et n'est pas exercé ici. Aucune disponibilité garantie, calibration indépendante, stabilité de l'alias, couverture transitive ou facture déduite de ce test. Aucun nouveau modèle ni compte interrogé.

Sources primaires vérifiées : [HTTP TypeSafe](https://docs.typesafe.ai/api/system-one), [Choice](https://docs.typesafe.ai/primitives/choice), [modèles et alias](https://docs.typesafe.ai/models).
