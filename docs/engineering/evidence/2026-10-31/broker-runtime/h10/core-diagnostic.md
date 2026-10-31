# H10 — état durable sélectionné et frontière d'erreur

Après signal terminal Root, lecture seule autorisée de l'unique QA `/private/tmp/br-euAmXg` : `sqlite3 -readonly`, `PRAGMA query_only=ON`, SELECT scalaires fermés ; projections jq whitelist du journal et métadonnées stat. Aucun Store applicatif, guard, moteur, probe, config, auth, provider, env ou erreur libre ouvert ; aucune mutation de QA/source. Gel `6c7b63ebcef4a92cadfc2ec95626033f48011576`. Le log sélectionné `/private/tmp/ai-guard-h10-20261031-223221/h10.log` constate la borne globale de 600 s, une durée de 600,04 s et RC101.

## Derniers faits disponibles

Job `f995099b-1820-4bf6-b33e-446a77fe1eb1`, scope journal `5253597fc62135a72e307aadd36e0427b08b3189f1a8389a66874c87f5ea51c6`. Le pointeur annonce **head30/hash `c07a77715db4789d6957ab882d1e57a97bf922673ce0a20f853b30c7a94cd51e`**. Les 31 révisions 0..30 conservées sont projetées ; aucune nouvelle ouverture applicative ne valide ici toute leur chaîne.

- **12/12 opérations supervision ACKées, zéro pending.** Aucune WorkerArm ni WorkerClaim, même pending. Les deux Arms sont des helpers.
- **Première et seule FailureObserved : rev30**, commande `eee11600-eaae-47a0-9874-3c1ca253752c`, phase `worker`, code `unavailable`.
- **Aucune révision V4/outbox ni événement `broker.batch.failure`** dans le journal ; SQLite contient 14 événements, aucun diagnostic batch et aucune allocation.
- Dernier ACK de supervision : **CreateIssued rev28**, opération `a50dd1a1-5840-427b-a619-8268d0ec6757`, audit seq14 reçu `2026-10-31T21:35:35.588Z` ; présent ensuite aux heads29/30.
- Worker `378bcf99-f90f-433f-948e-1e69d5c66c12` : **Preparing, revision3, quota retenu**, 2000 milliCPU/1 Gio alloués. Attempt `61e9f4a3-3804-41ed-8a4b-f83ac83b7978`. CID central/start op/emitter absents ; create op présent. Ce n'est ni un terminal ni une preuve d'usage réel nul.

**Différence importante entre journal et registre central : rev29 persiste `managed.owned`.** Handle/observation portent le CID `431d660f00012b1abcd7c34604529dfe915914b91880f16b3e2a25f7c7da6e70`, image `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`, process `created`, running=false, exit_code0, StartedAt/FinishedAt absents, fingerprint version2. Ce sont des faits runtime retenus par le programme, pas une nouvelle inspection indépendante. Un ExitCode0 de Created ne prouve pas un terminal. Aucun `Created` pending ou ACK n'est présent : le contrôle central n'a donc pas encore confirmé ce CID. L'absence de nouveau CID observé dans le parcours du harnais ne signifie pas absence de snapshot de propriété.

## Helpers et clocks

| Fait | Import | ImportVerification |
|---|---|---|
| Helper | `27b5b9a4-c626-4a92-9af1-ebd244d5ead3` | `1106de37-7745-4880-b027-9cd3b772b72a` |
| Arm | `bc959017-9d80-43dd-a69a-86a3eda2eec7`, seq6 | `3bd46644-d90e-47f3-877d-c2507cee5101`, seq10 |
| Claim | `95345db5-6c85-4981-ad63-1c8599c5465c`, seq7 | `0ae0dc1e-b2bf-43d7-8b1a-c8e667200e45`, seq11 |
| Armed/deadline ns | `352565155206541` / `352685155206541` | `352615630784583` / `352735630784583` |
| Claim clock ns | `352570143607458` | `352628174827625` |
| Fin constatée et cleanup | observation seq8 exited/exit0/runningfalse ; cleanup seq9 ACK | observation seq12 exited/exit0/runningfalse ; cleanup seq13 ACK |

