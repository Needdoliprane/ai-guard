# H6 — frontière observée, sans correction produit présumée

Produit gelé D3 `099238e3f38ac851a0a98813ec24ef9c5a2cea950`. Inspection seulement après signal terminal Root : JSON journal projeté, métadonnées `stat`, SELECT `sqlite3 -readonly`. Aucun Store ouvert, Cargo, test, moteur, fournisseur, configuration brute, prompt ou secret lu ; aucune mutation de QA/source par ce reviewer.

## Faits persistés

QA `/private/tmp/br-6dPLTS`. Job `4052b605-3517-4503-bd40-c39c44d152ee`, chemin `b/journals/g09/broker-journal-v1/8b70971d43cd8bdc0ed5f7fbcca7a88a9d8d0d3d86eb1882d01fbb38e98cb079/jobs/<job>/`. `current.json` annonce head22/hash `f9a1d806e01c4216f38d87f8326d0b5913b297b265094e0ef67ee45cae937add`.

- V3 : **9 opérations supervision sur9 ACKées**, zéro pending. Aucun `FailureObserved`, aucun V4/outbox et aucun `broker.batch.failure` dans les révisions conservées. SQLite contient11 événements, aucun diagnostic batch.
- Worker `78b2c9a7-e2be-4128-9142-51c66fee7054` : `preparing`, revision2, quota retenu. Attempt `1dde165d-4667-41e9-9770-53fb6471f21d` ; CID central/create/start IDs tous absents. **Aucune Arm worker n'a été créée.**
- Import : helper `24656752-18df-4837-a4a2-0ec3657ed912`, Arm `5bcaab4e-f13d-4f87-bc15-019ad0b5d256` (ACK10/seq6), claim ACK12/seq7, observation exit0 ACK14/seq8, cleanup ACK16/seq9.
- Vérification d'import : helper `60dfa89f-e0ad-48d9-bf00-1ae30578964a`, CID `f3ed32a35cd3ed767beacda7c01029f1c8033353990915c3fb8052cf0f88218a`. Arm `96998437-24ad-4968-a612-82f7fd5c1f0d` ACK20/seq10. **Dernier ACK22** : claim `87a3f1c1-ac78-4099-832c-e02a0bd54a66`, `emit_once`, seq11. Aucun HelperObservation suivant n'est encore durable.

Arm du dernier helper : timeout120s, sample `343244641647666`, deadline `343364641647666`, claim.clock_check `343248764547958` ns. Il restait115,877099708s à ce sample, avant ACK local. Ce n'est pas l'Arm worker5s. Aucun stage/reason/refus Prepare ou BeforeSpawn n'est prouvé par H6. L'absence d'un diagnostic n'est pas une preuve générale d'absence d'erreur ; elle borne ce qui est effectivement disponible.

Projections exécutées : `jq` liste exclusivement revision/version, kind/operation_id/présence de réponse, phases/codes locaux et champs fermés des événements exacts `broker.batch.failure`. SQL sélectionne `events.seq/kind/received_at` et les seuls champs diagnostic `$.data.{phase,runtime_effect,enforcer.*,runtime.stage,runtime.container_id}` sous `WHERE kind='broker.batch.failure'`, ainsi que UUID/état/revision/quota des tables workers/mutations/arms/helpers. Aucun payload brut affiché.

## Chronologie et arrêt du harnais

Métadonnées locales en secondes epoch : stage `submit_real_worker` **1793473047.758476834** ; ACK22 **1793473086.388616347** ; publication `result.json` FAIL **1793473093.879424470**. Ce dernier timestamp indique la publication du résultat, **pas l'instant exact de l'erreur**. `runtime-h6.log:7` rapporte précisément l'échec de l'oracle « worker never observed Running… », durée48,25s.

L'inspection ciblée Luna fournie par Root (RC0, `h6-container-scalars.txt`, SHA `6f42d309bf13265906886a01b671be77d4d258859ad9124edebb79ea9b62f0bf`) constate le même helper créé18:57:57.430961798Z, démarré18:58:07.705873553Z, terminé**18:58:07.87673872Z**, exit0/nonOOM/restart0. Il a réellement progressé après l'ACK22, puis terminé environ6s avant publication du résultat FAIL. Son temps de processus constaté est0,170865167s. Ne pas utiliser la fin globale de capture18:58:47Z comme heure de l'erreur.

Source du mécanisme : le test fixe sa borne préRunning à45s **après Submit** (`tests/broker_runtime.rs:206–214`). Le marqueur submit est écrit avant ce Submit (`178–179`). Le helper finit environ40,118s après ce marqueur, donc dans cette fenêtre selon les horodatages muraux conservés ; pas d'offset natif↔UTC simultané revendiqué. Au retourErr, `OwnedProcess::drop` appelle `child.kill()` puis reap sur les seuls enfants broker/gateway (`tests/support/broker_runtime.rs:231–240`). Le résultat est écrit après le retour de `run` (`tests/broker_runtime.rs:45–53`). Ce n'est ni la borne globale180s, ni le SIGKILL worker-running prévu par l'oracle ; le teardown peut interrompre l'exécution encore en cours avant la prochaine publication.

## Conclusion et lot pertinent

**H6 établit une borne d'observation du harnais atteinte pendant une préparation ayant encore réellement progressé ; il n'établit pas un refus préspawn produit.** H4/H5 gardent leur cause inconnue et leurs frontières différentes. Rien ne prouve que H6 aurait ensuite atteint Running sans autre refus.

Correction ciblée proposée au propriétaire du test : porter la seule attente préRunning45s à90s, avec borne globale180s conservée, même polling HTTP200ms, mêmes conditions de refus et preuve de progression. Conserver **strictement** worker Arm5s, commande sleep20, guard Ready/prepared_once, clock/deadline originales, SIGKILL du seul broker après Running, contrôle terminal original et absence de deuxième start. Il s'agit d'une correction du budget d'observation, sans garantie de délai produit ni PASS rétroactif. Aucune nouvelle tentative autorisée par ce rapport ; Root reste décideur de l'exécution.

Aucune correction runtime/Journal n'est justifiée par H6. Le coût O(historique) des relectures et synchronisations Full reste un constat source (`journal.rs:1369–1393`, pré/postload1025/1075, adaptateur writer873) ; sa contribution causale aux refus H4/H5 n'est pas mesurée ici. Ne supprimer aucune validation/barrière, ne renouveler ni claim ni Arm, ne modifier ni génération ni freshness pour ce lot. Ressources et gardien restent conservés ; qualification SIGKILL/deadline : **NOT_RUN dans H6**.
