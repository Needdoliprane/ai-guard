# Revue documentaire indépendante H8/H9/S1

Périmètre : worktree `broker-service`, lecture seule des sept documents de suivi et des passages source/tests utiles à leurs affirmations. Aucun Cargo, moteur, service, fournisseur, Store, credential ou QA ouvert par cette revue ; aucun fichier du dépôt modifié. Les consignes récentes du mainteneur, contrats d'architecture et règles de code ont été lus. Le rapport privé est seul écrit par le relecteur.

## Conclusion finale

**Aucun défaut bloquant identifié dans les sept documents relus au gel final H8/H9/S1/S2.** Les comptes et statuts sont cohérents avec les logs/RC lus. Aucun test supplémentaire requis par cette revue documentaire. Elle ne constitue ni un audit sécurité global ni une qualification runtime ou production.

Les remarques de chronologie initiales sont traitées :

1. `CURRENT_STATE.md` distingue explicitement le point courant H8/H9/S1 des statuts et prochaines étapes des gels historiques D3/PR4.
2. `PRODUCTION_READINESS.md` nomme le complément D3/PR4 comme historique avant H8/H9 ; son ancien NOT_RUN ne contredit plus l'émission H9 observée.
3. Les cases de préparation/exécution H8/H9 du plan sont cochées en tant qu'actions exécutées, avec leurs FAIL conservés. H10 reste ouvert. Le budget240s de H9 est historique ; l'instruction récente et le code S1 portent le global à600s, sans changer le worker5s.
4. Le README contient les nouvelles sections H8/H9 et S1/S2, les faits/limites et les deux ancres référencées. Les résultats plus bas sont explicitement attribués à leurs gels historiques.

## Contrats et limites recoupés

- `tests/broker_runtime.rs` utilise600s pour la borne globale ; profil et demande worker conservent5s. Les oracles avant/après la faute se réfèrent toujours à la deadline d'origine. Cette revue ne les exécute pas.
- Le marqueur H9 comporte huit champs fermés ; le GET `kind=broker.batch.failure&limit=1&order=asc` est celui existant. Les trois tests purs valident le lecteur, pas HTTP ou Docker.
- Le test Core replay utilise `Operation::Decision`, avec vrai gateway/SQLite/Writer ; il ne prouve pas un WorkerClaim/start physique. Le gardien natif est vide. La distinction apparaît correctement dans le nouveau README.
- Un Draft confirmé au head est historique, lié scope/opération, sans autorité ou garantie latest. Les documents ne doivent pas en déduire un start, un terminal, un fencing ou une libération de quota.
- Le snapshot `helper_slot` provient du status déjà lu ; il reste distinct du sample local. H9 n'avait pas ce nouveau snapshot. Une expiration native reste non démontrée dans H9 et l'effet runtime S1 est NOT_RUN.
- G09 conserve FAIL et0/12 gates restent fermés ; SSO/Keycloak/PKCE reste ABSENT/NOT_RUN. Aucun pourcentage nouveau ni production-ready revendiqué.

## Contrôles de preuve lus

Les cinq logs/RC S1 terminés lus dans `/private/tmp/ai-guard-s1-20261031-220416` confirment12 diagnostics,2 prédicats,2 Core,1 postload et3 marqueurs =20 fonctions uniques PASS. Le helper enfant et les anciens replays ne sont pas ajoutés. Les deux tests Core prennent72,05s ; les trois marqueurs0,00s après compilation release13min01s. Cette compilation de tests ne devient ni une exécution moteur ni le build applicatif final.

`06-fmt-check.rc=0`. Le log `07-clippy.log` et son RC101 montrent exactement le lint `.err().expect()` du nouveau test Core, conservé comme FAIL S1. Le delta `/private/tmp/ai-guard-s2-20261031-222447/s1-s2-journal-test.diff` remplace uniquement cette assertion par `expect_err()` avec même message ; aucune condition ou source applicative changée. Les20 tests ne sont pas rejoués ou présentés comme exécutés sur S2. Les logs/RC S2 lus confirment fmt0, Clippy all-targets strict0 et build release0 (0,54s). Aucun Cargo lancé par le relecteur.

Les documents nomment159 sources S2 et séparent vérifications Luna/root, checkout/archive et copies binaires ; leurs hashes sont cohérents entre README et VALIDATION. Cette revue n'a pas réexécuté la vérification d'archive ou des binaires déjà réalisée par root.

Outil local de liens exécuté au gel final sur les sept documents :364 liens relatifs,0 cible manquante, y compris après ajout du rapport source final. Ce contrôle vérifie les fichiers cibles, pas les URLs externes. Les deux nouvelles ancres `h8h9--observation-et-premier-refus-codé` et `s1--draft-acquitté-et-snapshot-helper` ont été recoupées manuellement avec leurs titres. Le lien `s2/independent-review.md` et sa cible sont présents ; SHA de cette copie `e840108df57a394fd130540229c2d757526dc9741060065434b28c15dfb7d7eb`, conforme à celui transmis par root.

## Limites maintenues

H8 reste un FAIL d'observation, H9 un FAIL préclaim avec diagnostic réel et cause de phase non établie. L'émission du snapshot et l'effet temporel S1/S2 restent NOT_RUN ; aucun correctif causal, autonomie de deadline, reprise, D5, nouveau claim/start physique ou gate fermé n'est déduit des20 fonctions. L'essai H10 reste la prochaine qualification instrumentée, sans extension des5s du worker. SSO, autres plateformes/runtimes et capacité100 restent des écarts ouverts. Les commandes logicielles/opt-in sont cohérentes avec leur périmètre ; la commande Docker propre à cet hôte n'est pas présentée comme quick start ordinaire.