Ces deux Arms ont un timeout de 120 s, méthode `darwin_mach_continuous_v1`, boot `df920bcd-1409-46e1-844a-47ba7ceb0e6c`. Il restait respectivement 115,011599083 s et 107,455956958 s au sample de claim enregistré ; ce ne sont pas des samples de l'erreur. **Aucune Arm worker de 5 s n'est disponible à comparer.** Aucune raison native, native_sample ou slot_snapshot d'erreur n'est disponible. Ni expiration worker ni SlotPhaseMismatch H9 ne peut être attribué à H10.

## Frontière source établie

`executor.rs:523–538` attend Import/ImportVerification, puis `mutate(CreateIssued)` et appelle `execute_worker`. Celui-ci fixe phase **Worker** dès `:616`, avant ses lectures/validation runtime. La FailureObserved worker établit donc que cette phase a été atteinte ; le code `Unavailable` est générique, posé pour toute erreur par `executor/journal.rs:164–177`, pas un code de refus précis du moteur ou du gardien.

Dans `execute_worker`, le callback `:695–713` publie l'OwnedContainerSnapshot ; `:714–720` fait ensuite load, vérification `created_inspection`, construction `managed::inspection` puis mutation Created. `arm_and_claim` vient seulement après (`:728`). Le runtime valide/inspecte le handle avant ce callback (`runtime/managed.rs:1093–1116`).

Les faits bornent donc l'erreur **après publication de l'ownership rev29 et avant publication d'une mutation Created**, non dans Ready/Prepare/BeforeSpawn du worker. Restent possibles le retour/postload du patch d'ownership, le load suivant, la construction/validation d'inspection ou la préparation durable de la mutation Created. Le contenu du pointeur ne prouve pas à lui seul le retour du reçu au caller. Aucune donnée codée ne choisit entre ces opérations.

Le transport préarmé (`duration=None`) conserve l'instance sur erreur sans attendre un nouvel arrêt/nettoyage (`runtime/managed.rs:1163–1174`). Cette observation source n'établit pas la cause de l'inventaire externe. Le snapshot local Created ne prouve pas l'état actuel du CID.

`failure()` tente d'abord FailureObserved, puis ne produit le nouvel événement que pour les types reconnus ; `classify` peut être vide, ou le chemin fact_event/enqueue peut échouer avant V4. **L'absence d'événement ne prouve pas absence d'erreur** : l'erreur locale est ici effectivement enregistrée. Le harnais ne lit que le filtre HTTP `broker.batch.failure` pendant cette attente, pas cette commande legacy du journal ; Preparing reste non terminal et la boucle peut donc atteindre le global de 600 s malgré FailureObserved.

## Chronologie, limites et décision

Mtimes epoch sélectionnés : Submit `1793482361.459873620` ; ACK28 `1793482542.388841997` ; owned29 `1793482561.490163957` ; Failure30 `1793482588.733356434` ; publication resultFAIL `1793482959.455010116`. Failure30 est publiée environ 227,273 s après le marqueur Submit et 370,722 s avant le résultat final. Ces métadonnées datent les publications, **pas l'instant exact de l'erreur, le retour d'un reçu ou un intervalle natif de coût**.

Intervention concomitante rapportée par le mainteneur : il a « supprimé du cache » pendant H10 ; commande, cible et portée inconnues. Cela reste un facteur de contexte, **pas une causalité établie** ni une reproduction isolée. Root transmet une whitelist Luna (`qa-preflight-identity-projection.json`) où l'engine ID préflight correspond à l'actuel `311179c5-f7e1-4130-bba1-50e8d3d7f537` ; aucune continuité d'epoch n'est prouvée par cette égalité. Cette information est attribuée à Root/Luna, sans lecture ou recherche additionnelle par le relecteur.

H10 est **FAIL d'observation du parcours avec couverture diagnostique manquante**, avec erreur locale worker générique enregistrée avant l'injection de faute. Les faits les plus précis restent owned rev29, puis FailureObserved rev30, sans Created pending/ACK, WorkerArm ou WorkerClaim. Running+Ready, SIGKILL après Running, arrêt autonome et reprise sans deuxième start ne sont pas établis par cet essai. Aucun motif causal ou correctif comportemental n'est justifié par le seul code Unavailable. L'anomalie d'inventaire 37→3, en revalidation séparée, n'est ni utilisée ni causalisée ici ; aucun moteur/guard interrogé. Aucune action de nettoyage par cette revue ; conservation des preuves et ressources demandée, leur continuité globale non certifiée. **Aucun H11 ni nouvelle tentative proposé/exécuté.** G09 reste FAIL, D5 non qualifié.
