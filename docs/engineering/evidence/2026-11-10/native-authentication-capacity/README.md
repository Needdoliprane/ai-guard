# Capacité OIDC native et audit terminal — preuves du 10 novembre

Le candidat composé R6 passe60 fonctions ciblées et le check de toutes les
cibles (RC0). Une reproduction supplémentaire R7 démontre ensuite une rétention
de propriétaire sans sortie dans une fixture de reprise. Le correctif gateway
FIXr3 est relu ;24 parents exacts et un nouveau check toutes cibles passent
sur son propre gel. Sources et guides EN/FR sont intégrés sélectivement en
préservant les autres travaux du checkout.
Aucune clôture du constat D5 complet ni qualification production n'en découle.

## Sources distinctes

Base publique `d5c46b23fc2326d730d21a6f3d8285ec326646aa`. Composition R6 :
Core FIXr3, opérateur RootR3 et gateway FIXr2, seize fichiers propriétaires,
2341 fichiers réguliers. Manifeste complet :
`7f06ab1317fb43c745e96dd07cee5f1af8ce9ee91ca445520c91e5f12638d081`.
Le manifeste réel recalculé après exécution est identique ; export réel :
`ff5a198c7d9ca2086f270975b3b6d71cbbb0c6b729d00e28cf763c72bb0ecd58`.
Les autres WIP du checkout d'intégration ne sont pas couverts par ce gel.

R7 ajoute seulement un enfant de tests et son raccord : même produit,2342
fichiers, manifeste
`5a5a4572bc2557ba752e15eb29f1375b2af3b31450184469e7e9933bfc6565a5`.
Son post-manifeste est identique ; export réel :
`b536ed733413cf9fc26d3d7965e4840098ccc78ff16e870fc2e23e6c17605200`.
Les gels, patches et rapports indépendants restent intacts hors worktrees.

## Reproductions avant correction et résultats R6

Deux parents du ledger atteignent leurs RED sur les ports actuels : aucun
reçu distinct de résultat Logout après FRefresh, et place du terminal Logout
consommée par une autre écriture. Le CLI reproduit ensuite le refus de
`--target-version 3` après une vraie migration synthétique V1→V2. Deux autres
parents démontrent la garde périmée du consommateur : elle refuse un audit
neuf réellement V3 et accepte encore la fixture commune marquée V2.

Le candidat introduit BEGIN/FRefresh/END typés, réserves records/octets,
rejeux stricts et format V3 avec migration explicite V2→V3/backup neuf.
V1→V2 reste le défaut CLI. Le gateway sépare les capacités anonymes et
vérifiées et conserve les propriétaires jusqu'aux reçus durables requis.
Les guides EN/FR et la garde normale V3 sont préparés dans le lot.

| Sélection réellement exécutée sur R6 | Fonctions PASS |
|---|---:|
| `audit::authentication::native_logout_tests::` |14|
| Dernier END, contrôle privé de réserve |1|
|19 noms exacts OIDC capacité/contrôles/échecs/Logout |19|
| Consommateurs de configuration OIDC V3/V2 |2|
| `authentication_migration_cli` |6|
| `authentication_terminal_reservation` |5|
| `oidc_authentication_audit`, filtre `ledger_` |13|
| **Total unique** |**60**|

Commandes, avec cache privé verrouillé hors réseau :

```text
cargo test --frozen --offline --lib --no-run
cargo test --frozen --offline --lib FILTRE -- --exact --nocapture --test-threads=1
cargo test --frozen --offline --bin agentsandbox --no-run
cargo test --frozen --offline --bin agentsandbox FILTRE -- --exact --nocapture --test-threads=1
cargo test --frozen --offline --test authentication_migration_cli -- --nocapture --test-threads=1
cargo test --frozen --offline --test authentication_terminal_reservation -- --nocapture --test-threads=1
cargo test --frozen --offline --test oidc_authentication_audit ledger_ -- --nocapture --test-threads=1
cargo check --frozen --offline --all-targets
```

