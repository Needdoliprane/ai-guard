# Broker permanent — essais Docker et diagnostics, 31 octobre 2026

**G09 FAIL ; aucun gate global fermé.** Deux vrais essais échouent avant Running, donc avant injection de panne. Le nouveau diagnostic passe ses vérifications logicielles ; son émission réelle reste **NOT_RUN**. Aucun fournisseur IA, SSO, frontend, cloud ou PKI utilisé par ce lot. Les revues indépendantes ne trouvent pas de P0/P1 dans leur périmètre ; elles ne qualifient pas le produit.

## Périmètre et gels

Base `e7cc0bb25dd1b8ab81e63bb5ed44cf8c55661efa`, branche `codex/broker-service`, worktree isolé. Le checkout principal et ses modifications du mainteneur restent préservés. Les manifestes figent Cargo, toolchain, sources et tests, hors secrets et données QA. H4/H5 ont155 entrées ; D1/D2/D3 en ont158. Les archives privées et les binaires ne sont pas ajoutés à Git. Les chemins temporaires désignent les preuves de cet hôte, pas des artefacts garantis après sa suppression.

Hôte Darwin ARM64, Rust1.99.1 ; Docker CLI29.2.1 et daemon Desktop29.9.2 Linux/aarch64, endpoint Unix explicite. Daemon sans rootless annoncé, opt-in rootful du harnais explicite ; worker non privilégié. Image locale `sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6`, sans pull ni qualification RepoDigest. Les services de contrôle de ce harnais sont des binaires natifs ; le déploiement conteneurisé du contrôle n'est pas qualifié par ces essais.

## Résultats observés

| Gel / commande | Statut | Portée constatée |
|---|---|---|
| H4 : test réel `broker_sigkill_preserves_original_deadline_without_second_start` | FAIL, RC101,47,39s | Aucun Running+Ready/prepared observé ; aucune faute SIGKILL injectée |
| H5 probe natif `inspect_retained_guard_after_runtime_failure` | PASS, RC0,0,02s | Lecture du seul gardien H4 existant : stopping, prepared_once, CID created, stop_failures0 ; aucune mutation/start/cancel |
| H5 retry : même test réel, observation HTTP légère | FAIL, RC101,46,59s | Même échec avant Running ; le retrait du polling writer ne suffit pas |
| D1 : `--lib diagnostics::tests` | PASS,5 | Métadonnées bornées, propagation typée et absence du texte privé dans le payload |
| D1 : `--lib runtime::prestart::tests` et `--lib broker::enforcer::client::stage_diagnostic_tests` | PASS,1+1 | Contextes runtime et prédicat final natif |
| D1 : fixture `runtime_prearmed` ciblée | PASS,1,15,79s | CLI runtime Rust de test, pas Docker ; contexte préserve l'erreur typée et l'arrêt pendant callback |
| D1 : fmt / Clippy strict | PASS / FAIL,RC101 | Dead code, taille d'erreur et placement du module de tests ; logs conservés |
| D2 : mêmes7 unités et fmt | PASS | Box du sample d'erreur, getters test-only, module déplacé ; application identique à D3 |
| D2 : Clippy strict | FAIL,RC101 | Un `collapsible_if` dans le harnais, corrigé par let-chain équivalente |
| D3 : fmt / Clippy strict all-targets / release | PASS,3RC0 | Aucun nouveau test rejoué après la seule correction syntaxique du harnais |
| Nouveau `broker.batch.failure` sur vrai moteur | NOT_RUN | Le raccord outbox est relu en source, pas encore exercé réellement |
| SIGKILL/deadline/reprise, disarm D5, recette A1/A2/B1, Podman/Linux, capacité100, TTY | NOT_RUN dans ce lot | Aucun résultat logiciel ne les remplace |

Le total diagnostic est **huit fonctions logicielles uniques**, sans additionner les sept replays D2, le probe natif ou les tests historiques STOP. Lire [la revue finale](d3/reviewer-prestart-diagnostic.md), les logs D1/D2/D3 et leurs deltas. Les échecs Clippy intermédiaires sont conservés.

## Faits des deux échecs réels

Chaque demande utilise un gateway schéma3 et un stockage neufs, une source jetable, une politique autorisant uniquement `/bin/sleep 20`, un timeout5s, réseau none et quota1. Les15 opérations de supervision sont ACKées, dont import/vérification/helpers ; Arm worker ACKrev33, claim emit_once ACKrev35, puis worker/unavailable rev36 et37. Le worker central reste StartIssued, le conteneur Created, quota retenu, sans durée runtime ou usage terminal inventé. Aucun pending HTTP n'explique à lui seul l'échec.

| Élément | H4 | H5 retry |
|---|---|---|
| Racine privée conservée | `/private/tmp/br-dFE99I` | `/private/tmp/br-NBcV3F` |
| Worker | `b7ab7ad0-a9cb-4271-a4a4-5927ce14013a` | `c252e676-d039-4c02-bcf4-18f6fdd92c36` |
| CID | `a87643c3d8bcffff916f79807ecf7d09469c1b399ec8687951d6ff3b22d82ecb` | `e22601834084d4cee79a4ffb0bc700656a4b420e2c7f89bc25c2eab64bd60db9` |
| Arm | `771ffb47-1928-4350-9412-28d72549f2cd` | `1d078937-ad5a-4c3f-9231-a2367c916740` |
| Nanosecondes natives Arm / deadline | `339650512620458 / 339655512620458` | `340604839052166 / 340609839052166` |
| Sample du contrôle du claim | `339653528379708` | `340607474064125` |
| Temps restant à ce sample, avant dernières écritures | `1,984240750s` | `2,364988041s` |

