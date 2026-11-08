# SDK HTTP d'admission du relais — 8 novembre 2026

**PASS logiciel ciblé : trois nouveaux parents SDK et trois canaris existants
sur le gel 75, exécutés dans QA AO.** Le vrai `AuditClient` parle au vrai
gateway Axum et à deux bases SQLite privées. Les seuls effets injectés sont
une perte du corps de réponse après commit et une contention d'écriture
SQLite. Les faits runtime restent synthétiques ; aucune factory de production
n'est ouverte par ce lot de tests.

## Source et contrat

Base `1943057ffc4caa335776553149831b71b8b07a91`, source testée
`bdcf21993ca0ce86a12b9c0534ebdec70a73888e`, snapshot privé
`relay-admission-sdk75`. Le [manifeste sélectionné](source.sha256) couvre les
cinq fichiers du lot et `Cargo.toml`/`Cargo.lock`. Les sept ancres ont été
comparées au manifeste complet du gel 75,
`relay-admission-sdk75-source.sha256`, SHA-256
`05fb88d072e57d0ddb4c2645bf885863e6efb72b544de18b295b1abf2f04b1df`.
Code et tests restent inchangés pendant cette finalisation documentaire.

Le seul ajout au module SDK existant est sa déclaration `cfg(test)`.
Seize helpers stricts ont été partagés dans
`tests/support/relay_admission_fixture.rs`, sans modifier leurs comportements.
La revue indépendante a vérifié leurs corps et les oracles conservés :
18 tests historiques dans `worker_relay_api.rs` et trois dans
`relay_admission_cases.rs`. Ces nombres décrivent la source, pas des PASS de
cette campagne. Les 18 historiques n'ont pas été exécutés ici.

