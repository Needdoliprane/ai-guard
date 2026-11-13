# Adoption des sources humaines et startup owned — validation logicielle

[English](README.md) · [Résultats expurgés](RESULTS.json) · [Procédure opérateur](../../../../setup/human-sources-maintenance/README.fr.md) · [Identité/ressources owned](../../../../setup/identity/OWNED_SOURCES.fr.md)

Le 13 novembre 2026, **107 fonctions nommées uniques passent sur la même source
figée en deux campagnes distinctes : 34 puis 73**. Aucune fonction en échec ou
ignorée, aucune omission acceptée. Ce sont des preuves logicielles ciblées, pas
un run unique de 107 tests ni une qualification production. Une revue
indépendante couvre les résultats de chaque campagne. Les verdicts libtest
nommés n'établissent pas indépendamment l'exhaustivité de chaque sous-cas interne.

## Source et décompte des résultats

La source testée est `dbee1d5f90abd884badf5cd255b69c436f38ae4a`, arbre
`f3f8c1e3ed7d3358526d1cf926d9b99f1b6f5b6b`. Son archive d'entrée contient
2771 fichiers réguliers, mesure 34061824 octets et porte le SHA-256
`7050d12ba3f8fe621b8efcf91a614c3715f410de2bf9302f07b310af89e79302`.
La base documentaire `e9c474af68c5ce9d91e963515801e108bd39f93e` a ce commit
source pour parent ; ce rapport ne transfère aucun résultat à un checkout non
identifié.

| Campagne | Groupe | Fonctions nommées PASS |
|---|---|---:|
| combined-r3 | owned-api | 21 |
| combined-r3 | owned-audit | 4 |
| combined-r3 | owned-startup | 4 |
| combined-r3 | owned-config | 4 |
| combined-r3 | owned-http-lifecycle | 1 |
| remaining-r1 | legacy-scoped | 5 |
| remaining-r1 | legacy-migration | 13 |
| remaining-r1 | legacy-context | 10 |
| remaining-r1 | legacy-context-fairness | 6 |
| remaining-r1 | legacy-http-lifecycle | 8 |
| remaining-r1 | bootstrap-cli | 7 |
| remaining-r1 | bootstrap-integration | 6 |
| remaining-r1 | bootstrap-operator | 8 |
| remaining-r1 | bootstrap-preparation | 2 |
| remaining-r1 | shared-registry | 6 |
| remaining-r1 | shared-store-head | 1 |
| remaining-r1 | shared-auth-incarnation | 1 |

Les cinq premiers groupes totalisent 34 ; les douze suivants, 73. Leurs ensembles
de noms sont disjoints. La seconde campagne résout les chaînes de modules Rust
déclarées et rapproche les douze sélections fraîches des enregistrements réels du
compilateur avant l'exécution fonctionnelle. Elle ne répète pas les 34 premières
fonctions. [RESULTS.json](RESULTS.json) liste chaque nom exécuté, les empreintes
des sorties relues, les faits de source/capture et les empreintes des quatre
reçus/revues primaires conservés. Fixtures privées, credentials, binaires et
archives brutes ne sont pas publiés ici.

Le check toutes cibles passe dans combined-r3. Remaining-r1 reprend explicitement
ce **PASS antérieur sur la source exacte**, sans répéter le check. Les deux
campagnes terminent la compilation des tests avec `--no-run` et
identifient quatre vrais artefacts Cargo : tests de bibliothèque, tests main,
intégrations bootstrap et CLI normal `agentsandbox`. L'aide `human-sources` du
CLI normal passe dans les deux campagnes. Remaining-r1 consigne zéro erreur de
compilation et 164 avertissements ; Clippy strict, suite complète et validation
release ne sont pas exécutés.

## Exécution observée et fermeture

