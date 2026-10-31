# Revue indépendante du harnais H9 — avant effet

Lecture seule du worktree `broker-service`. Périmètre : `tests/broker_runtime.rs` et `tests/support/broker_runtime.rs`, contrats réels `EventQuery`, `StoredEvent`, API `/events` et producteurs de diagnostics. Aucun Cargo, moteur, fournisseur, QA, Store ou credential ouvert ; aucune source modifiée par le relecteur. Le diff des sources applicatives, Cargo.toml/lock et toolchain contre `af4a868` est vide. Un second regard indépendant a relu le parseur et ses tests.

## Conclusion source

**Aucun P0/P1 restant identifié après les corrections de revue.** Le harnais retire seulement la borne locale de préparation de 120 s et ajoute un observateur HTTP de diagnostics. Global 240 s et Arm worker 5 s restent inchangés ; aucune deadline, claim, génération, permission ou logique applicative modifiée. Cette conclusion source ne vaut pas résultat d'un essai moteur H9.

- Le poll utilise l'endpoint existant `events?kind=broker.batch.failure&limit=1&order=asc`. `ReadIdentity` résout le tenant avant la query ; `Store::events` applique le filtre exact SQL paramétré, puis ordre ascendant et limite. `StoredEvent` est plat dans `items`. Un `next_cursor` positif avec l'unique item est normal pour cette limite et accepté.
- Les requêtes sont séquentielles ; `next_failure_poll` est fixé une seconde après la réponse. Donc au plus une nouvelle requête diagnostique par seconde, aucune boucle de retry ni RPC Writer/RecoveryStatus ajouté. Le client conserve timeout HTTP de 3 s et contrôle existant de taille 2 Mio après réception. Les requêtes peuvent retarder l'observation ; aucune neutralité de performance n'est revendiquée.
- Un candidat Running est retenu, puis un dernier poll diagnostic est obligatoire avant le break. Si nécessaire, `sleep_until` attend la prochaine échéance pour respecter la fréquence. L'attente vaut au plus une seconde, à laquelle s'ajoute la latence HTTP ; le global 240 s englobe l'ensemble. Cela peut faire échouer l'oracle strict avant faute, sans accorder davantage de durée au worker.
- Le parseur exige UUID canonique non nul, séquence i64 positive, source `runner`, kind exact, data v1 et effet `unknown`, phase `LocalFailurePhaseV1` fermée. Les allowlists broker/native/runtime correspondent aux enums réellement produits. Au moins une des trois provenances doit exister ; nested stage manquant, mauvais type ou label inconnu échoue avec un message fixe. Les erreurs UUID/serde sont remplacées par des erreurs opaques.
- Seuls huit champs validés sont écrits dans `reported-batch-failure.json`. Aucun `command`, argv, raw data, reason, corps d'erreur ou extra n'est copié. La requête utilise le runner du contrôle neuf, sans l'envoyer au worker. Ce contrôle privé, sa base neuve et son unique Submit établissent ici la corrélation ; ce filtre sans session ne prétend pas corréler des événements d'un serveur partagé ou de plusieurs jobs.
- HTTP/parseur invalide, disparition, remplacement, Terminal/Cancelled avant Running, refus Submit et fin du global restent FAIL. Un diagnostic accepté produit le marqueur privé puis un FAIL fixe ; il ne prouve ni absence d'effet, ni terminal, ni quota libéré. Sa séquence atteste la réponse de l'audit central ; l'ACK local d'outbox peut encore être pending.

Les oracles après observation restent identiques : unique Arm d'origine 5 s, worker réellement Running dans Docker, gardien sain Ready/prepared_once et epoch d'origine ; SIGKILL/reap du seul broker Child possédé avant la deadline native originale ; arrêt du même CID au plus tard deadline+3 s, exit137 non OOM, durée réelle strictement entre 0 et 20 s ; reprise avec mêmes Arm/deadline/clock/attempt/CID/claim/StartedAt, sans deuxième émission, usage idempotent, quota libéré et audit valide. Aucune absence/erreur inspect ne devient un terminal. Drop conserve sa portée broker/gateway ; anciens gardiens, QA et ressources restent conservés.

## Constats de revue traités

1. Le premier gel sortait sur Running avant le GET diagnostic. Running et une erreur déjà visible pouvaient donc éviter ce dernier poll. Corrigé : candidat retenu, poll soumis à la même fréquence, puis break. Cela couvre la réponse de ce poll, sans promettre une absence atomique de toute erreur future jusqu'au kill.
2. Le parseur acceptait les trois origines absentes/null, contrairement au producteur `classify`. Corrigé par le contrôle d'au moins une provenance et une assertion négative dans le test existant ; réponse malformée refusée au lieu d'être marquée comme diagnostic codé valide.
3. Le commentaire attribuait trop précisément H8 à une réponse Arm interrompue. Corrigé : borne atteinte sans Running ni code, retour de réponse non établi. Le libellé fixe nomme désormais la phase **before the planned broker fault**, car le dernier poll peut suivre une observation Running. Le diff final contre le dernier gel GREEN contient cette seule constante ; aucun contrôle modifié.

## Preuves et limites

Trois fonctions pures écrites relues : projection limitée avec canari privé exclu et next_cursor positif, page vide, métadonnées invalides/erreurs opaques dont absence de provenance. Logs Luna lus : RED `/private/tmp/ai-guard-h9-red-20261031-205332`, compilation réussie, test sélectionné échoué sur le stub None (RC101) ; premier GREEN `/private/tmp/ai-guard-h9-green-20261031-205708`, 3/3 PASS + fmt/Clippy RC0 ; gel corrigé `/private/tmp/ai-guard-h9-final-20261031-210125`, mêmes 3/3 PASS + fmt/Clippy RC0, 158 sources vérifiées après exécution. **Trois fonctions uniques**, sans addition des rejeux. Les deux scénarios runtime filtrés ne sont pas comptés comme exécutés. Aucun test exécuté par ce relecteur.

Le dernier gel GREEN précède seulement le remplacement de la constante de message relu ci-dessus. Les checks finaux de cette constante sont à l'orchestrateur ; aucun nouveau PASS anticipé ni besoin de recompter les trois tests. Le relevé Luna conserve le SHA release applicatif `ec99e7817efc53ccb912d99bd5d5130c24bc89c9e7a59ef5d24cfb782338e9f6`, identique à PR4 ; aucune modification applicative depuis `af4a868`. Les tests purs ne qualifient ni le débit HTTP réel ni une émission diagnostique sur moteur. Le test runtime demeure opt-in/ignoré ; sa compilation n'est pas son exécution.

H8 reste **FAIL d'observation**, cause produit non établie : head33 WorkerArm publié vers 118,303 s après le marqueur Submit, aucun WorkerClaim, FailureObserved ou événement diagnostic. Les mtimes ne datent pas un retour de reçu ni une erreur native. Le rapport sélectionné `/tmp/ai-guard-runtime-h8-core-diagnostic.md` ne permet pas de conclure à une expiration, une fraîcheur refusée ou un défaut corrigé. Aucun H9 réel exécuté par cette revue ; SIGKILL après Running, autonomie et reprise restent NOT_RUN dans H8. G09 reste FAIL, D5 ouvert, aucun gate fermé.

SHA finaux relus après le dernier renommage de message :

```text
2e9aab65b2465f3c265e0a6d0de8f0e4710cfbf3a4ee5c83a638337c3f16dd78 tests/broker_runtime.rs
fb83acf126022cdeec3e9202c5fbc6cede77778fe63e06e17d7c99df6a264fc4 tests/support/broker_runtime.rs
```
