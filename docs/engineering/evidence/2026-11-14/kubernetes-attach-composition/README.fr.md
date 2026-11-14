# Composition attach Kubernetes — 14 novembre 2026

[English](README.md) · [Sélections et reçus exacts](SELECTIONS.json)

**Build/check et 11 parents logiciels ciblés passent sur `1e30e5e`.**
Quatre contrats utilisent les vrais owner/codec avec peers synthétiques.
Sept tests lancent le binaire Rust normal `ai-guard-provider` avec de vrais
pipes stdin/stdout/stderr. Son mode d'attente interne et ses arguments sont
réellement exécutés ; aucun client ou modèle IA, vrai worker, cluster
Kubernetes ou contrôle réseau produit. Aucune activation de production ouverte.

## Source testée et commandes réelles

Commit testé :
`1e30e5e8546ab614eba7849789a1a0468cbc2843` ; arbre :
`25f1b907c028a68679d4b9e02b8760e2aa4ea998` ; archive d'entrée SHA-256 :
`ec0216f959edd6189102378d0c3250270b26ebd60d8ed177ae74859df380281e`.
La base documentaire/composition ultérieure
`3b538f7323141c4488baa031f93da5f9b0dc30cf` n'est pas testée par cette campagne.
La [composition inspection 45179db](../owned-policy-inspection/README.fr.md)
a sa preuve propre de 47 parents, avec répétition de ces onze contrôles.
Ce relevé reste onze sur `1e30e5e`, sans addition ni transfert de qualification.

La QA admise exécute :

```console
/usr/local/cargo/bin/cargo test --frozen --offline --lib --test kubernetes_waiting_cli --no-run --message-format=json -j1
/usr/local/cargo/bin/cargo check --frozen --offline --all-targets -j1
```

Les deux retournent RC0. Le résultat primaire ne donne aucun nombre
d'avertissements du compilateur ; aucun n'est inféré. Le vrai JSON Cargo
lie trois exécutables : libtest de bibliothèque, libtest d'intégration
`kubernetes_waiting_cli` et binaire normal `ai-guard-provider` avec
`profile_test:false`. Les tests sélectionnent ce binaire normal par
`CARGO_BIN_EXE_ai-guard-provider`, sans surcharge manuelle du binaire.
Aucun de ces artefacts opaques n'est exécuté sur l'hôte.

Quatorze listes réelles précèdent l'exécution : quatre inventaires
globaux/ignorés, deux listes stdio sélectionné/ignoré et huit listes de
contrats individuels/ignorés. Quatre groupes exacts de contrat passent un
parent chacun ; le groupe stdio passe sept noms exacts. Tous utilisent
`--test-threads=1 --nocapture`. [SELECTIONS.json](SELECTIONS.json) garde
noms, ordre des argv et empreintes des artefacts. **11 PASS uniques, zéro
FAIL et zéro sélectionné ignoré**, sans résultat de suite complète ni
assertion sur les helpers non sélectionnés.

## Contrats et résultats des subprocess observés

| Contrat | Transport réel | Comportement logiciel observé |
|---|---|---|
| 1. Upgrade initié, appelant perdu | Requête d'upgrade HTTPS vers un peer TLS loopback synthétique possédé | Le travail HTTP conserve la capacité de tentative jusqu'à sa fin. La perte de l'appelant ne rend pas immédiatement la capacité. |
| 2. Pong bloqué après Ready | `tokio::io::duplex(64)`, en mémoire | Une vraie écriture bloquée consigne un délai d'IO typé. |
| 3. Écriture bloquée après Ready avec annulation | `tokio::io::duplex(64)`, en mémoire | La demande d'annulation reste une cause séparée à côté du délai d'IO observé. |
| 4. Payloads de close invalides | play→open→`tokio::io::duplex(pipe)`, en mémoire | Le vrai lecteur du flux les refuse. |

Seul le contrat 1 exerce HTTPS/TLS sur loopback. Les contrats 2–4 prouvent le
comportement réel owner/codec/lecteur sur flux mémoire, sans preuve TCP/TLS.
Ces peers sont des fixtures, pas un serveur d'API ou un kubelet ; aucune
instance de Pod n'est authentifiée.

| Cas stdio réel du binaire normal | Sortie enfant / résultat attendu |
|---|---|
| EOF avant Hello | 66, aucun stdout |
| Hello puis EOF | Un seul Ready corrélé, métadonnées déclarées ; sortie 66 |
| Argument supplémentaire | 64, aucun stdout |
| Supplémentaire non UTF-8 | 64, aucun stdout |
| Préfixe tronqué puis EOF | 64, aucun stdout |
| Préfixe retenu avec stdin encore ouvert | 65, aucun stdout ; writer encore possédé quand l'enfant est observé terminal |
| Premier argument non UTF-8 | 126, aucun stdout |

