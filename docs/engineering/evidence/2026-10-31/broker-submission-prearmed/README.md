# Préparation du broker et transport batch préarmé

31 octobre2026, heure locale Europe/Paris ; base `7098e204a91182229c1a67bc8017271cb3d20918`, worktree `broker-service`. [Implémentation soumission](implementation-submission.md), [rapport Luna](tests.md), [revue indépendante](review.md). Verdict final **CLOSE logiciel/docs/preuves/artefacts au périmètre des deux ports**,RPT-01 corrigé, aucune réserve bloquante résiduelle. Le [cycle actif](../../../IMPLEMENTATION_PLAN.md) reste le service permanent complet. Ces coutures sont des composants réels du raccord ; elles ne constituent pas encore ce service et ne corrigent pas seules le FAIL de mort du CLI.

## Comportement du lot

`runner::broker` prépare une intention initiale avec UUID imposé et conserve les vrais FD du binding manuel/fournisseur. Nil, approbation héritée, interactif et dictionnaire de secrets contrôle hors bornes sont refusés avant IO. Le caller de confiance doit déjà avoir authentifié le profil et réservé l'opération ; la préparation ne crée aucune identité ni autorisation.

`BrokerRunState::from_job` reconstruit purement le plan depuis intention/checkpoint courant, approbation conservée. `credentials_from_job` n'utilise que le binding durable retenu du Journal et crée le handoff minimal existant. Il ne recharge ni la source tournée, ni `providers.json`, ni un ancien record fournisseur. Les objets et diagnostics restent opaques.

Le nouveau port runtime batch garde ownership, inspections strictes, captures bornées et terminal séparé. Il consomme un garde synchrone après préparation complète de la commande et vérification d'autorité, immédiatement avant spawn. Il n'a ni durée métier reconstruite ni abonnement Ctrl-C ; son résultat public ne porte pas de booléen `timed_out`. Le garde et le canal watch sont des ports de confiance, pas un enforcer déjà implémenté ou une preuve d'autorisation.

Signal vrai/canal fermé avant émission refuse start. Après signal, l'attente de fin du client moteur est bornée10s, puis une demande d'arrêt du seul transport est émise si nécessaire et une erreur conserve l'incertitude. Ni observation terminale, libération de quota ni fence moteur n'est inventé. Pendant l'attachement et la finition, les inspections d'observation et le callback `on_started` sélectionnent le signal ; l'erreur préarmée n'attend pas un second stop synchrone. `on_created`/`on_ready`, qui rendent le contrôle préarmé, ne sont pas interrompus par ce canal ; avant spawn, la vérification d'autorité garde les bornes techniques de commande existantes. Les10s concernent le drainage du client attach déjà émis, pas une borne de retour de toute la méthode. Le chemin CLI legacy conserve ses timers/signaux/TTY.

## Reproduction et correction de revue

**RPT-01, FAIL reproduit au GEL1 :** un callback commencé puis suspendu était détruit à EOF du client ; une nouvelle invocation pouvait créer un autre RPC. L'oracle synchronise l'entrée du callback, la fin du client de fixture puis sa reprise : compteur2 au lieu de1. Le test exact, la fixture, le binaire et les quatre sources modifiées du gel sont préservés hors target.

**GEL2 :** le même futur reste épinglé après EOF, avec inspection terminale puis attente stop-aware ; aucun second appel du callback n'est reconstruit. L'oracle passe dans la cible finale. Le reviewer sépare cette correction source des preuves de test.

## Vérifications

Commandes de Luna, avec `CARGO_TARGET_DIR=/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/target` :

| Commande | Résultat constaté |
|---|---|
| Deux cibles nouvelles `--no-run` sur base inchangée | **FAIL attendu**,RC101,E0432 : API absentes ([soumission](logs/submission-api-red.log),[runtime](logs/runtime-api-red.log)) ; aucun test exécuté ni vulnérabilité déduite. |
| `cargo test --locked --test runtime_prearmed pending_started_callback_is_not_reinvoked_when_attach_reaches_eof -- --exact --nocapture` au GEL1 | **FAIL produit reproduit**, deux invocations au lieu d'une ; le parent/enfant ne comptent pas comme deux défauts. |
| `cargo test --locked --test broker_submission -- --test-threads=1` final | [**PASS 5/5**](logs/submission-final.log),0ignoré,0filtré,0,45s de test. Vrais fichiers/Journal, None/manuel/fournisseur, approbation reconstruite, sources tournées/anciennes références retirées, bytes/SHA du handoff, durée de vie FD et tamper. |
| `cargo test --locked --test runtime_prearmed -- --test-threads=1` final | [**PASS 4/4**](logs/runtime-final.log),0ignoré,0filtré,13,94s de test après correction Clippy du seul harnais. EOF, callback bloqué/signal au seul child propriétaire, refus/dernier garde, ObserveOnly, reprise possédée et résultat/captures. |
| `cargo test --locked --test credential_handoff shared_snapshot_survives_handoff_and_native_cli_reads_its_exact_prepared_bytes -- --exact` | [**PASS 1/1**](logs/handoff-canary.log),6filtrés ;0,38s. Les six filtrés ne sont pas des PASS. |
| `cargo test --locked --test runtime_managed -- --test-threads=1` | [**PASS 14/14**](logs/runtime-managed-canary.log),25,90s. Pas de second décompte des fixtures/enfants. |
| `cargo fmt --all -- --check` / `cargo clippy --locked --all-targets -- -D warnings` | [fmt](logs/fmt.log) et [Clippy final](logs/clippy-final.log) **PASS**. Premier Clippy RC101 sur un if imbriqué du test seulement, corrigé sans allowance ; cible4/4 rejouée sur fichier final. |
| `cargo build --release --locked` | [**PASS**](logs/release-build.log),38,14s,RC0. |