Les samples proviennent de l'horloge continue Darwin ; `observed_utc=null`. Les mtimes du journal ne donnent pas une correspondance UTC exacte. Une expiration de durée ou de fraîcheur750ms est une hypothèse, **pas la cause établie**. Le prepared_once ultérieur du probe peut être établi par la restauration observe-only : il ne prouve pas le succès initial de Prepare. Aucun arrêt effectif du worker n'est revendiqué sur un CID jamais observé démarré.

Chaque essai ajoute un conteneur Created et son volume ; zéro conteneur/volume antérieur manquant dans les inventaires avant/après. Les gardiens non vides et leurs ressources restent conservés faute de disarm D5 qualifié. Aucun prune, kill global, effacement de claim, suppression de volume ou modification de la configuration existante. Les credentials frais et l'auth privée du gardien ne sont pas publiés.

## Diagnostic implémenté et limites

Les erreurs natives Prepare/garde conservent code fermé, Arm, deadline d'origine et sample existant éventuel. Les contextes runtime indiquent la frontière en échec et, seulement si valide, le CID complet. Le classifier sérialise uniquement ces types connus, sans formater la chaîne d'erreurs arbitraire. L'événement passe par masquage puis enqueue V4 durable avant HTTP/ACK, avec les limites existantes en cas d'échec du journal. Un message générique signale une livraison incomplète sans annoncer à tort sa durabilité ou son absence.

`runtime_effect=unknown` ne donne aucune autorité, preuve d'absence, terminal, fencing ou libération de quota. Wire IPC, LocalFailureV1, Status, formats persistés et dépendances inchangés. Le dernier garde reste immédiatement avant spawn ; aucune IO/attente/lecture d'horloge ajoutée au chemin de succès. Aucun réarmement, nouveau claim, rallonge de durée ou affaiblissement des synchronisations.

## Commandes exécutées

Toutes les commandes Cargo sont exécutées par Luna depuis le worktree, avec `CARGO_TARGET_DIR=../broker-supervision/target`. Les filtres unitaires restent ciblés :

```sh
cargo test --locked --lib diagnostics::tests -- --nocapture
cargo test --locked --lib runtime::prestart::tests -- --nocapture
cargo test --locked --lib broker::enforcer::client::stage_diagnostic_tests -- --nocapture
cargo test --locked --test runtime_prearmed prearmed_stop_interrupts_pending_started_callback_without_global_ctrl_c -- --exact --nocapture
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

Les deux essais réels utilisent la sélection suivante, **propre à cet hôte**, et conservent des ressources ; ce n'est pas une commande de quick start ordinaire :

```sh
AGENTSANDBOX_TEST_RUNTIME=docker \
AGENTSANDBOX_TEST_RUNTIME_PROGRAM=/opt/homebrew/bin/docker \
AGENTSANDBOX_TEST_RUNTIME_ENDPOINT=unix:///Users/louis/.docker/run/docker.sock \
AGENTSANDBOX_TEST_IMAGE=sha256:7c64760cc4a5f29a40685cae2451afed914ce81671601166792cd087beab9db6 \
AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1 \
AGENTSANDBOX_TEST_RETAIN_ENFORCER=1 \
cargo test --release --locked --test broker_runtime broker_sigkill_preserves_original_deadline_without_second_start -- --ignored --exact --nocapture
```

Le probe n'appelle ni sélection Docker ni serveur HTTP et refuse de créer un gardien de remplacement. Il lit la capability privée existante sans l'afficher :

```sh
AGENTSANDBOX_TEST_RETAINED_ROOT=/private/tmp/br-dFE99I \
AGENTSANDBOX_TEST_RETAINED_ARM=771ffb47-1928-4350-9412-28d72549f2cd \
cargo test --release --locked --test broker_runtime inspect_retained_guard_after_runtime_failure -- --ignored --exact --nocapture
```

## Artefacts et suite

Les captures privées originales sont `/private/tmp/ai-guard-runtime-h4.1Y3B4Z`, `/private/tmp/ai-guard-runtime-h5.4pwJ7e`, `/private/tmp/ai-guard-runtime-h5-retry.fxob46`, `/private/tmp/ai-guard-d1-software.3ipHYm`, `/private/tmp/ai-guard-d2-software.DkH0QN` et `/private/tmp/ai-guard-d3-final.UPraQc`. Root vérifie indépendamment158 fichiers de l'archive finale extraite et158 fichiers du checkout contre le manifeste D3, plus les deux copies de binaires. SHA agentsandbox D3 : `709b70eee959fb23bb309164ae5e7f8e32a5dcc6dfd96fd91673aa8fef8221ae` ; provider : `2cb846f02db229a673c95b3c35d287db6f0d0d5f16029deb3d580df664483390`.

Prochain critère : une seule tentative réelle instrumentée sur D3 pour livrer/lire le diagnostic codé, localiser le refus et corriger avec régression avant répétition. Puis établir Running, panne SIGKILL, arrêt sur deadline d'origine, reprise idempotente et cycle terminal/disarm. SSO/Keycloak/PKCE reste obligatoire mais absent, qualification NOT_RUN ; [contrat et documentation](../../../SSO_OIDC.md). Le rappel Daybreak Blue attend ces parcours réels stabilisés.
