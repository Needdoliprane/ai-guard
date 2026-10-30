# Séparation des credentials Jev et du contrôle

Lot correctif Rust borné, réalisé le 29 octobre 2026. Il ne modifie ni les quatre empreintes filesystem examinées séparément, ni le rapport formel Cloudflare/security-audit, dont les verdicts restent inchangés. Les exécutions ci-dessous sont des **tests de développement autorisés**, avec fichiers et secrets factices, et ne valent pas reproduction sous le sandbox formel de l’audit.

## Besoin et correction

Une même valeur pouvait être utilisée à la fois comme clé Jev, destinée à un tiers, et comme credential runner ou viewer du gateway. L’initialisation publiait alors les projections correspondantes ; le gateway démarrait avec cette configuration. Le registre vérifiait déjà l’unicité des credentials de contrôle entre tenants, mais la clé Jev n’entrait pas dans cette validation.

- [src/jev.rs:8](/Users/louis/Documents/ai-guard/src/jev.rs:8) ajoute un validateur commun minimal. Il refuse l’égalité avec n’importe quel credential runner/viewer fourni, sans inclure une valeur, un hash ou un identifiant de fichier dans le diagnostic.
- [src/operations.rs:420](/Users/louis/Documents/ai-guard/src/operations.rs:420) vérifie le triplet après lecture des sources, avant validation/publication des projections. Un refus ne remplace aucun contenu ou inode source/projection existant. Les ajustements de modes déjà appliqués par `source` restent identiques ; aucune clé n’est tournée.
- [src/main.rs:206](/Users/louis/Documents/ai-guard/src/main.rs:206) vérifie la clé Jev contre **tous** les credentials déjà lus, avant `JevClient::new`, l’ouverture des Stores et le service HTTP. La collecte précédente couvre aussi les tenants suivants du registre ; aucun arrêt au tenant A.
- `src/gateway_config.rs` reste inchangé : ses contrôles d’identités et d’unicité restent utiles, et sa liste complète alimente déjà le contrôle commun.
- [tests/runtime_operations.rs](/Users/louis/Documents/ai-guard/tests/runtime_operations.rs) vérifie le refus Jev=runner/viewer et la préservation du contenu/inode des six fichiers existants, ainsi qu’une réinitialisation conservant une clé Jev distincte.
- [tests/jev_credentials.rs](/Users/louis/Documents/ai-guard/tests/jev_credentials.rs) exécute le vrai binaire sur des configurations temporaires : refus local runner/viewer, refus du runner/viewer du second tenant, démarrage positif avec Jev distinct et healthz local. Les refus ne créent aucune base tenant. stdout/stderr ne contiennent aucun des credentials factices.

Le diagnostic de collision est : `Jev key must be distinct from all runner and viewer credentials; existing values were not changed`. Les lecteurs existants normalisent les espaces de bord avant la comparaison. Il s’agit d’une égalité de valeurs, indépendamment des chemins de fichiers choisis.

## TDD et résultats constatés

| Étape | Commande | Résultat |
|---|---|---|
| RED gateway | `CARGO_INCREMENTAL=0 cargo test --offline --locked --test runtime_operations --test jev_credentials` | **FAIL attendu**, exit 101 : les quatre cas de collision gateway annoncent effectivement un service HTTP. Le positif distinct passe. Cargo arrête avant `runtime_operations`. |
| RED init | `CARGO_INCREMENTAL=0 cargo test --offline --locked --test runtime_operations` | **FAIL attendu**, exit 101 : la nouvelle régression montre que l’initialisation accepte la collision ; huit tests existants passent. |
| GREEN intégration | `CARGO_INCREMENTAL=0 cargo test --offline --locked --test runtime_operations --test jev_credentials` | **PASS**, exit 0 : 5 tests gateway et 9 tests d’opérations. Rejoué après remplacement du port fixe par une socket loopback possédée par chaque fixture : mêmes 14 PASS. |
| Compatibilité Jev | `CARGO_INCREMENTAL=0 cargo test --offline --locked --lib jev::tests` | **PASS**, exit 0 : 3 tests existants, 35 filtrés. |
| Compatibilité registre | `CARGO_INCREMENTAL=0 cargo test --offline --locked --bin agentsandbox gateway_config::tests` | **PASS**, exit 0 : 4 tests existants, 5 filtrés. |
| Clippy ciblé | `CARGO_INCREMENTAL=0 cargo clippy --offline --locked --bin agentsandbox --test jev_credentials --test runtime_operations -- -D warnings` | **PASS**, exit 0, aucune alerte. |
| Format owned | `rustfmt --edition 2024 --config skip_children=true --check src/jev.rs src/main.rs src/operations.rs tests/runtime_operations.rs tests/jev_credentials.rs` | **PASS**, exit 0. Aucun autre module n’a été reformaté. |

