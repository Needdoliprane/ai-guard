# Correction de développement — fragments de secrets aux bornes de capture

Lot logiciel du 29 octobre 2026, distinct de l'audit formel Cloudflare. Aucun verdict de cet audit ni fichier de ses rapports n'a été modifié. Ses 0 confirmed / 19 needs_validation et ses limites de sandbox restent inchangés. Le gel précédent 209 et l'image QA précédente ne contiennent pas ce correctif ; aucune qualification de cette image, de Docker/Podman ou de production n'est déduite de ce lot.

## Changements et contrat

Fichiers modifiés exclusivement :

- `src/redaction.rs` : `Redactor::captured_text(&[u8], truncated: bool) -> String`.
- `src/adapters.rs` : `parse_redacted_capture`, résultat `NormalizedCapture` et plafond existant 500 événements rendu explicite.
- `src/runner/managed.rs` : utilisation pour stdout/stderr avant clip8192 et pour la normalisation provider.
- `tests/redaction_boundaries.rs`, `tests/runner_terminal_audit.rs`, `tests/runtime_fixtures/runner_terminal.rs` : régressions et fixture Rust.

`captured_text` traite le flag de capture sans changer les octets totaux observés ni les durées/résultats. Pour une capture tronquée, KMP marque les occurrences littérales complètes des secrets connus et les suffixes qui sont des préfixes propres de ces secrets **sur les octets originaux**. Leur union est remplacée par des marqueurs avant conversion UTF-8 lossy, puis le masquage textuel habituel s'applique. Cela préserve les portions utiles hors des régions masquées, y compris lorsque le dernier octet est au milieu d'un caractère UTF-8.

Cette union est nécessaire : couper simplement le plus long suffixe peut entamer l'occurrence précédente d'un secret périodique ; remplacer les secrets complets avant de chercher le suffixe peut aussi dissimuler le préfixe d'un secret long contenant un secret court. Les régressions couvrent ces deux interactions. Complexité du nouveau passage : **O(K×N + somme des longueurs des secrets))**, où K est le nombre de secrets connus et N le nombre d'octets capturés ; mémoire O(N + somme des longueurs des secrets)). Chaque recherche KMP et marquage non chevauchant est linéaire par secret, sans essai descendant quadratique de tous les suffixes. Les captures du runner restent bornées à 256 KiB ; le passage ne promet pas de reconnaître des encodages arbitraires ou des fragments que le worker a lui-même fabriqués ailleurs dans le flux.

Le provider ne reçoit aucun masquage textuel/littéral sur le JSON brut : un secret contenant guillemets et antislash pourrait aussi correspondre à la syntaxe d'un autre champ. `parse_redacted_capture` conserve toutes les lignes délimitées par LF, puis utilise le masquage **après décodage JSON**, avant les clips et budgets existants. Si la capture est tronquée, seule sa dernière ligne sans LF est écartée, même si le préfixe retenu ressemble à un JSON complet. Le diagnostic de couverture publie `unterminated_final_line_omitted`; le mot « unterminated » décrit l'absence de délimiteur, sans inventer la cause de la troncature. Une capture non tronquée accepte toujours un dernier record valide sans LF. Les compteurs/drapeaux `stdout_truncated`, `stderr_truncated`, octets observés, maximum500 et provenance de collecte restent présents.

## RED puis GREEN

Le premier RED reproduit le défaut réel de développement avec `capture_stream` : secret factice de 4097 octets répété 64 fois, 262208 octets émis, capture262144. Le pipeline antérieur conserve un fragment après remplacement des occurrences complètes et clip8192. Le test échoue sur l'absence attendue du fragment ; il n'imprime pas sa valeur. Log : `/tmp/ai-guard-capture-secret-red.log`.

Un deuxième RED montre que l'ancien parcours par `.lines()` accepte un dernier record non délimité malgré le flag truncated : `/tmp/ai-guard-capture-secret-provider-red.log`. Un troisième RED protège le cas de secrets courts/longs imbriqués : `/tmp/ai-guard-capture-secret-overlap-red.log`. Le log final retenu de ce troisième cas a été régénéré avec un message d'échec ne contenant pas la valeur factice.

