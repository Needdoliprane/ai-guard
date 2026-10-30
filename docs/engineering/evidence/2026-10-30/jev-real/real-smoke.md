# Jev réel — évaluation synthétique unique

## Résultat

**PASS — une seule requête d'évaluation a atteint Jev.** Elle a été envoyée par `JevClient` Rust depuis le gateway local vers `https://api.typesafe.ai/v1/systemone`, avec le modèle demandé `jev-latest`. Jev a retourné exactement `jev-1.14.0`.

- Identifiant : `89c0b82a-a95d-4d3c-9dab-905462eeb4de`
- Requête synthétique publique : profil `strict`, `argv=["/bin/echo", "sandbox-ok"]`, `cwd=/workspace`.
- Le texte de commande a servi uniquement de donnée d'évaluation. Aucun worker, conteneur, shell métier ni programme de commande n'a été lancé.
- Réponse Jev acceptée par le client : `allow`, confiance `1.0` (seuil gateway par défaut `0.85`), usage signalé par le fournisseur : 475 tokens d'entrée et 39 de sortie.
- Décision persistée renvoyée par le gateway : `allow`, moteur local `deterministic`, règle `exact_allowlist`. Le résultat Jev est attaché à la réponse auditée; Jev n'a pas remplacé l'autorité de la politique locale.
- Preuve d'audit retournée : séquence `3`, hash `5b8ef833ade265dfb52c6f380ad5772d3222b811d7a21dc172abb4d622d284cd`.
- Réponse du gateway HTTP 200; durée mesurée du parcours : 4 s. Le client Jev a un timeout total de 5 s, un timeout de connexion de 2 s, aucun retry et aucun suivi de redirection.

L'usage est le compteur de tokens retourné par le service. Aucun prix, coût ou montant facturé n'a été mesuré.

## Appels et écarts de prévol

Un seul POST a été effectué vers l'endpoint Jev. Aucun endpoint de compte ou de catalogue de modèles n'a été interrogé; aucun appel Claude/OpenAI n'a eu lieu.

Avant cet appel, un premier essai du harnais Rust a paniqué avant l'envoi HTTP (timer Tokio créé hors runtime). Il a été corrigé hors ligne. Un POST séparé au gateway a ensuite été refusé par la politique locale (`strict_default_deny`, `jev=null`) à cause d'une chaîne synthétique qui ne correspondait pas à l'allowlist : il n'a donc émis aucune requête Jev. Le harnais a été corrigé pour l'argv allowlist exact ci-dessus avec un nouvel identifiant, puis l'unique appel Jev a réussi. Ces états préalables restent dans la base d'audit privée de cette recette.

## Chaîne et confidentialité

- Binaire utilisé : `target/release/agentsandbox`, version `0.1.0`, SHA-256 `bb48545a6c7f6af0ab9b2b1e4b509927682ae6354d7372a979df1135d4058047`.
- Sources au moment de l'appel : `src/jev.rs` `157b05a5a6c2a2ba7a9dda43b261d7f9044b062a9e4c2f11b7d3de3290f72344`; `src/main.rs` `dfc772e81dd26a19de7c842f834c38010894068ae10373fc21bc1282212f2822`; `src/api.rs` `823451d1a68c91e4985237d24934bd896c10a808b40df25844bff3b8a61c0b79`. Les empreintes de ces sources et du binaire n'ont pas changé pendant la recette.
- Toolchain : Rust `1.99.1 (58cf8f9c3 2026-10-13)`, macOS ARM64.
- Petit client HTTP Rust temporaire, construit contre les dépendances release déjà compilées : source SHA-256 `4a12d87e45ce3a05f8f163edea515727a46db8291f39955b59bd410a7c4bdff1`; binaire SHA-256 `0d4b3a8ecbc2b03cc1e7053189dc9301690ad97c19cc43083da1a0df4d699844`.
- La clé Jev a été lue par le gateway depuis son fichier privé, jamais placée dans argv, variable d'environnement, réponse ou journal. `.env` n'a pas été ouvert. Le répertoire de preuve temporaire est mode `700`; fichiers de capture mode `600`. Recherche de la clé Jev et des deux tokens de contrôle dans les captures, la base d'audit et le binaire du harnais : aucun résultat.
- Gateway arrêté après la recette. Aucun Docker/Podman, image, test global ou build produit n'a été lancé.

Captures privées dans `/tmp/ai-guard-jev-private.GQXk5r/`. Le JSON réduit de réponse a SHA-256 `0c300ec271b84d5320be235092b67df704e27f9c63dea59f488a32c2e1ea4121`; la base SQLite d'audit privée a SHA-256 `67d84ded8f9ebf8d07f0197cbecc0c584d838a98c0a848603922270446edabda`.

## Portée

Ce smoke test vérifie un seul échange avec le contrat Jev configuré, le parsing d'une réponse, son association à une décision gateway et la présence d'une preuve d'audit. Un exemple synthétique et un résultat favorable ne démontrent ni la qualité sémantique générale de Jev, ni une couverture exhaustive, ni la disponibilité, la stabilité du modèle `jev-latest` ou l'efficacité d'un contrôle d'actions transitives.
