# Kubernetes — reprise avant émission et compatibilité schéma 13

Résultat constaté le 11 novembre 2026 à 20:33 UTC : **cinq parents logiciels PASS et compilation de toutes les cibles PASS sur le gel r8**, composé de la base publique `31453151b958c605a4200d6b0c3980d1fa3628bf` et des 54 postimages Kubernetes décrites dans [SOURCE_MANIFEST.json](SOURCE_MANIFEST.json). Les résultats, commandes, cibles et empreintes de sortie sont dans [TESTS.json](TESTS.json). Ce lot ne qualifie aucun moteur métier et ne ferme aucun gate de production.

## Comportement intégré

La reprise est limitée au post-état initial exact d'une réservation : `Reserved`, révision 1, époque 1, quota retenu et aucune émission possible. L'absence de Pod observé ou un délai dépassé ne remplace pas cette preuve. Le contrôle réévalue une identité de broker courante ; le créateur courant ou son successeur doit conserver le même broker stable et le périmètre requis de tenant, namespace de contrôle, autorité et runtime Kubernetes. Les bindings et révisions du credential et du grant courants sont vérifiés avant toute consultation d'un reçu historique. Une preuve descriptive renvoyée par l'API ne reconstitue pas une capacité d'exécution.

L'API V3 ferme cette réservation avant émission dans une transaction qui lie état, libération du quota et audit obligatoire. Un même identifiant d'opération et les mêmes entrées retrouvent le reçu exact ; une divergence est refusée. Le `GET` du reçu est une lecture autorisée sans mutation. Les chemins V1/V2 restent fermés aux preuves V3. La compatibilité conserve les couples existants, dont `(12, 2)`, et ajoute explicitement `(13, 2)`.

Le passage 12 → 13 est une migration explicite, avec sauvegarde préalable et validation du marqueur et de l'intégrité. Une simple métadonnée de schéma 13 ne répare pas le marqueur et n'active aucun Pod, démarrage ou release runtime. Les [contrats API](../../../../api.md) et les guides Kubernetes [anglais](../../../../setup/kubernetes/README.md) / [français](../../../../setup/kubernetes/README.fr.md) accompagnent les sources ; les contrôles sont dans [recovery.rs](../../../../../src/audit/kubernetes_admission/recovery.rs), [model.rs](../../../../../src/audit/kubernetes_admission/model.rs) et [migration.rs](../../../../../src/audit/kubernetes_admission/migration.rs).

## Preuves séparées par source

| Campagne | Source SHA-256 | Observation strictement locale à cette source |
|---|---|---|
| r5, base publique `1dc676b` + K54, 2 478 fichiers | `d6d3b711d84f6a74258c4920d884aee5e327d183c210d999e94dae514b004e99` | 4 parents PASS : migration, sauvegarde retenue, reçus checkpoint 12 et lecture de fédération. Puis échec de fixture. |
| r6, base publique `1dc676b` + K54, 2 478 fichiers | `9fde5e90a2e76d1d7b30b10d31e4d8e56932a9aa798a54254104fee98c0007f0` | 13 parents PASS : compatibilité pending, identité hors ligne, protocole et contrôles de reprise HTTP. Puis échec de fixture. |
| r8, base publique `3145315` + K54 corrigé, 2 497 fichiers | `0c96db41d708f98eef96c4cbb38775db5aa32b9da3c757070383f278ea9f2ba7` | 5 parents PASS : identité/périmètre/refus worker, validation descriptive, reprise idempotente après acquittement perdu et lecture sans mutation. Build des quatre artefacts, quatre listes et `cargo check --all-targets` PASS. |

Ces observations ne sont pas 22 PASS sur une source commune. Les tests historiques Core10 d'une source privée et les 14 parents du lot D ne sont pas transférés. Les cinq noms actuels, leurs cibles et leurs sorties brutes hashées sont conservés dans le registre JSON ; aucun journal brut ou chemin privé n'est publié ici.

## Échecs conservés et corrections des fixtures

| Campagne | Résultat | Cause et suite |
|---|---|---|
| r3 | FAIL build, RC 101, aucun test | Appels privés depuis le test d'intégration ; remplacement par le parcours HTTP public. |
| r4 | FAIL build, RC 101, aucun test | `unwrap_err` exigeait `Debug` sur le DTO de succès ; assertion typée équivalente. |
| r5 | 4 PASS puis FAIL, RC 101 | Bail synthétique de 30 s invalide face à l'intervalle existant de 30 s : fixture portée à 60 s, expiration associée ajustée. Aucun délai produit modifié. |
| r6 | 13 PASS puis FAIL, RC 101 | Fixture d'un autre broker stable réutilisant l'autorité historique du premier ; autorité propre et contrôle positif HTTP avant les refus attendus. |
| r7 | FAIL build, RC 101, aucun test | Comparaison de test `String` / `Uuid` ; opérande canonique corrigé. |

Les deux dernières corrections ne touchent que le test. Leurs revues indépendantes et le défaut de typage manqué lors de la première revue restent tracés par leurs empreintes dans [TESTS.json](TESTS.json). Aucun ancien échec n'est réécrit en réussite.

## Confinement de QA et limites

La campagne r8 s'est exécutée dans une QA Linux aarch64 isolée, image épinglée, UID 1000, réseau absent, racine en lecture seule, capabilities retirées, no-new-privileges et seccomp. Cargo est hors ligne, avec un seul job ; les ressources sont bornées à 4 CPU, 9 Gio de mémoire sans swap, 128 PID et un scratch tmpfs de 4 Gio. Cette utilisation de Docker comme sandbox de compilation n'est pas une qualification du runtime Docker du produit.

L'export réel de la source encore vivante contient 2 497 fichiers et correspond au manifeste d'entrée `0701956fd901908f811c50b7ab79a868aa70ac979206a4f7c14aec51f648d887` ; son archive SHA-256 est `a3b0d5e97a499120d90e9d80158cba90d92e096376e46b448f6eed9ac9d27044`. Il est observé avant l'arrêt du holder. Fermeture ciblée RC 0, aucun OOM observé. Les mesures mémoire publiées concernent le cgroup de QA et ne sont pas une mesure de worker, de latence ou de capacité.

**NOT_RUN ou gates ouverts pour ce lot :** Docker/Podman métier réels, cluster Kubernetes et émission/release de Pod, composition Full3 privée, fournisseur IA, macOS natif, charge 100 workers, Clippy strict, suite complète, restauration de déploiement et médiation egress. La composition du checkout d'intégration et les documents de preuve ajoutés ensuite ne sont pas rétroactivement couverts par le gel testé. Les risques d'audit globaux, les autres pièges de quota et le lot E restent hors de cette clôture logicielle limitée.
