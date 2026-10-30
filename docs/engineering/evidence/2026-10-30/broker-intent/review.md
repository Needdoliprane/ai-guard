# Revue ciblée — intention d'exécution immuable

Base : `11eb871db40ca9893a4492547561ec604247257c`. Worktree exclusif : `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`.
Périmètre : revue statique du contrat et, après gel, du delta intention. Aucun test, moteur, modèle, modification de code ou de document du dépôt exécuté par le reviewer. Skills de projet `.agents/skills` et `.claude/skills` absents ; lecture de RTK, AGENTS et des contrats imposés effectuée.

## Relecture de constructibilité initiale

1. `src/runner.rs:827-860` : `validate_provider_plan` vérifie résolution/argv/binding ID, mais ne vérifie ni forme du digest Frozen ni cohérence du contenu de `provider_input` avec les options initiales et la résolution. Le nouveau composant ne peut le réutiliser comme validateur suffisant. Cas négatif utile : plan3 avec résolution Claude/API valide et `provider_input.client=codex`, `provider_input.auth_mode=subscription`, ou changement d'image/runtime/projet uniquement dans `provider_input`. Ces valeurs ne peuvent provenir de `choose_plan` et ne doivent pas devenir une identité initiale incohérente acceptée.
2. `src/runner/managed.rs:968` écrit `plan.request=chosen` après approbation. Une projection « demande initiale » ne doit pas silencieusement retirer `human_approved`/`approval_hash` de ce checkpoint ni le faire passer pour le snapshot initial conservé. Le refus du nouveau composant pour ces snapshots doit être attendu ; les anciens parsers restent inchangés.
3. `RunPlan.operation_id` peut être omis et devient UUID nil (`src/runner.rs:314-316`). Le nouveau composant refuse ces anciens plans sans inventer une opération et conserve la distinction format intention1 / plans1,2,3.
4. Nouveau format : désérialiser les structs stricts depuis les octets avant conversion en `Value`, pour refuser les champs dupliqués au lieu de conserver silencieusement la dernière valeur. Les clés inconnues doivent être refusées à chaque niveau d'options/request/résolution/binding concerné ; ne pas modifier le serde historique permissif de `RunOptions`.
5. La borne canonique de 256Kio doit s'accompagner d'une borne d'entrée avant parsing (pas d'allocation illimitée d'espaces ou de valeurs ensuite écartées). Source de plan bornée avant parse à32Mio. Vérifier les bornes sur les octets JSON réels avec échappements et textes UTF-8.
6. Les textes fournisseur/arguments non exécutables autorisent des sauts de ligne/Unicode. Conserver leur texte exact dans le JSON/hash ; les règles du domaine sur métadonnées et NUL ne doivent pas devenir un refus global de `char::is_control` pour les prompts.
7. Le binding projeté est uniquement sa référence typée ; aucun succès de ce composant pur ne prouve la présence ni le digest du fichier privé, aucune résolution Auto ni lecture de `.env` ne doit se produire. `Debug`, `Display` et erreurs doivent rester opaques même sur parse échoué contenant un canari en champ inconnu, valeur d'enum ou UUID.
8. Le domaine attendu est la concaténation exacte `ai-guard.execution-intent.v1` + octet0 + JSON canonique UTF-8. Clés triées récursivement, ordre des tableaux et texte conservés. Un golden calculé indépendamment doit détecter absence/double NUL ou mauvais domaine. Les hashes legacy demeurent distincts et inchangés.

Ces points sont des critères de construction, pas des défauts qualifiés de l'implémentation encore à venir. L'intention/UID ne prouve jamais une autorisation tenant ni un effet runtime.

## Statut final de la revue

**PASS au périmètre logiciel de ce lot ; BIR-01 fermé ; aucun constat ouvert identifié.** Le delta final et les preuves Luna sont examinés ci-dessous. Les sections préparatoires et le premier gel conservent les étapes de revue du 30 octobre ; leurs observations ne sont pas des blocages actuels. Journal/ACK/service/IPC/gardien/enforcement/fournisseurs restent hors de la livraison qualifiée par ce lot.

## Décision de contrat supplémentaire — credential manuel avant ACK

Le `secret_env_file` manuel est aujourd'hui seulement une référence dans les options ; le digest réel n'apparaît qu'au checkpoint `managed` après préparation/admission (`src/runner/managed.rs:1349`). Une intention pure projetée depuis un plan initial ne peut donc pas identifier tous les octets effectifs d'authentification manuels. Le `Snapshot` produit par `credentials_from_source` est explicitement éphémère, supprimé au dernier Drop (`src/control_fs.rs:341-347`) : il ne constitue pas le binding durable du futur journal.

