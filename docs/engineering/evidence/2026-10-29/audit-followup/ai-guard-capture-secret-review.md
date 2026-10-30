# Revue indépendante — frontières de capture et diagnostics secrets

29 octobre 2026. **Un défaut concret de chevauchement a été découvert pendant la revue, reproduit par l'auteur puis corrigé. Après relecture du nouveau gel et des logs fournis, aucun autre bloqueur source concret n'est identifié dans ce périmètre.** L'acceptation porte sur les chemins examinés et les preuves de développement fournies. Elle ne change aucun verdict de l'audit formel et ne qualifie pas une release.

Le reviewer n'a modifié aucune source et n'a exécuté aucun code cible, test, fixture, moteur, endpoint ou fournisseur. Les lectures couvrent `src/redaction.rs`, `src/adapters.rs`, `src/runner.rs`, `src/runner/managed.rs`, `src/runtime.rs`, `src/runtime/managed.rs`, les tests directement concernés et les logs. AGENTS/RTK/contrats actuels et security-audit en guidance ciblée appliqués. Le checkout entièrement non suivi a été préservé. L'image QA209 ne contient pas ces correctifs et n'est utilisée pour aucune preuve de ce lot.

## Défaut trouvé et traité : occurrences complètes chevauchantes

Le premier code de `mark_literal_bytes` remettait `matched=0` après un match, ce qui ne marquait que des occurrences non chevauchantes. Démonstration source minimale : secret factice `abab`, capture tronquée `abababX`. Le premier match couvre les octets 0–3 ; le second match complet 2–5 est ignoré ; le dernier `X` ferme tout suffixe préfixe. Le résultat conservait donc `abX` après le marqueur. Un secret périodique long laissait un reliquat long. Cela contredisait l'union annoncée de toutes les occurrences littérales complètes ; ce n'était pas un simple manque de test.

Le reviewer a signalé ce contre-exemple au parent sans exécuter de cible. L'auteur a ajouté une régression courte et une longue, produit [le RED](/tmp/ai-guard-capture-secret-overlapping-complete-red.log), puis corrigé le code. Le log échoue sur l'assertion de fragment et ne contient pas une valeur réelle de credential.

Le code courant [src/redaction.rs:204](/Users/louis/Documents/ai-guard/src/redaction.rs:204) reprend maintenant `prefix[matched-1]` après chaque match, ce qui conserve les chevauchements KMP. `marked_until`, propre à chaque pattern, ne remplit que la portion nouvelle d'un match ; il évite de rebalayer toute sa longueur pour chaque occurrence périodique. [tests/redaction_boundaries.rs:97](/Users/louis/Documents/ai-guard/tests/redaction_boundaries.rs:97) exige que les cas `abab/abababX` et long périodique soient intégralement masqués jusqu'au dernier octet de leur union tout en gardant `useful` et `X`. Le [GREEN final](/tmp/ai-guard-capture-secret-control-green-final.log) contient ce test réussi. **Défaut traité sur source + preuve fournie ; aucune reproduction indépendante revendiquée.**

## Analyse du nouveau passage de masquage

