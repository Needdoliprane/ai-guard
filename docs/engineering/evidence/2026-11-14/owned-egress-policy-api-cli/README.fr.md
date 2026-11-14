# API/CLI A1 des politiques egress owned — 14 novembre 2026

[English](README.md) · [Sélections et reçus exacts](SELECTIONS.json)

**74 parents logiciels ciblés passent sur une source exacte. Le parcours
opérateur reste incomplet.** A1 raccorde replace/revoke du registre de
politiques à l'API technique réservée et aux quatre opérations CLI
prepare/apply/resume/status. Il n'applique aucune politique réseau, n'autorise
aucune connexion, ne lance pas de worker et n'établit aucune approbation humaine.

## Source et deux QA finales distinctes

Les deux campagnes finales utilisent le commit
`db2678d42cff9c8d6d15bf31284a989732cc6f0d`, arbre
`79a378461d75d46a8d102c9126d04251422b4dda`, archive d'entrée SHA-256
`40719c46c00d1793cea3518a48950a71501f00982e56cc672480729060a2a88d`.
La preuve reste liée à cette source. Elle ne se transfère à aucun autre HEAD
ou public composé ultérieur. Les modifications documentaires sont préparées
sur le public `0dab5f78464931301183824eb81724488981ce2e`, qui n'est pas la
source testée.

| Campagne | Résultat constaté |
|---|---|
| GREEN R2 build/check | Build des artefacts de test, build du CLI normal et check toutes cibles RC0 ; 156 messages d'avertissement du compilateur. Le harnais hôte Bash 3.2 sort ensuite en 1 sur un tableau local vide sous nounset avant enregistrement. Zéro liste, les 74 tests NOT_RUN dans cette QA. |
| Reprise tests-only | Réimporte les trois exécutables Cargo-bound originaux, byte-exacts aux chemins isolés d'origine. Aucun rebuild, check ou téléchargement. Dix-huit listes fraîches puis huit groupes : 74 parents uniques PASS, 73 ordinaires et un default-ignored explicitement exécuté. Zéro échec et aucun sélectionné laissé ignoré ; terminal 0. |

L'échec du harnais build/check reste un échec conservé ; la reprise réussie
est une preuve distincte. La reprise hérite des vrais build et check RC0
antérieurs, sans prétendre à une nouvelle compilation.

Commandes de build réellement utilisées dans la QA offline admise :

```console
/usr/local/cargo/bin/cargo test --frozen --offline --lib --bin agentsandbox --no-run --message-format=json
/usr/local/cargo/bin/cargo build --frozen --offline --bin agentsandbox --message-format=json
/usr/local/cargo/bin/cargo check --frozen --offline --all-targets
```

Libtest de bibliothèque, libtest du binaire et CLI normal sont résolus depuis
le vrai JSON Cargo. Le parent vertical appelle le CLI normal. Les 18 listes
sont deux globales, huit de groupes exacts et huit de groupes ignorés.
Les argv fonctionnels utilisent les FQN avec `--exact` et
`--test-threads=1 --nocapture` ; le vertical ajoute `--ignored` et l'artefact
vérifié du CLI normal. Noms exacts, ordre des arguments, SHA-256 des
artefacts et hashes du JSON Cargo figurent dans
[SELECTIONS.json](SELECTIONS.json) ; aucun credential brut ou chemin privé
n'est publié.

## Sélections et comportements constatés

| Groupe | PASS |
|---|---:|
| Contrats core API/SDK/Store et canari ACK | 19 |
| Canaris E1 existants du registre | 19 |
| Canaris A0 existants | 9 |
| Façade d'opération filesystem durable | 8 |
| Libtest CLI principal | 10 |
| Canari CLI de baseline d'absence | 1 |
| Lecteurs modernes partagés | 7 |
| Vertical CLI normal / HTTP, ignoré par défaut exécuté explicitement | 1 |
| **Total unique** | **74** |

Les vrais tests Router/SDK/SQLite couvrent identité technique retenue et
binding actif de source, format 15 avant lecture du corps, conflits scope/CAS
avant mutation/rejeu, entrée fermée bornée, vrai travail WriteEpoch après
corps, refus Mutex/SQLite et restauration du timeout 713 ms. SQLite-FULL
borne une base logique, sans remplir physiquement un disque. La perte de
l'observateur laisse travail accepté et permits liés au vrai job jusqu'au
drain ; le canari ACK prouve la réception de la libération par le waiter,
pas seulement un envoi. Un binding révoqué après vrai commit ne peut plus
publier le reçu.

