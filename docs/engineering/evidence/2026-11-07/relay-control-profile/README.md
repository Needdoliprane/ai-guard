# Profils fermés du client relais — 7 novembre 2026

**PASS : 19 fonctions uniques et compilation toutes cibles sur le gel53.**
Le code et les régressions ont été relus indépendamment. La bibliothèque fournit
une sélection explicite entre accès gateway racine et ingress de contrôle ; elle
ne raccorde pas un worker, ne conserve pas son binding et ne livre pas d'outbox.
Aucun gate de production n'est clos.

## Source et contrat

Base du composant : `c6dc1d2` ; gel privé `relay-client-profile-fix53`.
Le [manifeste](source.sha256) couvre trois fichiers de code/tests, Cargo.toml,
Cargo.lock et les [guides EN](../../../../setup/relay/README.md)/[FR](../../../../setup/relay/README.fr.md).
Les guides ont été finalisés après les tests sans modifier ces trois sources.

`RelayClientProfile::{RootOrigin, ControlIngress}` choisit deux chemins constants.
`new` et `with_ca_file` conservent RootOrigin ; `with_profile` et
`with_profile_and_ca_file` sélectionnent explicitement le profil. L'origine
reste une origine racine, sans préfixe libre. Credential, endpoint attendu,
TLS, DTO, bornes et sémantique de résultat restent communs. Aucun repli entre
profils, aucun nouveau rôle ou sélecteur de tenant n'est introduit.

Le contrôle de l'autorité brute exige `http(s)://` suivi d'une autorité non vide,
sans `@`, avant normalisation URL. Le dispatch construit la requête finale,
query comprise, et refuse localement son credential connu dans l'URL avant
envoi. Ce refus est opaque `InvalidRequest` ; il ne résout jamais une tentative
antérieure dont le résultat demeure inconnu.

## Reproductions puis régressions

Trois défauts de contrat local ont été reproduits sur les sources précédentes,
dans des fixtures privées avec preuves factices. Ils ne modifient pas le nombre
des treize vulnérabilités du rapport indépendant :

| Gel et sélection | Résultat observé |
|---|---|
| 48, `client_profile_cases::relay_client_root_origin_refuses_empty_userinfo_before_transport` | RED : le constructeur historique accepte une autorité avec information utilisateur vide ; 0 PASS, 1 FAIL. |
| 51, même parent | RED : les variantes de séparateurs restent acceptées après un premier correctif ; 0 PASS, 1 FAIL. |
| 51, parent ACK de `client_profile_cases` | RED : une preuve factice contenue dans le chemin ACK construit atteint le pair local, une requête observée. |
| 53, `test --test worker_relay_api client_profile_cases::` | 6 PASS, 0 FAIL ; tests 3,64 s, compilation 1 min51. |
| 53, `test --test worker_relay_api client_cases::` | 13 PASS, 0 FAIL ; tests 7,86 s. |
| 53, `check --all-targets` | RC0 ; environ 1 min04, aucun test exécuté par ce check. |

Les commandes Cargo utilisent `--locked --offline`, les tests
`--test-threads=1 --nocapture`. Les journaux complets parent retiennent la sortie
et le code réel du wrapper. Les six nouveaux parents couvrent chemins fixes,
origines, identités/roles et endpoint attendu, inbox proche de 2 Mio avec
pagination et refus avant transport. Les treize tests historiques restent
distincts ; aucune addition de répétitions ne gonfle le total.

## Journaux et environnement

Journaux privés sous `ai-guard-cold-drain-logs-20261106.nXx9XJ` :

| Journal | SHA-256 |
|---|---|
| `relay-client-profile-repro48-userinfo-aj.log` | `8292b57fb8fa135d79877d287a4b438fe6a48c6aebfeaabcef261eeb8b086b35` |
| `relay-client-review-repro51-origins-aj.log` | `5d9f0639308a1d69f4e384c657aedb35957d55311eb5665e24792ad043770841` |
| `relay-client-review-repro51-ack-proof-aj.log` | `7f301b941c494e416f047b39c70a81af93dbca287fd33b8136bd21803d1f0ad5` |
| `relay-client-profile-fix53-profiles-aj.log` | `a6cbf95eebc09442891bd1013fb7e68177c3a97923c9682822149e42cda92075` |
| `relay-client-profile-fix53-compat-aj.log` | `990c874d9054b96a6af1e72e0fdc4e92867f65091ea073d91f0161a75b5c3452` |
| `relay-client-profile-fix53-check-aj.log` | `5bfd61396578df9033261e30818f05783eedbba98d9c14760020a5af26599da6` |

QA AJ Linux ARM64, image
`sha256:2115d4321fa70d045c7c73361444bdef10f69c9069e66a70ad27a73daa92a207`,
toolchain Rust1.99.1, UID1000:1000, capacités supprimées, NNP/seccomp, réseau
coupé avec loopback privé, racine/outils/registry/sources RO. Écritures limitées
au scratch de 4 Gio et 65536 inodes ; quatre CPU,8 Gio sans swap,256 processus,
fichier512 Mio, nofile1024, core0. Environnement vide puis allowlist, HOME/tmp/
Cargo et cible propres au gel. Aucune dépendance n'est téléchargée par Cargo.
Inspection initiale privée SHA
`27b96dd937c512b6fe74c1c261f4b48b3a7db489532522a9cdda1f9948165ee5`.

Le parent a constaté après les tests l'absence d'OOM et de processus de test
résiduel. Ce constat n'est pas présenté comme un journal de postflight distinct
publié. AJ s'est arrêtée normalement à `2026-11-07T19:17:36.146087721Z`,
RC0, OOMKilled=false, puis son conteneur attribué a été supprimé. Inspection
finale SHA `4925e8cc3798a51e3c3a6517ed641e4c1a7aa7bb35cc918a65dbdfc95385cf25`.
Les sources et journaux restent hors du scratch détruit ; aucun binaire, base
ou credential produit n'est promu vers le dépôt.

Les avertissements existants restent présents : 61 bibliothèque, deux
intégration SSO et deux bibliothèque-test au check. Les lignes récapitulatives
ne sont pas des défauts supplémentaires. Aucun Clippy projet, build sans
warning, image worker, HTTPS/AC privée réelle ou campagne fournisseur crédité.

## Limites et prochaine intégration

Les fixtures emploient de vrais HTTP/Axum/SQLite et des enregistrements worker
synthétiques. Docker/Podman/Kubernetes réels, canal exec, propriétaire broker,
liaison durable du profil, replay/outbox et recette A1/A2/B1 restent **NOT_RUN**
ou à implémenter pour ce transport. La revue et les résultats du composant ne
qualifient pas toute la branche composée, ni la fédération multi-hôtes.
