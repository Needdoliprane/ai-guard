# Retrait des diagnostics et correction du seul harnais

1 novembre2026. Résultats attribués à la lecture Root/Luna de la reproduction privée `ai-guard-outbox-io-reproduction-20261101-013745` : RC101, receive_timeout ; retour caller10,304649958s ; backend512 réussi après14,120798250s, senddelivered=false ; observation postérieure head1/512 en8,855521250s. Cela établit la branche timeout de cette reproduction et la completion backend ultérieure, sans établir une causalité de la factorisation ni qualifier une latence10s.

Avant toute nouvelle édition, les deux gels temporaires ET leurs originaux privés ont été comparés aux SHA attendus. Copies exactes rétablies, puis SHA constatés :

- writer : 47c72983851dc057c22d6731f55b99b0c208bbe8401d8333e00e582d217538d7
- tests/outbox phase2 : f9a34cfc7fbf3398bc81821af21860ce8958a0a36a4e2dc376c7faf1481ac7a5

Ensuite seulement, delta du harnais dans tests/broker_outbox.rs :

1. Fixture::new délègue à with_timeout(control_secrets,10s), conservant les anciens usages à10s.
2. Le nouveau constructeur privé with_timeout reprend le même corps et transmet le timeout à WriterClient::start.
3. Le seul test wrong_context_secrets_strict_nested_decision_and_event_cap_are_rejected_without_writes appelle with_timeout(...,60s), plafond existant de l'API Writer. Son commentaire distingue vérification fonctionnelle des caps en debug et latence qualifiée.

Toutes les assertions/caps/oracles restent intacts. Aucun retry, sleep, nouvelle mutation, signal, secret, code applicatif ou budget worker changé. Aucun marqueur temporaire, conservation spéciale du TempDir ou lecture post-erreur ne reste dans le code final.

SHA FINAL harnais : 64843007eee0a8b7a11c2857e8d600294dc3404a957779215bfcb9b8bc06a242. Writer reste exactement au SHA d'origine ci-dessus. Les trois sources canoniques phase2 et S7 ont également été recoupés inchangés.

Rustfmt ciblé et git diff --check RC0. Aucun Cargo/test lancé par l'implémenteur. Rejeu ciblé à confier à Luna, même filtre fonctionnel exact, sans diagnostic temporaire. Qualification après correction du harnais : NOT_RUN à cette livraison.