| Point examiné | Conclusion source et limite |
|---|---|
| Ordre avant conversion UTF-8 | [captured_text](/Users/louis/Documents/ai-guard/src/redaction.rs:98) travaille sur les octets originaux pour les captures tronquées. Il calcule les tables KMP, marque toutes les occurrences complètes, puis marque le plus long suffixe qui est un préfixe propre d'un secret. Tous les suffixes candidats ayant la même fin, le plus long représente leur union. La conversion UTF-8 lossy ne vient qu'après remplacement des régions cachées. Une coupe au milieu du premier caractère UTF-8 d'un secret connu est donc reconnue avant l'apparition de U+FFFD. |
| Indices et secrets vides | [Redactor::new](/Users/louis/Documents/ai-guard/src/redaction.rs:65) ne garde que les secrets de longueur au moins 4 octets. Les patterns passés aux helpers sont donc non vides. Le scan de suffixe porte au plus sur `len(secret)-1` octets : `matched` ne peut y atteindre la longueur du pattern. Dans le scan complet, le fallback après un match reste strictement inférieur à cette longueur. `marked_until` ne dépasse pas l'index de fin du prochain match. Aucun accès hors borne identifié dans ces branches. |
| Temps et mémoire | Pour N octets capturés, K secrets et S la somme de leurs longueurs, tables O(S), scans O(K×N), marquage au plus N octets par secret, masque/output O(N) en plus des tables. Les buffers du runner sont bornés à 256 KiB et stderr de contrôle à 64 KiB. Cette analyse ne fait pas de K une constante : l'API publique Redactor n'impose pas de plafond universel au dictionnaire ou à son entrée. Le produit borne les sources de credentials et les captures ; aucune qualification de capacité/latence n'est déduite du seul algorithme. |
| Périodicité et secrets imbriqués | Le marquage de chaque secret utilise le même input original et le même masque, sans remplacer un secret court avant de chercher le préfixe du long. L'union protège les régions qui se recouvrent. Le nouveau fallback couvre les occurrences périodiques complètes. Les tests courts/longs, `aaaabbbb` et multiples préfixes sont présents. |
| Fusion des dictionnaires | [with_secrets](/Users/louis/Documents/ai-guard/src/redaction.rs:73) conserve le dictionnaire original et ajoute les valeurs privées dans une nouvelle instance. [RuntimeCommandFailure::redacted_stderr](/Users/louis/Documents/ai-guard/src/runtime.rs:288) fusionne **avant** de traiter les octets : il ne masque pas séparément une copie déjà modifiée. Le test [redaction_boundaries:81](/Users/louis/Documents/ai-guard/tests/redaction_boundaries.rs:81) couvre union et absence de mutation du dictionnaire d'origine. |
| Portée de la garantie | Pour `truncated=false`, le comportement textuel existant est conservé. Ce correctif protège les littéraux complets connus et le préfixe final compatible avec une capture tronquée ; il ne promet pas de reconstruire des secrets encodés arbitrairement, des fragments fabriqués ailleurs dans le flux, des valeurs inconnues ou des secrets sous le seuil du dictionnaire. Les bornes de capture ne sont pas une politique de rétention du moteur. |

## JSONL : conservation des records complets et redaction après décodage

[parse_redacted_capture](/Users/louis/Documents/ai-guard/src/adapters.rs:61) ne masque pas les octets de syntaxe JSON. Si le flag truncated est vrai et que les bytes ne finissent pas par LF, seule la portion après le dernier LF est omise. Le flag `unterminated_final_line_omitted` représente ce fait, même si le préfixe final aurait pu se parser comme JSON. Si la capture est complète, le dernier record valide sans LF reste admis. Une coupe exactement après LF conserve tous les records délimités.

La projection passe par [normalize_line](/Users/louis/Documents/ai-guard/src/adapters.rs:88) : ligne bornée à 64 KiB, parsing JSON, refus non objet, puis `redactor.value` avant `codex`/`claude`/`generic` et leurs clips/budgets. Le correctif ne remplace donc pas un secret contenant guillemets/antislash dans la syntaxe brute d'un autre champ. Les clés et valeurs décodées sont masquées avant la projection. Les plafonds existants 500 événements/capture, 32 événements/ligne, texte4KiB, données12KiB, profondeur8 et collections32 restent explicites dans l'adaptateur.

[finish_execution](/Users/louis/Documents/ai-guard/src/runner/managed.rs:1735) conserve le nombre normalisé et publie la couverture avec `stdout_truncated`, le flag de record omis, le plafond, `after_completion`, et `nested_authorization:false`. Il ne transforme pas ces logs déclarés en preuve indépendante d'autorisation transitive. Les octets totaux, flags et timestamps runtime ne sont pas recalculés à partir du texte masqué.