Commande GREEN exécutée après la dernière correction :

```sh
CARGO_INCREMENTAL=0 cargo test --locked --test redaction_boundaries --test runner_terminal_audit --test runtime_authorization --test runner_recovery
```

**PASS : 23 tests, 0 échec, 0 ignoré** dans `/tmp/ai-guard-capture-secret-green.log` : redaction_boundaries9, runner_recovery7, runner_terminal_audit1, runtime_authorization6. Le harnais terminal unique comporte dix scénarios, à ne pas compter comme dix tests Cargo. Son nouveau scénario vérifie les événements durables stdout **et** stderr, le texte utile conservé, l'absence de fragment, les drapeaux truncated et les vrais compteurs d'octets. Le cas4097×64 est testé directement avec le collecteur réel en mémoire ; le scénario broker utilise une valeur de contrôle factice503 octets autorisée par les bornes des tokens, répétée522 fois, pour tester le câblage complet sans modifier les contrats d'injection/réseau.

Les neuf tests de frontières couvrent ASCII, occurrences répétées et périodiques, dernier fragment, texte utile, capture non tronquée, caractère UTF-8 coupé, plusieurs secrets et fallback KMP, secrets imbriqués, derniers records avec/sans LF, JSON contenant guillemets/antislash, clé JSON et secret décodé avant projection.

```sh
rustfmt --edition 2024 --config skip_children=true --check src/redaction.rs src/adapters.rs src/runner/managed.rs tests/redaction_boundaries.rs tests/runner_terminal_audit.rs tests/runtime_fixtures/runner_terminal.rs
cargo clippy --locked --lib --test redaction_boundaries --test runner_terminal_audit --test runtime_authorization --test runner_recovery -- -D warnings
```

**PASS** pour les deux commandes, logs `/tmp/ai-guard-capture-secret-fmt.log` (vide, exit0) et `/tmp/ai-guard-capture-secret-clippy.log` (exit0). Contrôle des quatre logs RED/GREEN retenus : aucune des valeurs factices sentinelles recherchées n'y apparaît. Aucune valeur de credential existant n'a été copiée dans ces rapports ou logs.

## Acceptation et limites restant à intégrer

**PASS dans la portée testée** : masquage conscient de la capture sur les octets, conservation de sortie utile, absence des fragments connus aux frontières testées, JSONL complets encore exploitables, déclarations de troncature et mesures observées conservées, refus d'autorisation et reprise inchangés dans les tests ciblés.

**ÉCART OUVERT hors fichiers possédés** : `Runtime::capture_cli` dans `src/runtime.rs` capture stderr à64KiB puis construit une erreur `anyhow` à partir d'une chaîne (`runtime.rs:435–466` au point de départ de ce lot). Le flag de troncature et les octets originaux sont alors perdus avant le masquage du runner. Un fragment remonté sous le clip par les autres remplacements peut y survivre ; ce chemin n'est pas déclaré corrigé. Contrat proposé au propriétaire runtime : conserver un diagnostic typé avec `bytes` et `truncated` (et contexte/status non sensibles), afficher par défaut un contexte sans contenu brut, puis laisser le point qui connaît Redactor appliquer `captured_text` au diagnostic. Aucun fichier runtime n'a été modifié par cet agent.

**NOT_RUN** ici : suite Cargo globale, build release, front/navigateur, vrais moteurs et nouveau déploiement. La suite globale est réservée au coordinateur après la fin des éditions parallèles. Le contrôle négatif de concurrence/ACL/filesystem de l'audit formel et le sandbox complet requis pour son verdict ne sont pas accomplis par ces tests de développement. Les collisions Jev, ServeDir, autres chemins de credentials et les autres candidats de l'audit restent hors correction de ce lot.

**Revue indépendante** : demandée au coordinateur ; ce rapport ne la remplace pas. La documentation des gates, preuves et qualification globale doit être intégrée par le coordinateur, qui possède ces fichiers.

## Empreintes des fichiers au GREEN ciblé