Choix recommandé avant de figer le futur schéma journal : conserver `ExecutionIntentV1` comme identité de l'invocation/demande et des références disponibles dans RunPlan, et engager dans un enregistrement d'acceptation privé immuable le lien `intent_hash` + hash du payload Submit exact + références de bindings credentials typés (manuel/fournisseur, binding_id, fichier privé, digest du record persistant et handoff minimal/noms autorisés), avec opération et périmètre authentifiés. Le manuel doit créer/synchroniser son binding durable avant le marqueur/ACK depuis le PrivateFile initial retenu ; jamais depuis un path rouvert après ACK. Le binding fournisseur3 existant conserve son identité/digest inchangés. Le rejet d'un binding manquant/altéré est exigé avant admission/premier start ; le stop n'exige aucun credential fournisseur. Rejeu sous verrou recharge le lien accepté, sans Auto ni source relue. Cette obligation est à tester dans le lot journal ; le présent composant pur ne la livre pas et ne doit pas annoncer l'identité complète de tous les inputs effectifs.

## Jalon daté — première revue du 30 octobre, gel produit cb1a12e4

Gels lus et recoupés : intent `cb1a12e42c923a67619632b92ff61145c88a768e217f76d21f7be8b3fa1a8c28`, mod `13bb6fc40b56fae11905cfdaadeb2d311d467b3bc4ae420c608c0a76c935c094`, runner `5131cf9e8938d2e6ad222f15fd977b45f2e41281417e4010d81aa4dddf069d39`, provider `354e6789535d9ab8aff0c641354a90661b26b978e0c35b0a45590f57363f15bd`, tests `46986706813facad51726d62718df5d3763a215e7c06945cfdf4e84ad090cdad`.

### BIR-01 — P2, erreurs brutes des snapshots publics (ouvert à ce jalon, fermé au gel final)

`src/broker/intent.rs:46-48` et `254-256` exposent `InitialOptionsV1` et `ProviderCredentialRefV1` avec `Deserialize` dérivé. Les entrées principales `ExecutionIntentV1::from_bytes` et son `Deserialize` custom enveloppent bien leurs erreurs ; aucun appel métier à la désérialisation directe des snapshots n'a été identifié. Cependant, leur API publique autorise `serde_json::from_slice::<InitialOptionsV1>` : une valeur canari dans `runtime` invalide apparaît dans l'erreur serde « unknown variant ». Sur `ProviderCredentialRefV1`, une clé inconnue nommée par le canari apparaît dans « unknown field ». C'est un écart au contrat annoncé d'erreurs opaques du composant, pas une fuite runtime qualifiée.

Fermeture recommandée : exposer uniquement les snapshots sérialisables et garder leurs wires `Deserialize` privés, ou envelopper leur désérialisation publique avec erreur fixe, comme `InitialRequestV1` le fait déjà. Une régression doit injecter le canari en clé inconnue ou en valeur d'enum invalide ; le canari actuel en simple VALEUR d'un champ inconnu (`tests/broker_intent.rs:362-368`) n'est normalement pas imprimé par serde et ne détecterait pas une régression vers une erreur brute. Vérifier aussi Debug des quatre objets publics avec un snapshot contenant prompt/path canaris.

### Acceptations et limites de la lecture

- Les deux bornes publiques précèdent le parsing ; la borne canonique est revérifiée avant hash. Options/request/provider/binding sont décodés en types stricts avant toute conversion Value ; le tri récursif, ordre des tableaux, texte et domaine+NUL sont lisibles et cohérents avec le golden.
- Projection du vrai RunPlan, `managed` et `credential_source` exclus, aucune lecture FS/réseau/TTY ni résolution Auto. UUID non nil, refus d'approbation appliquée, validation héritée du fournisseur complétée par égalité de provider_input, sélection explicite et prompt exact ; plans1/2 ne peuvent acquérir de sélecteur géré. Copies historiques des contrats/hashes inchangées.
- `sandbox_id` n'est pas traité comme un alias obligatoire de session : la recherche des contrats actuels ne démontre aucun invariant général de ce type (c'est une métadonnée bornée ; seul le parcours d'export legacy emploie sa valeur pour rechercher des événements). Aucun défaut n'est retenu sur la seule différence possible de cette métadonnée.
- Les tests couvrent un duplicate au niveau racine. Le rapport Luna mentionne collectivement unknown/duplicate à plusieurs niveaux, alors que les niveaux options/request/resolution exercent des unknown seulement ; ne pas citer ces nested duplicates comme exécutés sans assertion complémentaire.

### Preuves examinées, aucune commande de test exécutée par le reviewer

