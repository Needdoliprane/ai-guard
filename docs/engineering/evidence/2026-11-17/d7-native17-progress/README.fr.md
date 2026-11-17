# D7 et Native17 — preuves logicielles privées datées

[English](README.md) · [Résultats et références SHA-256 complètes](TESTS.json)

Le code produit public reste `e794e45de243951f401a0ff0df741307cff01c00`.
Les résultats ci-dessous concernent des **gels privés distincts**. Ce document
publie les métadonnées des preuves, pas leur code ni une qualification du
checkout public. Aucun build ni test n’a été lancé pour cette mise à jour.

| Source | Résultat constaté | Portée |
|---|---|---|
| D7 R5 `653e2abe…` | Check toutes cibles RC0 ; build libtest RC0 ; **28 PASS / 1 FAIL / 5 NOT_RUN** | 20 premiers cas PASS, puis huit PASS et le cas 9 s’arrête avec RC processus réel −6/SIGABRT, débordement de pile avant son oracle. Aucune relance. |
| D7 R6 `8d174676…` | Nouveau build libtest RC0 ; **un seul cas 9 PASS RC0** | Frontière de tas test-only (`Box::pin`), sans changement de pile, d’horloge, de délai ou d’oracle. Les 28 PASS R5 ne sont ni rejoués ni transférés ; cinq cas restent NOT_RUN. |
| Legacy Native+Ports R1 `0dce799c…` | Check toutes cibles **FAIL RC101**, un E0599 ; build libtest/listes/tests **NOT_RUN** | Méthode de fixture `has_local_slot` absente, `src/broker/enforcer/client_legacy_tests.rs:229`. Correction `07c9679` non recompilée. L’ancien cas 9 est obsolète et non admis sur cette source. |
| Commun Native17 R5 `a3e9ca09…` | Check toutes cibles RC0 ; build libtest RC0 ; **0 PASS / 2 FAIL RC101** | Une invocation de chaque diagnostic logiciel réel, sans défaut de harnais ni relance. Les deux refus restent fermés sur l’époque inchangée de deux secondes. |

Il n’existe pas de bilan agrégé « 29 PASS » entre D7 R5/R6. Le PASS R6 appartient
à sa nouvelle source et à son binaire. La composition Legacy suivante change
le contrat de construction : son ancien cas 9 ne traverse ni la vraie
construction Fresh ni un véritable ancien pair V1. Cette conclusion source
ne réécrit ni l’abort historique R5 ni le PASS R6. Les régressions CLI de
remplacement restent privées, en revue, sans compilation ni exécution. La
preuve du check Legacy porte l’empreinte
`67ab96678b2b7ab3c71e58583e402c720dfd713373c551fe983682c2502029a0` ;
son reçu de commande figure dans [TESTS.json](TESTS.json).

## Observations Native17 et limites

Le premier build libtest R5 a été interrompu pour ressources, RC réel −15,
et reste conservé. Un build admis séparément sur la même source a ensuite
retourné RC0. Root a vérifié binaire, source et listes effectives avant les
deux cas. Les nouvelles invocations retournent toutes deux RC101 :

- Heartbeat du Broker enrôlé : 60,523 s pour le cas entier. Le postcheck de
  backup retenu expose désormais la cause typée
  `human_authority_epoch_expired`, avec les owners natifs conservés.
- Admission A17+B16/Auth5/OIDC : 54,590 s pour le cas entier. Le writer refuse
  à `TransactionMutations`, durée écoulée 1 999 ms, avec l’erreur d’époque
  originale.

Ces observations ne mesurent ni CPU consommé ni attente de verrou. Les
intervalles monotones incluent ordonnancement, attentes et instrumentation.
Le mark `SourceEpochReleased` suit les drops : ces intervalles ne sont pas
une borne inférieure mathématique de tenue de l’époque. Le résultat typé du
véritable `epoch.check()` reste l’oracle. Aucune époque produit n’est étendue.
Deux remédiations privées sont en cours ; aucun des deux diagnostics n’établit
un parcours positif Native17 réussi et aucun code de remédiation n’est publié
par ce relevé.

La preuve des cas Native17 porte l’empreinte
`8a70c0410c924fe650d2521fe35972fc308b06ee0fe97670826b5b7bee943e80` ;
la preuve libtest/listes porte
`0dc396349778b5e58cd9c4d5091856b0963c3da9ba7b766ffee3b838d2b1c1df`.
Le [JSON](TESTS.json) conserve les empreintes complètes des sources, binaires,
commandes et reçus Root, les FQN exacts et RC réels, sans chemins machine
privés ni rapports bruts. Les 19 anciens cas dépendants Native17 restent NOT_RUN.

## D1 et qualification

Le [relevé D1 existant de 12 cas](../runtime-observation-and-d1-late-finalizer/README.fr.md)
couvre déjà la finalisation logicielle Store/quota par quatre vrais cas
CLI/Gateway/SQLite avec moteur Rust de fixture. La synthèse courante ne
présente plus cette injection logicielle comme manquante. Moteurs réels,
TTY physique, abandon et reprise froide restent des écarts distincts.

Clippy strict reste ouvert. Ces campagnes privées ne remplacent pas la
preuve Clippy non stricte historique du public. Aucune qualification moteur,
fournisseur, cluster, A1/A2/B1 complet, reprise froide ou 100 workers n’en
découle. Les verdicts et sévérités primaires d’audit sont conservés :
**0/12 gates production clos**, aucun pourcentage fonctionnel ni release
qualifiée. [Gates](../../../PRODUCTION_READINESS.md) · [Audit run-2](../../../AUDIT_RUN_2_20261110.md).

## Formes des commandes enregistrées

Ces formes portables décrivent les invocations constatées ; aucune n’a été
nouvellement exécutée pour ce document. Elles supposent le gel correspondant,
le toolchain épinglé et les profils QA de [TESTS.json](TESTS.json) : opt0/debug0,
assertions et overflow checks actifs, 256 unités de codegen, incremental désactivé
pour dev/test. Les commandes utilisaient aussi ces surcharges de profil.

```sh
cargo check --all-targets --locked --offline --message-format=json -j1
cargo test --lib --no-run --locked --offline --message-format=json -j1
"$AI_GUARD_LIBTEST" --exact "$AI_GUARD_FQN" --nocapture --test-threads=1
```

La limite externe était de 180 s par cas, une invocation chacun. Elle ne change
aucun délai produit. Sources et archives de preuves brutes restent intactes ;
aucun ancien résultat n’est transféré à une autre source.