macOS ARM64, programmes CLI de substitution écrits en Rust et explicitement approuvés dans le harnais ; aucune fixture n'est présentée comme Docker/Podman. Aucun fournisseur, modèle, navigateur ou moteur réel exécuté pour ce lot. Les deux petites fonctions de types/Debug restent des contrats logiciels, pas des tests d'isolation supplémentaires.

Les échecs du [harnais](harness/submission-initial.log) sont conservés distinctement : [borrow déplacé](harness/runtime-compile-initial.log) à compilation, alias macOS `/var` refusés par le vrai contrôle de liens, [compte global de starts](harness/runtime-initial.log) au lieu du delta du scénario, puis [réouverture config non canonique](harness/submission-references.log). L'échec préalable `DirectoryNotEmpty` sur références fournisseur n'a pas de fichier brut conservé : constat transcrit et limite explicités dans le rapport Luna, sans log reconstitué. Seules les fixtures/oracles ont changé pour ces cas ; aucune protection produit n'a été relâchée. Un [build au mauvais chemin de cache](harness/cache-build-interrupted.log) a été interrompu par SIGINT ciblé,RC130,aucun test exécuté ; ses artefacts sont conservés et ne comptent pas PASS. Un [passage intermédiaire](harness/runtime-unused-assertion.log) porte encore un avertissement d'oracle inutilisé ; il est distinct du gel final sans avertissement.

## Refaire le RED de revue

Dans une **copie de travail isolée du commit de ce lot**, inverser le [seul correctif runtime](rpt01-gel1-to-gel2.patch), puis remettre le [test exact GEL1](rpt01-red-test.rs.txt) en `tests/runtime_prearmed.rs` et sa [fixture](rpt01-cli-fixture.rs.txt) en `tests/runtime_fixtures/managed.rs`. Exécuter la commande exacte RPT-01 ci-dessus attend RC101 avec compteur2/1. `git apply --reverse --check` du patch est vérifié sur les sources finales ; aucune inversion n'a été appliquée au checkout de développement. Source du test RED SHA256 `372f0a58bf0d677e59931c0d9b6b4170383ada4e06bb9af50e43e48fd7961762`. Le [log RED](logs/rpt01-red.log) et les archives privées conservent l'observation originale ; aucun second run RED n'a été lancé pour la documentation.

## Gels et artefacts

[Manifeste des118entrées Rust/Cargo/tests](SOURCE_SHA256SUMS.txt), SHA256 `84bcbb15d0e440975ee345c49c3530519421fb5c53abac865f3238139e897516`. Vérifications [checkout](logs/source-verify.log) et [archive extraite](logs/archive-verify.log)118/118 conformes,RC0. Archive privée complète `/tmp/ai-guard-broker-submission-qualified.oCJSTs/source.tar`, SHA256 `1e0b5b9944274fd0fb83d33b5cb0945811f1c1d887081c7aab5920ba4ccce7eb`. Le tar BSD conserve aussi118entrées AppleDouble `._*` de métadonnées macOS ;118désigne les sources logiques du manifeste, pas le nombre total d'entrées tar. Dossier700,archive/manifeste/artefacts600 ; aucun binaire ni binding de credential ajouté à Git.

| Artefact privé hors target | SHA256 |
|---|---|
| `artifacts/agentsandbox-macos` | `c80bf668f142ca0184ef947a77d7c3344ebef7e3cefd33e510d9dd9558129278` |
| `artifacts/libagentsandbox.rlib` | `5e44d0295282394bee2f5183c6f982680f858d8c83d8c82e37d0c2bdb2bcc3a4` |
| `artifacts/broker_submission` | `1bddf1c997eb8353e42825dab12354c5962c0a0e416b90c642934192ada5f951` |
| `artifacts/runtime_prearmed` | `3ab7052fb33b7970f1676d7484024dd7949be7ff94d1d7ed88ca6ddb03832ea2` |

Sources finales de test : soumission `1b5b7b31d4bb37bc0de5c9ae272b7cd076d7b7cfda546d07a6c76c2a4579954f`,runtime `ac8901fcaac3d9944daaa40bebc41d86825dd1f12332b64584821c049c84a24f` ; les SHA antérieurs ne sont pas attribués au gel final. GEL1 RPT-01 conservé dans `/tmp/ai-guard-submission-prearmed-gel1.57LMSX` et `/tmp/ai-guard-prearmed-gel1-callback-qa`, dont binaire du test antérieur hors target. Les empreintes décrivent ces artefacts observés, sans promettre un build bit-à-bit reproductible ni une image qualifiée.

Le [manifeste des pièces de preuve](SHA256SUMS.txt) couvre les textes de ce dossier, hors lui-même. Les scans prépublication portent sur le delta Git explicite ; l'absence de clés connues ou de motifs détectés ne constitue pas une DLP générale.

## Limites et suite

Enforcer concret/horloge consommée, IPC authentifié, cycle de vie local durable, service permanent, helpers v3 et gardien restent à raccorder. Un watch et un garde ne ferment pas D5, les courses de start retardé ou la panne du broker. Docker/Podman réels, autres OS, crash/reprise du service, recette A1/A2/B1 et charge100 : **NOT_RUN** pour ce lot. G09 reste **FAIL**, aucun des12gates fermé ; défaut schéma2, QA et `billable=false` inchangés.

Le mainteneur demande un rappel Daybreak Blue après stabilisation du service et des tests réels d'arrêt/reprise. Cette revue de patch ne déclenche pas l'audit global.
