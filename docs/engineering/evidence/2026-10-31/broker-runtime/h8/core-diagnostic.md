# H8 — dernier état confirmé, aucun diagnostic d'erreur disponible

HEAD revalidé avant essai : `af4a8682edf1bfcb18dcd08113e86f9724e9bcc1`, worktree alors propre. Après signal terminal Root, projections JSON whitelist et SELECT SQLite `-readonly` seulement. Aucun Store ouvert, Cargo, moteur, fournisseur, configuration brute, prompt, erreur libre ou secret lu ; aucune mutation de QA/source par ce reviewer.

## Faits codés

QA `/private/tmp/br-FV9BGW`, job `c885341c-e812-4e36-b683-e18657261447`, sous `b/journals/g09/broker-journal-v1/e0bc5bb276a81b788c4c1b969c0f48b35cc6f7d2ac21019b17e91ac065b0b3ab/jobs/<job>/`. Le pointeur annonce head33/hash `1ee0981cd0e089e9303fce5f4952bb2cd7cdd271eee08b541631c8d9b7dd01e6`. Projection des fichiers conservés, sans nouvelle ouverture applicative validant leur chaîne.

- **14/14 opérations supervision ACKées, zéro pending** dans ce head V3. Import et vérification, observations et cleanups sont ACKés.
- **Aucun `FailureObserved`, aucun V4/outbox, aucun `broker.batch.failure`**, dans toute l'histoire locale conservée ou les16 événements SQLite.
- Dernier ACK33 : Arm **worker** `596a58aa-f6af-4cc3-afaf-d359b408e770`, Store seq16 reçu19:44:48.273Z. Aucun WorkerClaim, même pending.
- Worker `634ac558-ffff-40bb-b9c2-38a9f5ae1c05` Created, revision4, quota retenu. Attempt `aa5427e6-9041-4ff6-8b27-316a24cd34a5` ; create op `98877f6e-22ba-4509-b56b-bf383fab2a3d` ; start op absent, zéro allocation.
- CID `800ec51a21e5cb8996a4f3de811fe447211408118e5c1bc5a561790c5d51381b`. L'inspection ciblée Luna conservée dans `docker-new-cid-selected.json` constate le même CID Created/runningfalse, StartedAt et FinishedAt non renseignés (`0001-01-01…`), nonOOM. Un ExitCode0 en Created n'est pas un terminal.

Il n'existe donc **aucun premier broker_step/enforcer.stage/reason/native_sample à rapporter** pour H8. PR4 a bien ces champs à la source, mais aucun événement d'erreur n'a atteint le journal conservé. Ne pas substituer un probe ultérieur ou une cause présumée à ces données absentes.

## Temps observés

Arm worker : timeout5s, sample natif `346047387655750`, deadline `346052387655750` ns, méthode Darwin, boot `df920bcd-1409-46e1-844a-47ba7ceb0e6c`, observed_utc null. Aucun sample natif d'erreur disponible ; expiration, fraîcheur750ms ou SlotPhaseMismatch **non établis**.

Mtimes epoch : stage Submit `1793475773.467847802`, pending Arm32 `1793475887.802635873`, réponse Arm33 `1793475891.771155138`, résultat FAIL publié `1793475894.005455811`. La réponse Arm33 n'est visible qu'environ118,303s après le marqueur Submit, et seulement2,234s avant la publication du résultat. Ces mtimes ne prouvent pas l'instant de retour du reçu Writer à l'executor : le postload strict peut encore être en cours après la publication du head.

Le log `h8.log:8` constate la limite préRunning120s ; durée122,63s, RC101. Le résultat est publié après retour de `run` et Drop des enfants possédés ; il ne date pas exactement le refus ou l'expiration de la boucle. Les conditions d'injection SIGKILL après Running n'ont pas été atteintes.

## Ce que cela explique, et ce qui reste inconnu

H8 est compatible avec l'interruption d'un travail encore en cours à la borne du harnais, **jusqu'au postload/reply de l'ArmExchange**. La présence du reçu Store dans la révision33 ne prouve pas que `enforcer.register` avait déjà été appelé. Aucune erreur persistée ne permet de localiser plus précisément cette fenêtre ni de conclure à un refus produit.

Différences à préserver : H4/H5 avaient un claim worker ACKé ; H7 avait une FailureObserved après Arm sans claim ; H6 s'arrêtait dans la préparation helper. H8 n'a ni claim ni FailureObserved. Les diagnostics des autres exécutions ne sont pas rétroactivement ceux d'H8.

**Aucun correctif causal de comportement n'est justifié par H8.** L'adaptateur lifecycle déjà fusionné supprime une lecture Full redondante, sans preuve de correction du défaut runtime. Les lectures/validations/synchronisations restantes et leur durée ne sont pas individuellement mesurées par ces fichiers. Ne retirer aucune barrière, ni rallonger/recréer l'Arm5s, ni réémettre claim/start, ni modifier génération/freshness pour obtenir le vert. Toute prochaine observation ou modification du harnais reste une décision Root ; aucune relance proposée automatiquement ici.

Gardien, QA et ressources restent conservés, D5 ouvert. Qualification du SIGKILL après Running, arrêt autonome à l'échéance et reprise sans deuxième start : **NOT_RUN dans H8**. Ce rapport ne clôture ni G09 ni une qualification produit.