**PASS source + fourni :** tests derniers records LF/sans LF, dernière ligne valide mais non délimitée malgré truncated, JSON échappé, secret dans une clé et valeurs décodées avant projection. Aucun contournement concret de cette frontière identifié dans le gel relu. Aucun moteur/fournisseur réel exercé ici.

## Erreurs moteur : données typées, contexte utile et markers privés

Le chemin initial perdait les bytes et le flag de troncature en construisant une erreur à partir d'un stderr lossy. Le [RED unitaire runtime](/tmp/ai-guard-control-capture-error-red.log) échoue sur la présence brute dans la chaîne d'erreur. Le premier [RED de câblage broker](/tmp/ai-guard-capture-secret-control-red.log) relève aussi la perte du diagnostic utile/provenance ; un autre [RED de contrôles](/tmp/ai-guard-capture-secret-control-chars-red.log) relève une raison invalide pour le contrat durable. Ces résultats appartiennent à la correction en cours, pas au gel QA précédent.

Le code actuel conserve la cause :

- [RuntimeCommandFailure](/Users/louis/Documents/ai-guard/src/runtime.rs:276) contient statut, Capture stderr et dictionnaire runtime privé. [capture_cli](/Users/louis/Documents/ai-guard/src/runtime.rs:476) garde la capture64KiB et son compteur total/flag avant de produire ce type sur statut non-success. `Display` n'affiche que runtime/statut ; `Debug` seulement runtime/statut/métadonnées de capture. Aucune valeur privée ou bytes n'entre dans ces formats par défaut. Le type n'a pas de Serialize/Deserialize. L'accès explicite `stderr_capture` demeure une API de confiance documentée comme non journalisable brute ; la garantie n'est pas que cet accesseur public rend les bytes physiquement inaccessibles à son caller.
- [cli_sensitive](/Users/louis/Documents/ai-guard/src/runtime/managed.rs:404) conserve l'erreur et lui ajoute les redactions privées, puis un contexte générique. Ce n'est plus un remplacement de toute la cause par un message opaque qui empêcherait le caller de retrouver le Capture. Avec binding Podman établi, volume marqueur et nonce rejoignent le dictionnaire privé.
- Le [bootstrap Podman](/Users/louis/Documents/ai-guard/src/runtime/managed.rs:197) passe explicitement ces deux valeurs à `cli_sensitive` **avant** `binding.podman=Some(marker)` à :218. Le chemin d'échec de création ne dépend donc pas d'un binding qui n'existe pas encore. La réponse de création ambiguë ou les JSON d'inspection invalide produisent des erreurs génériques sans inclure leur contenu privé.
- [redacted_error](/Users/louis/Documents/ai-guard/src/runner.rs:75) parcourt la chaîne de causes. Sur RuntimeCommandFailure, il ajoute longueur retenue, longueur totale, flag truncated et `redacted_stderr`, en gardant le contexte utile. Le masquage précède le clip ; les caractères de contrôle sont ensuite échappés pour ne pas rendre Unknown.reason invalide. La sortie console de Run [runner:623](/Users/louis/Documents/ai-guard/src/runner.rs:623), Unknown [managed:1360](/Users/louis/Documents/ai-guard/src/runner/managed.rs:1360), cleanup [managed:1604](/Users/louis/Documents/ai-guard/src/runner/managed.rs:1604) et postprocessing [managed:1754](/Users/louis/Documents/ai-guard/src/runner/managed.rs:1754) passent par ce formatter avant leur clip final.
- Les captures worker stdout/stderr [managed:1678](/Users/louis/Documents/ai-guard/src/runner/managed.rs:1678) passent par `captured_text` avant clip8192 et stockage du completed/output event. Le nouveau cas du harnais terminal vérifie les deux flux, les compteurs et flags, le texte utile et l'absence du fragment factice, sans créer un faux terminal ni libérer le quota dans le cas d'erreur de contrôle.