Le groupe Core14 emploie son filtre de module sans `--exact` ; les19 OIDC
emploient chacun leur nom compilé complet, vérifié par `--list`. Root a relu
les résultats :26 invocations valides,60 noms uniques, aucun doublon. Deux
filtres erronés à zéro sélection sont exclus ; les erreurs de wrapper avant
Cargo ne comptent pas comme tests. Le check toutes cibles termine RC0 à
05:14:45UTC, raw stderr
`3243f56bd0d0836cfb5d94ea35f6cea51d50dcdb92d8101cdc7094f31a433814`.
Avertissements conservés ; Clippy n'est pas réputé exécuté.

## Nouveau défaut de reprise R7 — FAIL discriminant

Parent exact :
`api::oidc::gateway::admission_tests::capacity_tests::controls::faults::native_logout::qualification_followup::durable_refresh_before_first_logout_must_reconcile_without_a_retroactive_logout_begin`.

Build lib RC0 ; énumération du nom exact une fois, puis sélection1/RC101 à
05:22:12–05:22:13UTC. Un vrai FRefresh SQLite est commis avant toute ILogout ;
son ACK est délibérément omis du keeper. Son rejeu exact rend le même reçu.
Logout, reap à l'échéance originale et le vrai Drop laissent cependant un
propriétaire, contre zéro attendu ; Logout/reap répondent503. Aucun nouveau
fait d'audit n'est ajouté. L'assertion sur la réponse finale, située après
ce défaut, n'est pas atteinte et n'est pas déclarée vérifiée.

- stdout : `c2f030785be4051a1754778d804ed8b654c1015bf9d1f7eef37b6650d795743b`.
- stderr : `b095109edbe5c3ae8e27e2a2b0eabc49684e630cfe12fbd73816caeae8e7c696`.

Un second regard vérifie ces preuves par lecture/empreintes. L'omission ACK
est synthétique à une vraie frontière SQLite : aucun incident OS ou moteur
réel n'est reproduit. Le refus Core d'inventer un BEGIN rétroactif est correct ;
la sortie du propriétaire initial est à corriger côté gateway. Un retrait
fondé seulement sur CurrentClosed ne suffit pas : le correctif doit obtenir
le reçu FRefresh exact avant retrait, préserver une promesse Logout existante
et refuser une preuve divergente. Le nouveau candidat gateway FIXr3 est livré
pour revue/QA ; il n'est pas déclaré testé par les résultats R6.

## Correctif FIXr3 et nouvelle validation — PASS

Base publique `b74e8986803898c2d9f7e358a68156244c345907`, composée avec les
seuls17 chemins acceptés Core FIXr3/opérateur RootR3/gateway FIXr3. Les autres
WIP du checkout ne sont pas inclus. Source2343 fichiers :
`c629f4bbf72c4cf1ad7079aa81476fd797c517640ecbc9901b789c28f603ad05` ;
manifeste des17 chemins :
`ce428d3551a01ae3f768f2df224a02b5fcefe28b95ac8ed5eb0b98b7d9d35d6c`.
Le manifeste final recalculé est identique,2343 fichiers vérifiés RC0.
Export réel après exécution :
`8c1f1c419fbf6a244e18413b608bb4a66caf5fd62cfab92235bb6039dc2b70da`.

Avant de créer une nouvelle ILogout, le gateway distingue le Refresh déjà
fermé durablement avec sa liaison exacte et son choix initial conservé.
Il doit obtenir le vrai ACK du rejeu FRefresh avant de retirer le propriétaire.
Il n'invente ni BEGIN rétroactif, UUID, résultat Logout ou preuve de succès.
La réponse reste `unknown/not_attempted/unavailable` pour cette nouvelle demande.
Si un BEGIN peut déjà exister, les trois ACK BEGIN/FRefresh/END restent requis.
Un état Closed consulté, une preuve divergente ou un audit indisponible ne
suffisent pas au retrait. Le délai Finish initial15s est conservé ; il n'est
pas présenté comme un service autonome de drainage.

Validation Linux ARM64/Rust1.99.1 dans une nouvelle QA isolée :

| Opération | Résultat constaté |
|---|---|
| `cargo test --frozen --offline --lib --no-run` |RC0,05:57:14–06:02:12UTC|
| Liste compilée des noms et comparaison au plan |RC0,24 noms exacts|
|24 `cargo test --frozen --offline --lib NOM -- --exact --nocapture --test-threads=1` |Chacun sélectionne1 et PASS ; dernier terminé06:05:50UTC|
| `cargo check --frozen --offline --all-targets` |RC0,06:06:07–06:10:25UTC|
| Vérification finale du manifeste source |RC0,2343 fichiers|