Les sept cas utilisent un environnement nettoyé, un répertoire privé neuf
et des pipes bornés. L'oracle du préfixe retenu lit le vrai statut terminal
du processus en conservant le writer stdin ; il ne provoque pas EOF pour
fabriquer le résultat. Le superviseur enfant est borné à 15 s, stdout à 16388
octets et stderr à 8192 octets. Le cleanup d'erreur tue et reap l'enfant
avant de retirer sa seule fixture neuve. Ce sont des bornes de test, pas une
latence produit mesurée.

La sortie 0 de chaque test est distincte de la sortie enfant non nulle
attendue. Ready reste un écho, sans identité ni exécution métier ; EOF 66
signifie absence de handoff, pas succès de session. Les tests stdio
n'exercent ni transport attach ni image worker. Les quatre contrats de flux
sont des sélections distinctes.

## Conservation, profil QA et reçus vérifiés

Root vérifie les trois artefacts Cargo et les hashes, tailles et modes des
2805 fichiers source. L'archive post-run a pour hash
`b663b2ccc528e5142de8a9788c86baf1244ac388793fba7ae676a91e20ee5b7d` ;
c'est une sérialisation distincte de l'archive d'entrée, sans prétendre que
leurs octets sont égaux. L'export de fixtures retenu contient zéro fichier
régulier et zéro nœud ; son hash d'archive figure dans le relevé typé.

Les 56 RC d'étapes consignés valent 0, streams producteur/sink bornés compris.
La fermeture retourne 0 et le conteneur QA exact est absent. Le holder sleep
est arrêté ; ce n'est ni un crash worker ni une sortie de test. Le flush FD
hôte complet couvre séparément 493 fichiers réguliers single-link et 17
répertoires, 391272605 octets, sans symlink. Un flush ne qualifie ni coupure
électrique ni restauration.

Il s'agit d'une QA outil isolée : image épinglée
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
UID/GID 1000:1000, réseau none, racine en lecture seule, non privilégié,
capabilities retirées et no-new-privileges ; quatre CPU, 9 GiB mémoire sans
swap, 128 PIDs,4 GiB scratch et 64 MiB temporaires noexec, deux binds exacts
input/cache public en lecture seule. Aucun socket moteur, projet, home ou
credential monté. Pic mesuré 8960716800 sous 9663676416 octets ;
OOM/OOM-kill 0. Ces chiffres décrivent la QA build/test, pas une mémoire
worker, un vrai confinement ou une capacité 100 sessions.

| Reçu primaire | SHA-256 |
|---|---|
| Résultat réel | `dd197af11edfd46da7c70de6ce1096320e349fb841b4e6f4ab96bf10418a4c74` |
| Flush FD hôte complet | `90e23f7b1986cbb90ba46fa2d3c9e6be1803f7fb9c9e5c67d9e99056e892b506` |
| Addendum de précision de couverture | `92a82687117f7c5eb56518a13506640a7398ee0f5f7d45044fd07b2df24ae90c` |

L'addendum précise chaque transport sans modifier les reçus de résultat/flush
originaux, les comptes, artefacts ou la source testée. Le reçu du profil créé est aussi identifié dans
[SELECTIONS.json](SELECTIONS.json). Traces brutes, contenus des fixtures,
credentials et chemins privés ne sont pas publiés.

## Résultats historiques et limites ouvertes

Les 121 PASS sur `be6b12dce7f74e2062f7ad5524e12818bf7313b5` sont historiques
et ne sont pas rejoués sur `1e30e5e` ni ajoutés à ces 11. Cette preuve ne se
transfère ni à `3b538f7`, ni à un autre HEAD. La campagne inspection
ultérieure possède sa propre source, ses commandes et ses reçus dans le lien ci-dessus.

Kubernetes réel, worker/image, client/modèle IA, filtrage réseau produit,
confinement Docker/Podman, capacité 100 workers, Clippy strict et suite
complète restent NOT_RUN. S5, permis d'effet de production, fencing,
R4/R7/R10 et gates production restent ouverts. La garde scellée n'a aucune
implémentation de production ; aucun parcours de session produit n'atteint
l'attachement. Les [guides EN](../../../../setup/kubernetes-runtime/README.md)
et [FR](../../../../setup/kubernetes-runtime/README.fr.md) conservent
activation fermée, préambule déclaré, hypothèses et plan de qualification.