La frontière publique ne copie pas les markers : RuntimeAuthority et son PodmanBinding se sérialisent dans le checkpoint privé, [Debug masque le marker](/Users/louis/Documents/ai-guard/src/runtime/managed.rs:25), ContainerHandle/snapshot ne portent que fingerprint/identités de tentative, et [action_status](/Users/louis/Documents/ai-guard/src/runner/managed.rs:1970) renvoie worker/observation/message/résultat, pas le checkpoint/binding. Les inspections privées ne désérialisent pas Config.Env vers les DTO. Aucun chemin source de ce périmètre ne désérialise RuntimeCommandFailure ou ses redactions privées depuis une entrée API.

**Limite historique de la première passe, maintenant traitée :** l'unitaire runtime couvrait l'opacité Display/Debug, la capture bornée et l'ajout d'un secret privé ; les tests managed déjà existants couvraient Debug et marqueur manquant/modifié. Le raccordement « échec volume create qui écho volume+nonce au bootstrap avant Some » était alors vérifié seulement en source. L'addendum ci-dessous examine la nouvelle régression et ferme précisément ce manque de preuve.

## Résultats retenus et limites

| Statut | Preuve et portée |
|---|---|
| **FAIL historique, traité** | Contre-exemple chevauchant découvert par cette revue, reproduit dans le RED auteur et corrigé ensuite ; autres RED de capture/formatter lus comme preuves de développement. Aucun log ne justifie une confirmation formelle de l'audit. |
| **PASS fourni, gel final ciblé** | `/tmp/ai-guard-capture-secret-control-green-final.log` : redaction_boundaries11 + runner_recovery7 + runner_terminal_audit1 + runtime_authorization6 = **25 tests**, zéro échec/ignoré. Le harnais terminal comprend plusieurs scénarios mais reste un test Cargo. |
| **PASS fourni, unitaires spécifiques** | `/tmp/ai-guard-control-capture-error-green-final.log` : runtime failure opaque/capture/privates, 1 test. `/tmp/ai-guard-capture-secret-ordinary-error-green-final.log` : formatter causes/contexte/secrets/contrôles, 1 test. Ne pas recompter les exécutions intermédiaires. |
| **PASS fourni, outils ciblés** | Clippy ciblé sans avertissement dans `/tmp/ai-guard-capture-secret-control-clippy.log`. Log fmt vide, exit0 rapporté par l'auteur. Le reviewer a lu ces logs, pas relancé les outils. |
| **PASS source reviewer** | Analyse des indices et complexité KMP, ordre bytes→mask→UTF8→clip, records JSONL complets, chaîne typée et fusion privée, bootstrap avant binding et DTO publics. Aucun autre bloqueur concret identifié après le correctif chevauchement. |
| **NOT_RUN reviewer** | Tous tests/probes/builds/moteurs/navigateurs et endpoints. Pas de preuve de chrono/performance sur capacité100, pas de fuite réelle reproduite, pas de validation du worker après mort du broker. |
| **BLOCKED formal audit** | Sandbox OS complet d'audit non établi pour cette revue : réseau, environnement allowlisté, lectures seules, scratch, limites descendantes et promotion sûre ensemble. Les verdicts originaux restent inchangés, aucune sévérité nouvelle. |

Le parent garde les checks globaux après gel intégré. Les preuves Docker/Podman, image QA209, UI et release précédentes ne se transfèrent pas à ce lot. Le contrôle du volume et de la rétention moteur reste un sujet distinct de la taille de la copie capturée en mémoire.

## Empreintes relues après traitement du chevauchement

