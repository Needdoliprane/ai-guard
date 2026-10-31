# Revue indépendante courte — fenêtre d'observation H6

Lecture seule du worktree broker-service, HEAD `99238e3`, état Git propre avant revue. Aucune source modifiée, aucun Cargo, moteur, fournisseur, Store ou credential ouvert. Le second regard a relu séparément les mêmes oracles, sans exécution.

## Conclusion sur le delta proposé

**Oui : remplacer uniquement observation 45→120 s et borne globale 180→240 s conserve les oracles worker demandés. Aucun P0/P1 reproductible ni chemin de faux PASS attribuable à ce delta identifié.** Delta minimal dans `tests/broker_runtime.rs` : ligne45 `from_secs(180)`→`from_secs(240)`, ligne47 libellé `180-second`→`240-second`, ligne207 `from_secs(45)`→`from_secs(120)`. Aucun changement de timer applicatif, Arm, claim, rearm, sélection moteur ou nettoyage.

- Refus Submit : FAIL immédiat (:203). La boucle légère de 200 ms reste bornée ; identité remplacée, disparition après observation, Terminal/Cancelled avant Running et erreurs HTTP restent FAIL (:216–244). Attendre davantage ne suffit jamais à produire PASS.
- Le worker reste demandé avec timeout de 5 s (:168/:186), confirmé contre l'Arm durable originale (:277–282). Avant la faute, il faut un worker central Running, une seule Arm liée au même worker/attempt, un gardien sain Ready/prepared_once, puis Docker Running réel et même CID/image/StartedAt (:228–332).
- Le broker Child possédé doit réellement sortir de SIGKILL, puis être reaped avant la deadline worker originale ; l'échantillon natif doit conserver boot/méthode (:333–352). Le test ne déduit pas la faute d'une simple demande kill.
- L'arrêt doit être observé au plus tard deadline+3 s, jamais par absence/erreur inspect ; exit137, nonOOM, durée réelle strictement positive et inférieure à 20 s (:356–396). Le snapshot exact Quarantined du gardien, son horloge et son epoch restent requis (:401–440).
- Après restart, même Arm/deadline/clock, attempt/CID/claim/StartedAt et instance Docker ; aucune nouvelle émission worker Arm/claim, aucun usage dupliqué et quota réellement libéré, audit valide (:460–573). Les nouveaux helpers de FinalScan ne sont pas assimilés à un nouveau worker.

## Ce que H6 établit, et ce qu'il ne permet pas de conclure

Captures sélectionnées lues dans `/private/tmp/ai-guard-runtime-h6.mLgMJm` : log du test FAIL après 48,25 s, RC101 ; stage `submit_real_worker` ; worker Preparing rev2 sans CID/start ; résultat FAIL. L'assertion de la boucle de 45 s est bien le motif constaté. La faute broker/worker de la recette n'a pas été atteinte.

La projection de métadonnées de Core, relue dans `/tmp/ai-guard-runtime-h6-core-diagnostic.md`, donne 9/9 opérations ACK, head22 HelperClaim ImportVerification, zéro FailureObserved et zéro événement diagnostic ; aucune nouvelle lecture Store effectuée par ce relecteur. Cela n'établit aucun refus produit. La capture Docker sélectionnée montre le helper exact `f3ed32a35cd3ed767beacda7c01029f1c8033353990915c3fb8052cf0f88218a` réellement terminé : Started 18:58:07.705873553Z, Finished 18:58:07.87673872Z, exit0/nonOOM/restart0. Ce fait concerne le helper, pas le démarrage du worker.

Le timeout helper de 120 s vient de la politique applicative fixe (`src/control/supervision.rs:730/:749`), utilisée pour l'échéance native à l'armement (`src/runner/broker/executor/helpers.rs:39–71`). La projection Core indique environ 115,877 s encore au claim, sans inférence d'expiration. La différence entre mtime ACK22 et publication resultFAIL est environ 7,49 s ; ces mtimes et l'heure de fin de capture ne sont pas l'horloge native de l'erreur.

`OwnedProcess::Drop` (`tests/support/broker_runtime.rs:231–240`) tue/reap uniquement ses enfants broker/gateway au retour Err ou annulation du futur. Il ne découvre ni ne tue le gardien. H6 a donc interrompu l'observation et les contrôles possédés avant d'établir la cause ; ressources et gardiens conservés.

## Limites à conserver pour H7

120 s reste une fenêtre d'observation globale de préparation, pas une garantie d'épuiser le budget de chaque helper de 120 s armé tardivement. Un nouveau timeout resterait **FAIL d'observation, cause non établie**, et non une preuve de refus ou de panne applicative. L'élargissement retire seulement le plafond de préparation de 45 s ; il ne corrige ni ne qualifie une performance produit. L'échéance stricte worker de 5 s et tous ses oracles restent inchangés. Aucune tentative H7 exécutée par le relecteur, aucune autorisation de nettoyer les anciennes ressources ; G09 demeure FAIL jusqu'à une preuve réelle du périmètre.

Sources relues avant delta :

```text
70681753f75d8b3b9636f9377d0ca29beaab2a7b3f03f4d8725deb4132cbc73d tests/broker_runtime.rs
5ee0fbad8eae087c309031f3645754fca425de9ac9251ce0e9880b7bb82becd5 tests/support/broker_runtime.rs
47ccfc4ad5ee9d903ca2290863541bddb4cb86b7e0630dce148094405f405a19 src/runner/broker/executor/helpers.rs
```
