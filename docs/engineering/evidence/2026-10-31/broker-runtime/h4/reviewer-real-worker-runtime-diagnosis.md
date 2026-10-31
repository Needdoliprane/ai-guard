# H4 : premier diagnostic réel, lecture seule

31 octobre 2026. QA conservée `/private/tmp/br-dFE99I`, capture `/private/tmp/ai-guard-runtime-h4.1Y3B4Z`. Aucun Cargo, processus applicatif, commande moteur ou changement source effectué par cette revue. Rapport privé 0600.

**Confirmé : panne après Ready et claim durable, avant Running.** Le titre « Created bloqué avant Ready » ne décrit pas les faits : `worker.json` contient `state=start_issued`, révision5, CID `a87643c3d8bcffff916f79807ecf7d09469c1b399ec8687951d6ff3b22d82ecb`, claim `cf6c7c6d-ebd5-4aa2-8933-7949ff0ea4c7`. Core a recoupé les15 opérations ACKées : Arm worker `771ffb47-1928-4350-9412-28d72549f2cd` ACKrev33, claim EmitOnce ACKrev35, puis Failure worker/unavailable rev36–37 ; aucun pending. `helpers.rs:90–127` exige register+Ready avant l'émission de ce claim. L'hypothèse d'un reopen initial de l'actor toujours indisponible est donc écartée pour ce worker.

## Fenêtre qui consomme la deadline

Core extrait : Arm.clock=339650512620458ns ; deadline=339655512620458ns ; claim.clock_check=339653528379708ns. Il ne reste que1.984240750s au clock_check, AVANT pending et ACK durables du claim. Métadonnées FS lues sans ouvrir les payloads :

| Révision | mtime Unix secondes |
|---|---:|
|32 pendingArm|1793469487.346486828|
|33 ACKArm|1793469488.543166645|
|34 pendingClaim|1793469490.481406284|
|35 ACKClaim|1793469491.636534726|
|36 Failure|1793469492.580795587|

`helpers.rs:153–180` attend cet ACK avant `EnforcedSlot::prepare_start`. Celui-ci revérifie santé et deadline (`enforcer/client.rs:773–788`). Même si Prepare réussit, `runtime.rs:1101–1116` attend une nouvelle vérification d'autorité/CLI, prépare la commande puis consomme le dernier garde ; ce garde refuse deadline/état/horloge périmés (`client.rs:845–890`). L'actor passe en arrêt dès la deadline originale (`server.rs:423–455`). Aucun réarmement ne doit contourner ces refus.

**Hypothèse forte, pas encore cause terminale prouvée :** le budget5s est épuisé pendant les validations/persistances entre Arm et dernier garde. Le code ne permet pas de distinguer, dans le journal H4, expiration dans Prepare, refus de fraîcheur/phase au dernier garde, ou erreur CLI après Prepare : `executor/journal.rs:123–137` réduit toute erreur à FailureObserved/Unavailable ; broker stderr est vide. Les scalaires du slot existant (`prepared_once`, phase, clock, observation/CID) restent nécessaires pour resserrer cette fenêtre. Ne pas appeler cela une dépendance externe : moteur en ligne et claim central réel.

## Point de harnais confirmé et correction minimale proposée

Le polling actuel appelle RecoveryStatus avant chaque recherche Running (`tests/broker_runtime.rs:178`) puis dort50ms. Ce n'est pas une lecture bon marché : `service/server.rs:478–490` → même acteur Writer (`writer.rs:785–797`) → mutex writer et `read_locked/read_job` de la chaîne (`journal/recovery.rs:194–228,296–301`). Le harnais ajoute donc des lectures complètes concurrentes aux pending/ACK de sa propre fenêtre5s. Sa contribution précise n'est pas mesurée ; ce n'est pas une preuve de cause exclusive.

Avant une nouvelle tentative réelle : poller les lectures HTTP légères `/api/v1/workers` puis `/workers/{id}` de cette base neuve à un worker ; après Running, lire UNE RecoveryStatus authentifiée pour l'Arm d'origine et interroger le gardien. Conserver strictement timeout5, Arm/deadline/CID, oracles et interdiction de re-claim. Capturer sur échec les seuls faits statut/phase/clock/observation du gardien existant, sans secret ni nouveau child ; rendre l'échec immédiatement utile. Aucune hausse de timeout/health-window, suppression de fsync, nouvelle durée ou permission stockée n'est une correction acceptable.

Le défaut produit de diagnostic opaque reste ouvert ; si le parcours sans polling lourd dépasse encore5s, il faut mesurer et réduire le coût réel de la séquence durable/validation, sans supprimer ses garanties. Une course de snapshot Prepared/Slots existe comme candidat source distinct, mais aucune preuve H4 ne permet ici de la désigner cause.

**Statuts :** H4 réel FAIL avant injection SIGKILL ; preuve autonome G09 NON ACQUISE. Source/quota/claims/ressources conservés, aucun nettoyage ni arrêt du gardien. D5 et boucle stop persistante restent des écarts connus, sans rapport avec un PASS du démarrage.
