# H5 — diagnostic domaine/journal, lecture seule

Base produit déclarée inchangée `e7cc0bb25dd1b8ab81e63bb5ed44cf8c55661efa`. Lectures : JSON ciblé des révisions et pointeur de `/private/tmp/br-NBcV3F/b/journals`, métadonnées `stat`, `sqlite3 -readonly` avec SELECT limités aux types/UUID/états/seq/horodatages. Aucun Store ouvert, moteur, service, test, Cargo, secret, config brute, prompt ou environnement lu ; aucune mutation de QA/source.

## Établi

Job : `6d08d387-29ad-4501-9987-f5546f78ba83`, sous `g09/broker-journal-v1/0b6f29137b90178bb1cf5e5daff8981f97d6663764c072b907533d10b75c58dd/jobs/…`. `current.json` pointe rev37/hash `7fed01a653e0382d60c002e6b76f06cb0f76645336a5be9515031e61950a391d`. Il s'agit de la projection des fichiers conservés, sans nouvelle validation applicative de la chaîne.

| Étape | Révision / reçu | Identité |
|---|---|---|
| Created | ACK31 / SQLite seq15 | worker `c252e676-d039-4c02-bcf4-18f6fdd92c36`, attempt `d5a4d221-7a78-4c3b-a509-326f64961b12` |
| Arm worker | pending32 → ACK33 / seq16 | `1d078937-ad5a-4c3f-9231-a2367c916740` |
| Claim worker | pending34 → ACK35 / seq17, `emit_once` | `fd41987e-09ad-4165-b664-e8c4b6541412`, emitter `4f53ff5c-f0da-44a9-aa3a-a2d0f077b067` |
| Première FailureObserved | rev36 | `cac39af0-0784-4a62-ad99-bb7c7e4ddc1f`, phase `worker`, code `unavailable` |
| Deuxième FailureObserved | rev37 | `4cdb2ffb-884c-4e94-8928-7e0c34ca8dfc`, mêmes phase/code |

Les **15 opérations supervision sont ACKées, zéro pending** au head. Import, ImportVerification et leurs cleanup sont engagés. Journal et SQLite gardent le worker `start_issued`, revision5, quota retenu ; zéro allocation. CID durable : `e22601834084d4cee79a4ffb0bc700656a4b420e2c7f89bc25c2eab64bd60db9`. Aucun Observe Running ou terminal n'a suivi le claim. Ce résultat ne vient donc ni d'un pending HTTP laissé sans ACK, ni d'une admission/quota refusée, ni d'une absence d'Arm.

Le claim n'est émis qu'après `enforcer.register` puis `check_ready_for_claim` (`src/runner/broker/executor/helpers.rs:90–122`). Le WorkerClaim ACK établit que cette barrière Ready préalable a réussi ; il ne prouve pas que Prepare ou le guard final ont réussi. `prepared_once=true` observé ensuite ne prouve pas un Prepare initial : le Register `observe_only` de la reprise le positionne aussi (`src/broker/enforcer/server.rs:313–318`, `330`).

## Temps : borne connue, offset inconnu

Arm native `340604839052166 ns`, deadline `340609839052166 ns`, claim.clock_check `340607474064125 ns`. Au sample du claim : **2,635011959 s écoulées, 2,364988041 s restantes**, avant publication pending/ACK et postload. Méthode `darwin_mach_continuous_v1`, boot `df920bcd-1409-46e1-844a-47ba7ceb0e6c`, `observed_utc=null`.

Mtimes des fichiers rev32→37 : `1793470441.758118812`, `.0442.544637004`, `.0444.180635347`, `.0445.049956581`, `.0446.947067182`, `.0453.143990376` (les suffixes abrégés gardent le préfixe179347). SQLite seq16 reçu `2026-10-31T18:14:01.941Z`, seq17 `18:14:04.367Z`.

Ces horodatages sont cohérents avec une fenêtre courte consommée par le chemin postclaim. **Aucun offset natif↔UTC exact n'est stocké** : mtime et received_at ne sont pas des samples simultanés d'horloge continue. On ne peut donc pas prouver ici que l'échéance était dépassée à Prepare/spawn. H4 gardait1,984240750 s au claim ; H5 conserve2,364988041 s et échoue au même bord malgré le retrait du polling RecoveryStatus préRunning. Le polling H4 n'est pas une cause suffisante.

## Ordre critique et premier lot

`helpers.rs:153–180` persiste la réponse claim puis appelle `slot.prepare_start`. Après retour du callback (`runtime/managed.rs:1116`), `runtime.rs:1101` attend encore `verify_authority` (Docker info avec vérifications d'invocation), `1102` construit la commande avec une vérification supplémentaire de l'ancre exécutable. Puis `ensure_prearmed_live` et guard final sont adjacents au spawn (`1113–1119`). Le guard vérifie phase/génération, notification stop, fraîcheur750ms et deadline (`enforcer/client.rs:854–890`). Le refus exact peut être Prepare, perte de santé/fraîcheur, expiration ou un autre contrôle ; `worker/unavailable` ne permet pas de les distinguer.

Premier lot pertinent : le diagnostic typé borné préparé par Root doit identifier **étape + code + current/deadline native**, sans argv/env/secret. Si le refus est la fraîcheur du contrôle entre Prepare et spawn, une correction ciblée consiste à placer la consommation Prepare **après les contrôles coûteux moteur/commande**, en conservant toutes leurs vérifications et le guard final sans await jusqu'au spawn. Cela exige une petite couture explicite du port préarmé, à relire ; déplacer seulement `verify_authority` avant le long callback affaiblirait le recoupement postclaim et n'est pas recommandé. Ne pas refaire claim, prolonger/recréer l'Arm ou relever la fraîcheur pour faire passer l'essai.

Coût Journal établi, responsabilité non prouvée : `load_locked` relit/revalide et `persistent_sync` chacune des révisions0..head (`journal.rs:1369–1374`), ainsi que réservation/accepted/current via `read<T>`, intent/bindings et répertoires. `advance_checkpoint` garde Full préload1025 et postload1075 autour du publisher330–344. **Correction d'une imprécision H4 : `read_file`1432–1439 ne sync pas ; `read<T>`1440–1452 et `read_canonical`1454–1468 sync. Il n'y a pas de double sync de chaque révision par `read_file`.** Le reader STOP vivant a pour sa part `synchronize_reads=true` (`recovery.rs:734`), et sync ses fichiers dans `read_file:949–962` ; le reader startup est false. Ces barrières protègent les confirmations : aucune suppression n'est justifiée par ces seuls timings.

Conclusion causale : dernier succès durable précisément localisé au claim ; première Failure localisée à worker. Le mécanisme de refus préspawn reste **UNKNOWN**, à faire émerger par le diagnostic typé avant un correctif de comportement ou une nouvelle tentative réelle. D5 et nettoyage restent hors preuve ; ressources conservées.