Les sélections filesystem/CLI conservent la préparation immuable, refusent
les enregistrements dangereux ou changés sans réparation, exigent une
reprise explicite du pending et vérifient les synchronisations
fichier/répertoire d'opération/parent. Une faute injectée après un vrai sync
n'est pas un errno fsync observé. Le vertical CLI normal perd le reçu après
vrai commit, conserve l'opération, rejoue les mêmes octets/UUID et demande
la révocation. Son reçu est historique, `policy_registry_only` ;
`revoke_requested` n'est pas une fermeture runtime observée.

## Échecs et baseline conservés

- La baseline test-only sur
  `ae8d5210272f2af903da53f999ab03d63d1fd086` compile et liste ses deux
  vrais callers. Tous deux atteignent les oracles d'absence attendue après
  setup : diagnostic SDK réservé 200 puis POST 404 au lieu de 200 ;
  control-info existant parsé mais policy/status refusé comme sous-commande
  invalide. Chaque parent sort 101, zéro PASS. Ces échecs attendus d'absence
  de fonctionnalité ne sont ni des vulnérabilités ni des PASS.
- GREEN R1 sur `0abb6efe045701e0d3f76548a15392db8159c41d` sort 101 à la
  compilation avec trois erreurs Rust. Tests, CLI normal et check NOT_RUN.
- L'échec de harnais 1 ultérieur de GREEN R2 reste consigné ; seule la reprise
  tests-only distincte apporte les 74 PASS. Les campagnes recouvrantes ne
  sont pas additionnées.

## Conservation et profil QA borné

Chaque QA finale préserve les 2813 fichiers source, dont 18 privés 0600,
chemins/contenus/modes exacts. Chacune termine sept collectes et fermeture
RC0 ; son conteneur QA exact est retiré et OOM/OOM-kill valent 0.
Les hashes différents des archives post-run du relevé typé décrivent leurs
sérialisations propres, sans affirmer que les octets d'archive d'entrée et
de sortie sont identiques.

La reprise utilise une image outil Linux ARM64 épinglée
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
UID/GID 1000:1000, réseau none, racine en lecture seule, toutes capabilities
retirées, no-new-privileges et seccomp 2 ; deux CPU, 4 GiB mémoire, aucun swap,
128 PIDs, 2 GiB scratch et 64 MiB temporaires noexec. Pic mémoire mesuré :
1252507648 octets, OOM 0. Cela mesure la QA, pas un worker produit, un débit
ou une capacité 100. Aucun exécutable opaque n'est lancé sur l'hôte.
Les flushs FD des reçus et répertoires sont vérifiés séparément, sans
qualification de perte de courant ou de restauration.

SHA-256 des résultats/flushs primaires :

| Preuve | SHA-256 |
|---|---|
| Résultat build/check | `c24896a4bc8fa649d23891e165dd00b13b5e65c27d4af9c00418f99124eb100a` |
| Flush hôte build/check | `aaba0fd7ef4d38af2559b5e6d1342659565ac3ea162c760c4e5311d284ad81a3` |
| Résultat tests-only | `ae52ce89e84dc85102e9f3e555fd20fd5ff1275ede62c2a223ea50eda9d07a74` |
| Flush hôte tests-only | `c5c587f0f7ab1d1741c228651d73e88e7a46f4f1211fd4ddc51bd77d17ca8bff` |

## Parcours opérateur ouvert et limites de qualification

Il n'existe **aucune commande supportée pour obtenir un scope réservé et un
head/CAS de politique frais** avant préparation. Le control-info existant
utilise le diagnostic public fermé en owned ; status est un historique
local. Les quatre commandes exigent de vraies preuves de contrôle retenues,
jamais un scope déduit, un CAS deviné, une valeurs d'amorçage devinées ou du SQL
utilisateur. `expected_scope` est une assertion comparée, jamais un sélecteur
de Store/source/tenant ou une autorité.

`inspect` et `prepare --inspection-dir` ne sont pas implémentés. Leurs cinq
tests ultérieurs sur `05a4451d08856ce5253bce0103acf8b3d873057b` restent
NOT_RUN et sont exclus des 74. Le [guide opérateur](../../../../setup/identity/OWNED_SOURCES.fr.md#owned-policy-a1-operator)
conserve cet écart visible.

Aucune politique durable ni reçu n'établit filtrage appliqué, droit de
connexion, exécution observée ou approbation humaine. Lifecycle broker 3–13
et relais 6–13 restent fermés. Runtime/worker/réseau egress réels,
fournisseur/SSO, cluster, 100 workers, suite complète, Clippy strict,
crash/restauration physiques et tout autre HEAD composé restent NOT_RUN.
Aucun gate production ou fingerprint historique d'audit n'est clos par
cette preuve.