```text
e18aaa022a343a6c14f0133aced38a637ff5e8a4089d1b25e3a17be2fbba8b1d  src/redaction.rs
0bd3479b5a540844c9d50ad13526c7749b0e6caa6cf0eecf1071fb71c4f145d4  src/adapters.rs
49f1a8b8637e3b69aec148258d75e168f26d2b11c01d1baaa6d3348482974609  src/runner.rs
401f7bdfd0d88fd7921f6d01042b44ddb71385e00d3d8f6343f72b4aafe7e6bd  src/runner/managed.rs
810298d1694a52c3a91b97906b7b37973c55a85d5ce7d836e5485530010a28a3  src/runtime.rs
198db2824296a706040ad043f48620ad525a88d474c869dd894892221b775b74  src/runtime/managed.rs
6daff1d665735fc42b27f6360c1916e085a0f3d891bb9b5fd8b05a96c00ec5c9  tests/redaction_boundaries.rs
3f506aadd4be3b20e472e27bb00dcf58ea552c1281d7a8c531988b26d17cc408  tests/runner_terminal_audit.rs
ce8550634c0effdbf5d040bdd03d6fe7c9617f1f0a0a10b1c76edb1edb2c2dd6  tests/runtime_fixtures/runner_terminal.rs
a5083f4b15d0a027c84bf1cd8d8b9dbce99b5944cda9081459e8509e5549828d  capture/control final GREEN
918d3cc27223e39b64a821c6837737275d44553064e243d21cfc5ccd17df0f70  overlapping complete RED
f9dd9eb810fb90132a2d6229a8fb30c29e0c0284eeb762c3575741464f7dc1bd  runtime typed error final GREEN
0c611cfabc1d529c7f8962bf5f57824cd1b304cfd3ae30ad0e4797080ea73df1  ordinary formatter final GREEN
```

## Addendum — échec du bootstrap privé et nouvelles preuves fournies

**Manque de régression marker fermé sur source + log fourni.** Aucun code cible exécuté par le reviewer et aucune nouvelle modification de code demandée par cet addendum.

[tests/runtime_fixtures/managed.rs:50](/Users/louis/Documents/ai-guard/tests/runtime_fixtures/managed.rs:50) lit les arguments de création du marqueur, garde les valeurs factices dans les fichiers privés de la fixture puis, sous `marker_create_error`, émet sur stderr un contexte utile accompagné du nom du volume et du nonce, et termine en échec125. Ce chemin est bien la création du marqueur, pas son inspection après bootstrap réussi.

[tests/runtime_managed.rs:514](/Users/louis/Documents/ai-guard/tests/runtime_managed.rs:514) déclenche cet échec via `ManagedRuntime::bootstrap_podman` avant toute autorité retournée. Il exige que la cause `RuntimeCommandFailure` reste récupérable, appelle son renderer avec un dictionnaire caller vide, puis vérifie que **les deux** valeurs sont absentes du stderr rendu et des formats génériques Display/Debug de l'erreur. Il exige en même temps que `private marker` et `[REDACTED]` soient conservés. Ce test ne peut pas réussir par simple suppression de toute la cause ou tout le texte utile. Le code applicatif relu reste celui où `cli_sensitive` reçoit explicitement volume et nonce avant `binding.podman=Some(marker)`.

Le [log ciblé](/tmp/ai-guard-control-private-marker-green.log) rapporte **1 PASS, 0 FAIL, 0 ignoré**, en0,46s. Le scénario reprend ensuite le bootstrap normal, conserve les vérifications de marqueur changé/manquant et n'émet aucun start/kill/rm. Ce résultat qualifie le raccordement de redaction sur une **fixture CLI Rust**, pas le fonctionnement d'un vrai moteur Podman.

Les sources applicatives de la revue précédente sont inchangées ; les deux fichiers de test sont maintenant :

```text
fb2cd312fc8ff1481ad27c58b3befbe8c4b187fda845ac6db5d3d56951d32bdc  tests/runtime_managed.rs
5651e969d5dbab679e0e9ab963cf56ca8f22fb5f1f052a3ebb82ab0c1fa33a3c  tests/runtime_fixtures/managed.rs
3ad3ea59bad9c02dcac48490310d4190b1ed258db74317ca49af69c43793df8f  private marker GREEN log
```