Cette preuve complète le
[lot Store/API précédent](../../2026-11-07/relay-admission/README.md) pour le
SDK HTTP du [contrat d'admission](../../../RELAY_ADMISSION.md). Elle ne
transfère aucun résultat au raccord runtime ou à une autre composition.

## Commandes et résultats

Commandes ciblées exécutées par Luna, sans relance lors de la rédaction.
Les argv et le contexte viennent de son registre, pas des lignes de sortie
qui ne les réimpriment pas. Le binaire est celui du toolchain Linux
`/usr/local/rustup/toolchains/1.99.1-aarch64-unknown-linux-gnu/bin/cargo` :

```sh
cargo test --lib --locked --offline runner::relay_admission::tests:: -- --test-threads=1 --nocapture
cargo test --test worker_relay_api --locked --offline relay_admission_cases -- --test-threads=1 --nocapture
```

Répertoire courant `/scratch/sdk75-ao`, `CARGO_HOME=/scratch/cargo-fixed`,
cible `/scratch/target-sdk75-ao`.

| Sélection du gel 75 | Résultat observé |
|---|---|
| `runner::relay_admission::tests::` | 3 PASS, 0 FAIL, 0 ignoré ; 629 filtrés, non comptés PASS ; RC 0 ; 6,55 s de test. |
| `relay_admission_cases` | 3 PASS, 0 FAIL, 0 ignoré ; 40 filtrés, non comptés PASS ; RC 0 ; 0,95 s de test. |

Il s'agit de **six fonctions uniques**, dont trois nouvelles. La première
sortie imprime un temps Cargo de 1 min 56 et un `real` hôte de 1471,84 s.
Ces observations sont distinctes du temps de test de 6,55 s ; l'origine de
leur écart n'est pas établie dans cette preuve. La seconde sortie imprime
53,57 s pour Cargo et 54,67 s pour le wrapper hôte. Aucune performance produit
n'est déduite de ces durées et aucune répétition n'a été lancée pour les réduire.

Les avertissements existants restent présents : deux bibliothèque-test dans
la première sélection, 61 bibliothèque dans la seconde. Aucun `check
--all-targets`, Clippy strict, build sans avertissement ou suite complète
n'est crédité à ce gel.

## Comportements exercés

| Nouveau parent SDK | Oracle réel |
|---|---|
| `sdk_http_fresh_claim_current_scope_and_revocation` | Réservation puis `Fresh` unique ; reçu corrélé au hash du claim ; relecture courante et transition Ready ; replay sans nouveau fait ; identité B refusée avant le POST claim de A ; révocation refusant la revalidation sans nouvel événement. |
| `sdk_http_lost_claim_response_replays_without_fresh_capability` | Le vrai gateway commet, puis le middleware interrompt uniquement son corps de réponse. Une connexion SQLite indépendante constate un seul claim ; le même UUID rend ensuite `Replayed` avec la même séquence et le même hash d'audit, jamais `Fresh`. Un nouvel UUID ne contourne pas le claim consommé. |
| `sdk_http_sqlite_write_unavailability_cannot_mint_claim` | Après préflight d'identité positif, un writer SQLite indépendant tient `BEGIN IMMEDIATE` pendant le prochain POST claim. La réponse est 503, aucun claim ni nouvel événement n'est conservé, la réservation reste Reserved et le worker conserve Running/quota. Après libération, le même claim peut devenir frais une seule fois. |

Les trois canaris existants réexercent réservation/replay/révocation,
annulation non claimée, claim unique et état inconnu, clôture sur terminal
exact, reopen inactif, rôles/tenants avant corps et namespace réservé. Ils
vérifient la non-régression du déplacement des fixtures, sans ajouter leurs
répétitions au nombre de fonctions uniques.

Les tests n'inspectent ni ne journalisent Authorization. Ils vérifient
l'absence de leurs credentials publics synthétiques dans l'audit persistant
des deux tenants et dans les diagnostics SDK fermés. Aucun credential réel
ni configuration hôte n'est lu.

## Journaux et environnement

Pièces privées sous le registre parent
`ai-guard-cold-drain-logs-20261106.nXx9XJ`, lues et rehashées :

| Pièce | SHA-256 |
|---|---|
| `sdk75-unit-ao.log` | `886ad75cc4a6c4bb202b1e291d176c4fc4838574d68c40f24f7bee43b7cbf7ff` |
| `sdk75-worker-am.log`, exécuté dans AO | `77cf04105393cc8f607e97a77e5dbb6e9a7c5fa7757261c336f9326a5da3b00e` |
| `qa-ao-inspect.json`, initiale | `af1bc704a7737b5c4af1ccc61d285f83892ca0d6ff0af0502eef4baef1dfb43f` |
| `qa-ao-preflight.log` | `7ae49c7830ae7c20f704854ce72f36c5e247173e46a798174e4c86d00abe3578` |
| `qa-ao-final-inspect.json`, terminale | `3ffe1f1e55358ea011cbe77c7941daf4bd8b9cafac55c4fa9d93b000a59c7f16` |

Le parent conserve aussi les pièces hors `/tmp` dans le checkpoint privé
`relay-sdk-audit-checkpoint-20261108-0000`, dont les 18 entrées ont été
vérifiées par lui. Ce nom de conservation n'est pas un lien public stable.

Le suffixe `am` du nom du second journal est historique : ces deux sélections
ont été exécutées dans **AO**, pas dans AM.

QA AO, Linux ARM64, image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
Rust 1.99.1, UID 1000:1000, capacités supprimées, NNP/seccomp, réseau coupé
avec loopback privé, racine/outils/registry/sources en lecture seule. Scratch
de 4 Gio et 65536 inodes, quatre CPU, 8 Gio sans swap, 256 processus, fichier
de 512 Mio, nofile 1024 et core 0. Ces valeurs bornent le harnais ; elles ne
mesurent pas la consommation du produit.

Le cache privé est une copie du registry en lecture seule, complétée avant
les essais avec `linux-raw-sys 0.12.1`. Le parent a fourni l'archive publique
`public-linux-raw-sys-0.12.1.crate`, 3006116 octets, depuis `static.crates.io`,
en une acquisition distincte du harnais. Luna a vérifié son SHA-256
`32a66949e030da00e8c7d4434b251670a91556f4144941d37452769c25d58a53`,
égal au checksum de `Cargo.lock`, puis copié l'archive RO `/inputs` dans le
cache scratch privé. La vérification offline complète a ensuite réussi.
Les commandes Cargo ci-dessus restent hors ligne, sans réseau QA, et
utilisent une cible privée au gel. Ce prérequis de cache n'est ni un appel
fournisseur ni une API modèle.

Après les résultats RC 0, le parent a constaté qu'il ne restait que le sleep
du harnais puis a fermé AO. L'inspection terminale confirme
`FinishedAt:2026-11-07T23:55:54.580513798Z`, RC 137 et `OOMKilled:false`.
La cause opérateur est le constat du parent, distinct des champs inspectés.
Cet arrêt du harnais n'est ni un test supplémentaire ni un terminal worker
produit. Les journaux et manifestes restent séparés du scratch de test.

## Limites

La panne exercée est une **contention SQLite réelle avant commit**, qui peut
refuser le parcours dès son contrôle transactionnel d'identité. Elle ne
prouve pas un EIO, ENOSPC, une panne électrique, une perte après commit du
stockage ni une disponibilité sous charge. Les états inconnus ne sont pas
résolus par la seule perte du corps HTTP.

L'ancien lecteur, l'intégration du permis avec dernière observation runtime,
échéance monotone et RoleLease, la preuve de jonction/RootRetirement,
les factories runtime/propriétaire et la recette réelle restent hors de ce
gel. Aucun moteur produit, fournisseur, modèle, frontend, campagne E2E
générale ou infrastructure distante n'est invoqué. La revue indépendante
acceptée et ces six PASS ciblés ne qualifient pas une release et ne ferment
aucun gate de production.