```text
21d52065319eb303b4eb414cc7f4965e953b1c4327eebe572c0fc81a51cb92f5  src/redaction.rs
0bd3479b5a540844c9d50ad13526c7749b0e6caa6cf0eecf1071fb71c4f145d4  src/adapters.rs
320f2222ae06c34eb8a8403f96567c3127fd4e8c54552d9bf88f35b7bc9749d7  src/runner/managed.rs
ff34fe361ab0a3dc6c398dc61183e1bcfe2370454ce550150c7bad3e4f084f6c  tests/redaction_boundaries.rs
57d151b2c16b073269ab610430eddf44f1b89bd5f1c5bd44f66872cb43f6ed69  tests/runner_terminal_audit.rs
c43e59be622fadd7825e12bb3e2dc385603ff77882a0312c3866bb2b09eddff6  tests/runtime_fixtures/runner_terminal.rs
```

## Complément final — diagnostics CLI64KiB et matchs complets chevauchants

Ce complément succède à la première phase ci-dessus, dont les logs et empreintes restent historiques. **L'écart CLI64KiB décrit dans cette première phase a maintenant une correction logicielle intégrée et des régressions PASS dans la portée ci-dessous.** Cela ne change aucun verdict de l'audit formel ni la qualification du gel209/image antérieurs.

Le coordinateur a ajouté dans les fichiers runtime qu'il possède `RuntimeCommandFailure`, dont Display/Debug excluent stderr brut et les valeurs de redaction privées. Le type conserve les octets/counters/flag originaux jusqu'au point de confiance qui possède le dictionnaire. `ManagedRuntime::cli_sensitive` conserve cette cause typée dans un contexte sûr, transporte les références privées Podman et couvre aussi le bootstrap avant installation du binding. Le getter `stderr_capture` ne sert ici qu'aux métadonnées ; le formatter runner appelle `failure.redacted_stderr(redactor)` pour le contenu.

Dans les fichiers possédés par cet agent :

- `Redactor::with_secrets(&self, values)` fusionne les dictionnaires via `new`, sans exposer les valeurs ni muter l'original. `RuntimeCommandFailure::redacted_stderr` peut donc masquer d'un seul coup les octets originaux avec les secrets privés runtime et les secrets contrôle/provider du caller. Aucune conversion UTF-8 ni remplacement privé intermédiaire ne détruit une correspondance du dictionnaire complet.
- Le helper interne `runner::redacted_error` parcourt toutes les causes anyhow, masque les causes ordinaires, conserve les contextes et ajoute les métadonnées `stderr_retained_bytes`, `stderr_total_bytes`, `stderr_truncated` puis le diagnostic typé masqué. Tous les anciens formatages `{error:#}` de `runner.rs` et `runner/managed.rs` passent par lui : retour final run, Unknown.reason, contrôle en cours d'exécution, cleanup, postprocessing et cleanup d'import.
- Les contrôles sont échappés **après masquage** pour que les diagnostics restent lisibles et puissent entrer dans les champs worker bornés. Un LF de stderr auparavant caché par le map_err générique avait déclenché un HTTP400 dans `runner_recovery::recovered_create_and_import_keep_the_same_instance_and_first_start` ; la régression de causes ordinaires avec LF/TAB reproduit cette incompatibilité avant correction. La reprise passe de nouveau avec le contexte utile conservé sous forme d'échappements.
- La revue indépendante a trouvé un dernier cas : le marquage des occurrences complètes ignorait les chevauchements en remettant KMP à zéro. `mark_literal_bytes` reprend maintenant `prefix[len-1]` après chaque match ; un frontier propre au pattern ne marque que la portion encore non couverte par ce pattern. Tous les matchs complets sont pris en compte et chaque octet est marqué au plus une fois par pattern. La borne **O(K×N + somme des longueurs des secrets))** reste valable. Les mentions « non chevauchant » de la première phase décrivent son algorithme antérieur, remplacé par ce complément.

Preuves RED complémentaires retenues, sans sortie des sentinelles :