Le logiciel s'exécute en QA Linux ARM64 isolée, avec Rust 1.99.1 et l'image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`.
Le profil observé emploie un réseau absent, UID/GID 1000:1000, racine en lecture
seule, exécution non privilégiée, capacités retirées et no-new-privileges,
quatre CPU, 9 Gio de RAM sans swap, 128 PIDs, 4 Gio de scratch et une limite de
fichier de 1 Gio. Ce conteneur de QA n'est pas un worker métier et ne qualifie
pas son adaptateur runtime.

Chaque campagne termine sept collectes indépendantes avec RC 0. Root vérifie les
2771 contenus et modes des sources après exécution contre l'entrée. Les exports
de source mesurent 34263040 octets ; leurs empreintes d'archive distinctes sont
conservées dans les résultats. Les vraies racines de fixtures ancrées sont
observées vides après teardown RAII ; un vrai producteur GNU tar capture chaque
archive vide de 10240 octets. Ce sont des observations, pas des preuves de
fixtures fabriquées ni des contenus de fixtures durables.

Les pics mémoire QA mesurés sont 9158021120 et 8938557440 octets sous la limite
de 9663676416 octets, avec compteurs OOM et OOM-kill à zéro. Les rlibs de build
observées mesurent 545228954 et 545229866 octets, au-dessus de 512 Mio. Ce sont
des mesures de build QA : elles ne mesurent ni usage ou prix d'un worker métier,
ni fichier exact ou cause du signal de l'ancien échec `a1669bf`.

Les deux fermetures renvoient RC 0 pour arrêt, suppression et liste exacte des
conteneurs ; Root confirme leur absence exacte. Le holder sort avec le code 137 après arrêt
volontaire, sans verdict OOM ou échec produit. Pour remaining-r1, le handle 25710
de l'exécution Luna initiale est perdu après reset de contexte et renvoie
`UNKNOWN_PROCESS` ; aucune exécution n'est redémarrée. Root observe le PID 34620
existant avec Cargo/rustc vivants, puis vérifie les reçus terminaux et l'absence
exacte du PID RUN/conteneur. L'agent Luna suivant observe seulement la fin.
Le constat terminal repose sur `FINAL` de confiance et chaque RC de phase, les
vrais résumés Cargo/tests et la continuité processus/conteneur ; la sortie
terminale de l'outil unified n'est **pas directement observée**.

## Échecs conservés et limites

L'ancien échec de compilation à quatre erreurs sur `3a79c52` n'atteint aucun
test. Le build no-run suivant sur `a1669bf` s'arrête avec signal 25 après un
check toutes cibles PASS ; l'hypothèse de limite de fichier à 512 Mio est
fortement étayée, non formellement prouvée, et ses tests restent `NOT_RUN`.
Les corrections de métadonnées/ordre et de noms des propositions avant
exécution restent conservées séparément. Combined-r3 passe ensuite 34 fonctions
mais s'arrête avec **RC d'orchestration 85** : son sélecteur de lecture scoped
utilise un mauvais namespace Rust. Le garde de liste fraîche refuse la sélection
vide. La fermeture reste RC 0 ; cette campagne n'est pas réécrite en run global
réussi. Remaining-r1 corrige le sélecteur du harnais, sans patch applicatif, et
passe les 73 restantes. Ses propres reçus final/fermeture renvoient RC 0.

Le parcours opérateur prépare les sources Store13/Auth4 existantes, appelle la
vraie migration d'union vers Store14/Auth5 et publie l'activation ; il ne crée
ni personne nominative, credential ou grant. `--confirm-control-stopped` reste
une attestation manuelle, sans fencing d'un ancien consommateur. `check` valide
seulement fichiers privés/métadonnées. Les interruptions conservent les mêmes
sources, identifiants, configuration, journal et backups pour la reprise
documentée correspondante.

Le profil owned adopte délibérément les seuls GET humains identité/ressources.
`contexts_complete=false` décrit les appartenances inconnues lorsqu'une autre
source tenant est sautée ; une union à un seul tenant peut annoncer des
contextes complets. Les routes connues non adoptées refusent avec 503 avant
legacy/corps/Store ; URI ou méthode inconnue garde le comportement normal 404/405.
OIDC sélectionné reste dans l'union avec ses handlers connus fermés, y compris
status/CLI-profile. Les GET existants status/CLI-profile d'état désactivé sont
admis seulement si OIDC est absent. `/healthz` GET/HEAD consulte la mémoire et
ne prouve pas la disponibilité des sources.

Ce résultat ne qualifie ni coupure électrique physique, macOS natif, workers
métier ou leurs vrais adaptateurs Docker/Podman/Kubernetes, IdP/fournisseur réel,
frontend/navigateur, Clippy strict, suite complète, release ou 100 workers.
Sessions/révocation OIDC modernes, approbation/lancement humain, permission ou
filtrage egress et provisionnement moderne d'identités restent des exigences
obligatoires ouvertes. Le défaut legacy est inchangé. Aucun constat d'audit ni
gate de production G01–G12 n'est clos.