Ces deux empreintes et celles de redaction/adapters/runner/runtime relues correspondent aux entrées du [manifeste du nouveau gel](/Users/louis/Documents/ai-guard/docs/engineering/evidence/2026-10-29/audit-followup/SOURCE_SHA256SUMS.txt), SHA256 `59d0eb554879bc2bbe98ae5cdbb5ef5bb6f477944277c5cf3df1f6f3637b3c34`. Le manifeste n'est pas une preuve d'effet ; il lie ici les lectures source aux artefacts annoncés par le parent.

Preuves complémentaires lues, avec leurs portées distinctes :

- **PASS fourni, vérifications globales Rust :** `/tmp/ai-guard-followup-tests.log` totalise235 réussites,0 échec et3 ignorés sur37 groupes de résultats, doc-tests compris. Les3 ignorés restent NOT_RUN dans cette commande. Logs clippy/release terminés avec succès ; log fmt vide et exit0 rapporté par le parent. Ces résultats ne sont pas des exécutions du reviewer.
- **PASS fourni, capture Docker réelle synthétique :** `/tmp/ai-guard-followup-deployed-smoke.log` rapporte1 test réussi en22,24s. La source [runtime_smoke:268](/Users/louis/Documents/ai-guard/tests/runtime_smoke.rs:268) construit un credential strictement factice de4097 octets et le remet par env-file privé au worker, sans le passer en argv et sans réutiliser une clé de contrôle. La commande conteneur émet64 répétitions sur stdout et stderr. Les assertions exigent flags truncated sur les deux flux, absence du préfixe secret dans l'événement durable, texte utile et marqueurs de redaction. Le profil est **observe+bridge explicitement choisi**, car le contrat actuel d'injection de credentials l'exige. La commande n'effectue aucune requête fournisseur ; aucune qualification de réseau coupé n'est tirée de ce scénario bridge.
- **PASS fourni, provenance du nouveau contrôle :** le log de build contient l'image `sha256:4eb348a836fe51ae3325aa5c078a98a505ab0dc87306762f35b1faf567c12a78` ; le log de déploiement rapporte recréation et santé de gateway/dashboard/ingress. L'attribution du smoke déployé à ce nouveau gel est celle du parent et de ces artefacts. Aucune preuve de l'image QA209 précédente ne sert à cet addendum.
- **PASS fourni, autres effets ciblés Docker :** `/tmp/ai-guard-followup-real-isolation-terminal.log` rapporte confinement/copie/export/timeout broker vivant en4,14s, puis refus terminal HTTP503 en3,13s : même instance arrêtée conservée, audit valide, allocation absente et quota inconnu retenu. Ils ne qualifient ni la durée autonome après disparition du broker ni une panne disque.
- **PASS fourni, navigateur :** `/tmp/ai-guard-followup-deployed-browser.log` contient10 contrôles live réussis. Ils couvrent le dashboard Rust, l'authentification, SSE/export et les vues worker/quota/allocation ; ils ne prouvent pas la redaction marker, un appel fournisseur ou une nouvelle direction CSS.

Le complément ferme **le seul manque de test marker signalé dans cette revue**. Aucun nouveau bloqueur concret découvert. Les verdicts de l'audit original restent inchangés ; aucune confirmation formelle Cloudflare, Podman réel, capacité100 ou release n'est déclarée.

### Vérification documentaire de la direction visuelle

[AGENTS.md:27](/Users/louis/Documents/ai-guard/AGENTS.md:27) et [docs/ui.md:3](/Users/louis/Documents/ai-guard/docs/ui.md:3) nomment bien **Nothin’** comme référence du mainteneur. La déclinaison demande typographie affirmée, noir/blanc, contraste et espace, puis les adapte aux tableaux/logs de supervision avec thèmes, clavier/focus, mobile, mouvement réduit et états métier incomplets. Le texte de docs/ui indique que ces principes guident les prochains chantiers visuels ; il ne présente pas une nouvelle CSS comme livrée ou qualifiée. Ce contrôle est documentaire ; aucune nouvelle capture ou inspection visuelle du produit n'a été réalisée par le reviewer.