- `/tmp/ai-guard-capture-secret-control-red.log` : diagnostic CLI64KiB non récupéré avant intégration du helper.
- `/tmp/ai-guard-capture-secret-control-diagnostic.log` : les booléens de contrôle identifient la destruction de la cause par l'ancien map_err, sans imprimer les données.
- `/tmp/ai-guard-capture-secret-control-chars-red.log` : LF/TAB incompatibles avec un worker reason borné.
- `/tmp/ai-guard-capture-secret-overlapping-complete-red.log` : secret court périodique puis secret4096 octets avec match complet chevauchant et sentinelle finale hors préfixe ; l'ancien algorithme laisse un fragment, le nouveau masque jusqu'au dernier octet de l'union.

Commandes finales exécutées sur les fichiers stables :

```sh
CARGO_INCREMENTAL=0 cargo test --locked --test redaction_boundaries --test runner_terminal_audit --test runtime_authorization --test runner_recovery
CARGO_INCREMENTAL=0 cargo test --locked --lib runner::tests::ordinary_error_causes_keep_context_without_known_secrets
rustfmt --edition 2024 --config skip_children=true --check src/redaction.rs src/adapters.rs src/runner.rs src/runner/managed.rs tests/redaction_boundaries.rs tests/runner_terminal_audit.rs tests/runtime_fixtures/runner_terminal.rs
cargo clippy --locked --lib --test redaction_boundaries --test runner_terminal_audit --test runtime_authorization --test runner_recovery -- -D warnings
```

**PASS : 25 tests ciblés / 0 FAIL / 0 ignoré**, log `/tmp/ai-guard-capture-secret-control-green-final.log` (frontières11, reprise7, harnais terminal1, autorisation6). **PASS : 1 test unitaire formatter / 0 FAIL / 0 ignoré**, log `/tmp/ai-guard-capture-secret-ordinary-error-green-final.log`. Le harnais terminal unique compte désormais onze scénarios ; les deux erreurs runtime conservent allocation/terminal inconnus, et le nouveau scénario64KiB vérifie CLI stderr, Unknown.reason et événement d'audit sans fragment ni invention de résultat. **PASS fmt** (log vide exit0 `/tmp/ai-guard-capture-secret-control-fmt-final.log`) et **PASS Clippy -D warnings** (`/tmp/ai-guard-capture-secret-control-clippy-final.log`, exit0). Le log runtime du coordinateur `/tmp/ai-guard-control-capture-error-green-final.log` a été lu : 1 PASS / 0 FAIL / 0 ignoré, preuve distincte du type opaque et du dictionnaire fusionné ; aucun moteur réel n'y est déduit.

La validation globale et le second regard sont coordonnés par le parent après cette remise ; les sources possédées sont stables. Les limites toujours applicables : dictionnaire de secrets connus et littéraux, encodages arbitraires non couverts, dernière ligne provider non délimitée explicitement écartée si la capture est tronquée, aucun moteur/nouveau déploiement/front/release exécuté par cet agent et aucune qualification de production. Ce sont des tests de développement autorisés, pas une validation sandbox complète du verdict de l'audit initial.

Empreintes des fichiers possédés au GREEN final :

```text
e18aaa022a343a6c14f0133aced38a637ff5e8a4089d1b25e3a17be2fbba8b1d  src/redaction.rs
0bd3479b5a540844c9d50ad13526c7749b0e6caa6cf0eecf1071fb71c4f145d4  src/adapters.rs
49f1a8b8637e3b69aec148258d75e168f26d2b11c01d1baaa6d3348482974609  src/runner.rs
401f7bdfd0d88fd7921f6d01042b44ddb71385e00d3d8f6343f72b4aafe7e6bd  src/runner/managed.rs
6daff1d665735fc42b27f6360c1916e085a0f3d891bb9b5fd8b05a96c00ec5c9  tests/redaction_boundaries.rs
3f506aadd4be3b20e472e27bb00dcf58ea552c1281d7a8c531988b26d17cc408  tests/runner_terminal_audit.rs
ce8550634c0effdbf5d040bdd03d6fe7c9617f1f0a0a10b1c76edb1edb2c2dd6  tests/runtime_fixtures/runner_terminal.rs
```