La première fonction est la régression R7 exacte ci-dessus ; elle vérifie
maintenant aussi le retrait après vrai Drop et la voie Router avec un autre
propriétaire préservé. Le deuxième parent regroupe quatre cas négatifs : preuves
causale, de choix ou de liaison divergentes, puis writer SQLite indisponible.
Les22 suivants couvrent budgets anonymes/vérifiés, faux bearer, fenêtres,
propriétaires, deadline, audit refusé, reprises Logout et Drop réel. Les19
fonctions déjà vertes sur R6 sont rejouées ici mais ne sont pas des fonctions
nouvelles à additionner. Core, consommateurs et CLI inchangés gardent leurs
preuves R6 ; ils ne sont pas relancés par réflexe sur ce gel.

Empreintes des sorties brutes :

- compilation lib stderr : `9e764dd1115d7d3bb1197d909007dd3ac1e051709cba8b0b882906ab26280cfc` ;
- liste stdout : `c36569e51f14ab7ac467881a43a0fdb2c960a4daab24b6390728c1132d4aed60` ;
- check toutes cibles stderr : `04b27915233300339b0820674326d1d254584ed5b3d2d2e6543757aae40ba1fe` ;
- vérification finale stdout : `e449983459644abf47791d1ce4326a765f4b74ea1d0a68e6b8f82dcd1b0e1361`.

Les revues source indépendantes Core/opérateur/gateway et le complément
documentaire bilingue sont conservés avec les manifestes hors worktrees.
Le delta final des guides, documentaire uniquement, explique les budgets et
les deux chemins de fermeture ; aucun nouveau test applicatif n'en découle.
Les17 blobs indexés sont comparés aux sources acceptées avant commit ; main
est intégré par ses seuls hunks Auth, sans embarquer ses WIP broker/Kubernetes.
La qualification d'un ancien lecteur réel reste une vérification distincte.

## Environnement, conservation et limites

Rust/Cargo1.99.1, Linux ARM64 dans une QA Docker isolée, UID1000, rootfsRO,
capabilities0, NNP/seccomp, réseau none, CPU6/RAM9Gio/swap0/PIDs128,
scratch3Gio/65536inodes. Aucun socket moteur, home, secret réel ou preuve
privée monté. Sources publiques et caches épinglés seuls en lecture seule.
Les fichiers bruts sont persistés directement sur l'hôte dès chaque commande.

**Ancienne QA R6/R7, arrêtée avant la nouvelle validation FIXr3 :** une
première compilation sous4Gio subit un OOM avant les tests ; la reprise
à9Gio réussit. Une erreur de syntaxe de test est aussi conservée et corrigée
avant R6. Ces échecs de préparation ne sont pas des RED produit. Pic final QA
7013416960 octets, aucun nouvel OOM après le premier ; ce n'est pas une mesure
d'un worker ou d'un modèle. Le conteneur exact est arrêté après sauvegarde,
PID0 à05:26:42UTC, puis retiré normalement ; son indicateur OOM historique
reste distinct de cet arrêt explicite. Aucun volume ou objet non attribué
n'est supprimé. Sources, index et rapports survivent hors de tous les worktrees.

**Nouvelle QA FIXr3 :** même profil borné décrit ci-dessus, conteneur neuf
démarré à05:46:02UTC. Le relevé après les24 tests et le check indique un pic
mémoire5695160320 octets et zéro OOM. Ce relevé ne mesure ni un worker ni un
modèle. Le conteneur est encore actif à ce relevé ; sa conservation et son
arrêt sont suivis séparément, sans lui transférer l'arrêt de l'ancienne QA.

**NOT_RUN** : ancien lecteur V2 réel contre V3, Clippy de cette composition,
IdP réel, fournisseurs,
workers Docker/Podman/Kubernetes, crash physique et qualification cloud.
Le dernier END privé altère un compteur mémoire ; il ne prouve pas à lui seul
un préfixe corrompu persisté à cette dernière ligne. Les fixtures d'anciens
headers ne sont pas des historiques produits par un ancien binaire.

G09 FAIL, zéro gate global fermé. Le suivi D5 du run-2 et NV-03 restent
distincts ; ces résultats ne remplacent pas leurs exigences restantes.
