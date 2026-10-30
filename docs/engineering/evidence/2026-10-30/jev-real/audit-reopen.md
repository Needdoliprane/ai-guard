# Addendum — vérification d'audit Jev hors ligne

Après arrêt du gateway, un petit vérificateur Rust temporaire a ouvert `/tmp/ai-guard-jev-private.GQXk5r/audit.sqlite` avec `Store::open`, `Redactor::new(Vec::new())`, puis a appelé `integrity`, `head` et une lecture ciblée après la séquence 2. Aucun service n'a été démarré, aucun appel réseau ou évaluation Jev supplémentaire n'a été effectué.

Résultat : `valid=true`, `event_count=3`, `head_sequence=3`, `error=null`. L'entrée de séquence 3 correspond à la requête `89c0b82a-a95d-4d3c-9dab-905462eeb4de`, et son hash `5b8ef833ade265dfb52c6f380ad5772d3222b811d7a21dc172abb4d622d284cd` égale la tête vérifiée et le hash renvoyé pendant la décision. La charge persistée rapporte Jev `jev-1.14.0`, `allow`, confiance `1.0`, usage 475/39 tokens; la décision locale demeure `deterministic` / `exact_allowlist` / `allow`.

Le hash SHA-256 de la base avant et après vérification est identique : `67d84ded8f9ebf8d07f0197cbecc0c584d838a98c0a848603922270446edabda`. Le résumé JSON privé est `/tmp/ai-guard-jev-private.GQXk5r/audit-integrity.json` (SHA-256 `efe6cc47f370ee406f927f7f1d758bd633addadc0d505b1fd894b642770f4a7c`). Vérificateur temporaire : source SHA-256 `d8e22fa5b2f38415bd7053f8a6c6e289eb4e971a16bd03bdd07b1081b40e4eb6`; binaire SHA-256 `cdcfc018aae617661cbed5cc33652c7be93965e6a487c8d4c2398259930b6031`. Le vérificateur et ses sorties restent dans le répertoire privé mode 700; fichiers mode 600.

Ce contrôle confirme l'intégrité de cette chaîne d'audit locale après réouverture. Il ne compte pas comme une nouvelle évaluation Jev et n'élargit pas la portée sémantique du smoke test initial.