Les commandes utilisent le PATH explicite de l’assignation avec `/Users/louis/.cargo/bin` en tête. `--offline --locked` interdit une résolution/téléchargement de nouvelles dépendances. Aucune dépendance ni outil installé.

Logs bruts conservés : [RED](/tmp/ai-guard-jev-credential-red.log), [GREEN](/tmp/ai-guard-jev-credential-green.log), [Clippy](/tmp/ai-guard-jev-credential-clippy.log), [format](/tmp/ai-guard-jev-credential-fmt.log). **21 tests distincts PASS**, pas 35 : le log GREEN conserve aussi le premier passage des 14 tests d’intégration, sans le compter deux fois.

Les processus CLI de test sont bornés par timeouts, ont un environnement vide, travaillent dans leur répertoire temporaire et sont tués/récoltés de manière ciblée si nécessaire. Les endpoints Jev finaux appartiennent à des sockets loopback de fixture ; chaque test vérifie qu’aucune connexion n’y a été reçue. Seul le healthz du gateway positif est appelé. Aucun fournisseur, moteur, credential réel, volume existant ou endpoint partagé n’a été contacté.

## Acceptation et limites

**PASS :** clé Jev égale au runner ou viewer local refusée ; collision avec runner/viewer du tenant B refusée ; validation avant construction Jev, Stores et HTTP ; aucun credential dans les diagnostics testés ; contenu et inode des sources/projections existantes conservés à l’échec ; clé distincte utilisable et conservée à la réinitialisation ; aucune connexion fournisseur ; tests et contrôles ciblés ci-dessus.

**NOT_RUN :** suite Cargo complète, fmt global et Clippy tous targets, conformément à l’instruction de ne pas les lancer pendant les éditions des autres agents. Le parent doit les exécuter après gel intégré. Aucun front, Docker, Podman ou test fournisseur réel nécessaire à cette égalité de configuration n’a été rejoué.

**BLOCKED hors de ce lot :** qualification de release et validation formelle d’exploitation selon le sandbox de l’audit. Cette correction et ses tests de développement n’attribuent aucune sévérité ou verdict `confirmed` au rapport indépendant.

La revue indépendante et l’intégration finale restent à la charge de l’orchestrateur. Aucun fichier hors du périmètre d’écriture confié n’a été modifié ; le rapport filesystem demeure dans `/tmp/ai-guard-audit-filesystem-delta.md`.

## Extension après revue : assemblage bibliothèque GatewayState

La revue indépendante a identifié une omission du premier lot : les validations `init` et CLI ne protégeaient pas les consommateurs Rust construisant directement `GatewayState::new` ou `GatewayState::for_tenants` avec `Some(JevClient)`. Ce chemin était encore ouvert et a été corrigé avec un périmètre API explicitement cédé par l’orchestrateur.

`JevClient::new` normalise désormais les espaces de bord de sa clé, comme le font déjà les lecteurs de fichiers. `JevClient::validate_control_credentials` conserve la clé privée et valide sa séparation contre l’itérateur des contrôles de l’assemblage. `GatewayState::for_tenants` renvoie une erreur si elle égale le runner ou viewer de **n’importe quel tenant**. Le constructeur historique `GatewayState::new` conserve son contrat `Self` et le mécanisme existant de refus des identités invalides : une collision fixe `valid_credentials=false`, et toutes ses routes authentifiées répondent `503`. Le `Some(JevClient)` et la politique restent présents ; la collision ne désactive jamais silencieusement Jev ni une règle locale.