Rapport Luna lu : `/tmp/ai-guard-broker-intent-tests.md`, gel rapport `0b91189400e12d5f0211c2c908519fde48206c4d8072e842c44f4fef8ac428f4` annoncé. RED préliminaire E0432 conservé, absent-feature et non reproduction d'un défaut du broker existant. Logs GREEN `broker_intent` : **5 PASS/0 FAIL/0 ignoré**, manifestes source avant/après identiques aux gels. Canaris `provider_authorization`+`supervision_contract` : **11 PASS** ; golden `worker_registry` ciblé : **1 PASS/35 filtrés**. Ces fonctions pures ne prouvent ni binding réel disponible ni admission/ACK/journal, reprise, durée autonome, moteur, fournisseur ou identité tenant.

### Reproduction BIR-01 par Luna avant correction

Log lu : `/tmp/ai-guard-broker-intent-redaction-red-final.z3caW3/test.log`. La régression `public_snapshot_deserialization_errors_do_not_echo_canary` échoue effectivement avant correction sur les six canaux attendus : Options unknown-key Display/Debug, Options enum invalide Display/Debug, Binding unknown-key Display/Debug. UUID Binding est un contrôle négatif opaque ; Request/Top sont opaques également. Ne pas leur attribuer une fuite. Source test finale recoupée `daf7eaa0a27d6304dce8ca2b114895bf852417768430dbc06e9b1fe352d0e529`. Le test se limite à la surface publique de désérialisation ; il ne prouve aucune fuite d'un appel métier ni d'un credential réel.

## Revue du correctif final — BIR-01 fermé

Nouveau gel produit recoupé : intent `73078447289411744e410b8c2e7ee1db97444dde4cff4863c9455775795adbf3` ; les trois autres sources produit restent identiques. `InitialOptionsV1` et `ProviderCredentialRefV1` sont désormais des wrappers publics transparents sur des Data privés stricts, et leur `Deserialize` mappe tout échec vers un message constant. `InitialRequestV1` et `ExecutionIntentV1` conservent leur enveloppement opaque. Champs, format, getters, validation et copie des options restent identiques après les accès internes `.0`. Aucun nouveau défaut identifié à la relecture ciblée.

Le test BIR-01 reproduit avant correction passe ensuite, avec contrôles négatifs UUIDbinding/Request/Top inchangés. **BIR-01 : CLOSED / PASS** au périmètre API de désérialisation, sans revendication de fuite runtime passée.

## Dernière amélioration de preuve des bornes — acceptée

La source test finale `2bf54a9c45b113cb815ce03b306c3d23b13bf1fc826fe7a8a6c3330941e09bca` remplace les assertions peu discriminantes par trois cas utiles : JSON de plan valide paddé au-delà de32Mio avec `SourceTooLarge`, golden valide paddé au-delà de256Kio avec `IntentTooLarge`, puis plan de taille source admise et argv JSON <=96Kio dont le prompt15Kio U+0001 non-NUL fait dépasser la projection canonique256Kio, avec `IntentTooLarge`. Ils prouvent des gardes distincts sans dépendre d'un path invalide ni d'un simple échec JSON.

Logs finaux lus : `/tmp/ai-guard-broker-intent-bounds-green.WjYb4I/test.log` : **6 PASS/0 FAIL/0 ignoré**. `fmt.exit` et `clippy.exit` dans `/tmp/ai-guard-broker-intent-final-static.gNbFiX` valent0 ; le log Clippy confirme la fin sans warning. Les manifests explicites avant/après des deux dossiers sont identiques et couvrent également les nouveaux fichiers untracked.

**Verdict de revue final : favorable au lot logiciel pur, aucun défaut ouvert identifié.** Lecture statique du produit, des assertions et des logs ; aucune commande de test exécutée par le reviewer. L'unique `cargo build --release --locked` final est recoupé ci-dessous. Le journal/ACK/binding manuel durable/service/IPC/échéances/gardien/fencing et tout runtime restent hors livraison/qualification de ce lot. Les limites des sources et l'absence d'autorisation tenant créée par une intention restent explicites.

## Dernier build logiciel recoupé

Lecture de `/tmp/ai-guard-broker-intent-release.IF4htX/build.log` et `exit-code.txt` : unique `cargo build --release --locked` final **PASS**, code0,34,83s. Manifestes source avant/après identiques. Artefacts consignés : binaire `831a3e19854e29f35dade316f71cb463c43faacf49f5f96204277b5ac85b8e1a`, rlib `7edfc144d56f656bcc9980ec68dd83a42e2554b4a4b2bdce210d2fc8c1b6c280`. Rapport Luna final SHA-256 recoupé `abe3f111a47551cccea55dfca9dc3afd710cfb89ee4f87b5e8e1afaaac80bb6f` ; le libellé des duplicates y distingue désormais correctement racine testée et unknowns imbriqués.

Verdict inchangé : **PASS au périmètre du lot composant pur ; BIR-01 fermé, aucun constat ouvert**. Les critères runtime/journal/fournisseurs restent NOT_RUN pour ce lot, et ne peuvent être attribués ni aux tests purement logiciels ni au build release.