Les deux appels privés à `fresh_policy` sont précédés de `authorize` dans `decide` et `reserve_worker`. L’état invalide ne peut donc pas atteindre ces appels. Les nouveaux tests du constructeur historique vérifient d’abord les lectures d’identité runner/viewer, puis une demande de décision qui serait localement autorisée ; elle reste `503`, sans événement supplémentaire dans le Store et sans connexion à la socket de fournisseur factice.

Nouvelles régressions : constructeur local et assemblage multi-tenant × runner/viewer × clé exacte ou entourée d’espaces/tabulation/newline. Les huit cas ont été observés **RED**, exit 101 : local renvoyait `200` à l’identité au lieu de `503`, et multi-tenant construisait l’état au lieu de le refuser. Aucun cas RED n’a atteint une décision fournisseur : le test local s’arrête sur l’identité incorrectement autorisée. Un positif supplémentaire conserve l’autorisation locale lorsque Jev est distinct.

| Vérification de l’extension | Résultat constaté |
|---|---|
| `CARGO_INCREMENTAL=0 cargo test --offline --locked --test jev_credentials library_` avant correction | **FAIL attendu**, 0 PASS/8 FAIL, exit 101 ; [log RED bibliothèque](/tmp/ai-guard-jev-library-red.log). |
| `CARGO_INCREMENTAL=0 cargo test --offline --locked --test jev_credentials --test api_security --test tenant_api --test runtime_operations --test gateway_tenancy` après correction finale | **PASS**, exit 0 : **31 tests** = 14 Jev + 4 API security + 3 tenant API + 9 opérations + 1 registre HTTP/restart. Le log conserve aussi un passage intermédiaire de 29 tests avant ajout du positif local et du rejeu registre ; ne pas additionner ces passages. |
| `CARGO_INCREMENTAL=0 cargo test --offline --locked --lib jev::tests` | **PASS**, 3 tests existants, exit 0. |
| `CARGO_INCREMENTAL=0 cargo test --offline --locked --bin agentsandbox gateway_config::tests` | **PASS**, 4 tests existants, exit 0. |
| `CARGO_INCREMENTAL=0 cargo clippy --offline --locked --bin agentsandbox --test jev_credentials --test api_security --test tenant_api --test runtime_operations --test gateway_tenancy -- -D warnings` | **PASS**, exit 0, aucune alerte. |
| `rustfmt --edition 2024 --config skip_children=true --check src/jev.rs src/api.rs src/main.rs src/operations.rs tests/runtime_operations.rs tests/jev_credentials.rs` | **PASS**, exit 0. |

Total actuel : **38 tests distincts PASS dans le périmètre ciblé**, sans reprise du compte historique 209 et sans somme des passages répétés. Logs additionnels : [GREEN bibliothèque](/tmp/ai-guard-jev-library-green.log), [Clippy bibliothèque](/tmp/ai-guard-jev-library-clippy.log), [format bibliothèque](/tmp/ai-guard-jev-library-fmt.log). Les limites de qualification formelle et les contrôles globaux `NOT_RUN` décrits plus haut restent applicables.

Gel transmis pour relecture à `audit_runtime_delta`, puis intégration par le parent. Empreintes SHA-256 au gel :

```text
823451d1a68c91e4985237d24934bd896c10a808b40df25844bff3b8a61c0b79  src/api.rs
157b05a5a6c2a2ba7a9dda43b261d7f9044b062a9e4c2f11b7d3de3290f72344  src/jev.rs
dfc772e81dd26a19de7c842f834c38010894068ae10373fc21bc1282212f2822  src/main.rs
a7edf62b13631e5ca17d57d595457057f046b10acb84f07076699bdd8862d42e  src/operations.rs
2bc67301de756f77b05eafb780975b5f9f7098d4ed1879fb1c0802805e07fa24  tests/jev_credentials.rs
4bc8e51d733f372840073bf32160df0e522081434ac6bb67c22dffa69a3318e9  tests/runtime_operations.rs
```
