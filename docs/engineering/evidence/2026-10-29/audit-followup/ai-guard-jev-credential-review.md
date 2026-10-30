# Revue indépendante du correctif de séparation Jev — 29 octobre 2026

**Conclusion finale après correction : B1 est traité dans la source relue et les logs fournis ; aucun bloqueur résiduel identifié pour la séparation Jev/contrôle du lot.** La première passe ci-dessous garde le défaut découvert et sa provenance. L'addendum final documente le nouveau gel, les contrôles bibliothèque et les limites de preuve.

Périmètre principal : `src/jev.rs`, `src/main.rs`, `src/operations.rs`, `tests/runtime_operations.rs`, `tests/jev_credentials.rs`. Lecture complémentaire de `src/gateway_config.rs`, `src/api.rs`, `src/lib.rs` et `tests/jev_contract.rs` pour vérifier les chemins d'assemblage effectifs. Aucun fichier source modifié et aucun code cible, test, fixture, endpoint ou fournisseur exécuté par le reviewer. Les règles AGENTS/RTK/contrats déjà lues et le skill security-audit en guidance ciblée restent appliqués. Aucun statut de l'audit original n'est changé.

Le fingerprint canonique du rapport externe est **`operations/jev-control-credential-reuse`**, et non `ops/jev-control-credential-reuse`. Son verdict `needs_validation` et l'absence de sévérité formelle restent inchangés. Les tests de développement lus ici ne sont pas une reproduction selon le sandbox formel d'audit. L'ancienne image QA et ses résultats ne prouvent pas ce correctif.

## B1 — La bibliothèque permet encore d'assembler Jev avec un credential de contrôle

**Bloquant pour une revendication « séparation appliquée par tous les chemins produit/bibliothèque ».** Pas de nouveau contournement distant démontré : le principal pertinent demeure le contrôle qui assemble une configuration erronée.

Trace source actuelle :

1. [src/lib.rs:2](/Users/louis/Documents/ai-guard/src/lib.rs:2) et [src/lib.rs:7](/Users/louis/Documents/ai-guard/src/lib.rs:7) exportent publiquement `api` et `jev`.
2. [src/jev.rs:38](/Users/louis/Documents/ai-guard/src/jev.rs:38) conserve un constructeur public `JevClient::new(endpoint, key, ...)`. Il vérifie l'URL, la présence de clé/modèle et le seuil, sans connaître les credentials du contrôle. Le nouveau validateur commun [src/jev.rs:8](/Users/louis/Documents/ai-guard/src/jev.rs:8) est un appel facultatif du caller ; il n'est pas associé au type construit.
3. [src/api.rs:166](/Users/louis/Documents/ai-guard/src/api.rs:166), `GatewayState::new`, reçoit à la fois runner, viewer et `Option<JevClient>`. `valid_credentials` vérifie uniquement runner/viewer non vides et distincts, puis conserve Jev tel quel.
4. [src/api.rs:193](/Users/louis/Documents/ai-guard/src/api.rs:193), `GatewayState::for_tenants`, dispose de tous les tenants et de Jev. Il vérifie identités, bases/intégrité et unicité des credentials de contrôle, puis conserve Jev sans comparer sa clé. Le token du second tenant n'est pas oublié dans main, mais cette seconde frontière d'assemblage demeure non protégée.
5. [src/api.rs:408](/Users/louis/Documents/ai-guard/src/api.rs:408), `fresh_policy`, appelle ce Jev après une autorisation locale. [src/jev.rs:120](/Users/louis/Documents/ai-guard/src/jev.rs:120) remet sa clé à `bearer_auth` puis envoie la requête. La redaction du body ne masque pas cet en-tête.

Un caller peut donc construire un JevClient avec exactement le runner ou viewer, puis appeler un constructeur GatewayState sans passer par main ni init. Le chemin réseau subsiste en source pour une décision locale autorisée. [tests/jev_contract.rs:31](/Users/louis/Documents/ai-guard/tests/jev_contract.rs:31) et [tests/jev_contract.rs:43](/Users/louis/Documents/ai-guard/tests/jev_contract.rs:43) utilisent déjà cette composition publique, avec une clé distincte ; ce n'est pas une hypothèse d'accès à des champs privés.

**Correctif minimal proposé :** rendre vérifiable la séparation de la clé privée de Jev depuis l'assemblage du gateway, sans l'exposer, puis l'exiger dans les constructeurs local et multi-tenant avant de produire un état utilisable. Le constructeur Jev seul ne connaît pas les tokens du gateway ; un helper public qu'on peut oublier ne suffit pas au dernier point de décision. La stratégie d'erreur de `GatewayState::new`, qui renvoie actuellement `Self`, doit rester fail-closed et explicite. **Ne pas traiter une collision en remplaçant Jev par None** : cela supprimerait silencieusement le contrôle externe.

Régression attendue : assemblage direct des deux constructeurs avec Jev=runner/viewer local et du tenant B ; refus ou état inexploitable avant toute décision, diagnostic sans valeur, aucune connexion à une socket fournisseur factice. Le cas distinct reste utilisable. Aucune nouvelle requête externe ni reproduction sur une pile partagée n'est nécessaire. Le reviewer n'a exécuté aucun de ces cas.

## Critères CLI/init vérifiés

| Critère | Résultat source et preuves fournies | Limite précise |
|---|---|---|
| Init refuse Jev=runner et Jev=viewer avant publication | **PASS source.** [operations:411](/Users/louis/Documents/ai-guard/src/operations.rs:411) lit les trois sources ; [operations:419](/Users/louis/Documents/ai-guard/src/operations.rs:419) compare avant `validate_existing` et avant toutes les publications à partir de :425. Le test [runtime_operations:146](/Users/louis/Documents/ai-guard/tests/runtime_operations.rs:146) couvre les deux rôles. | La création de répertoires et le durcissement de mode des sources précèdent ce contrôle, comme auparavant. Le critère porte sur contenus/inodes existants et absence de publication, pas sur l'absence de toute opération filesystem. |
| Contenus et inodes des clés/projections préservés au refus | **PASS fourni.** Le test sauvegarde les six fichiers source/projection, octets et inode à [runtime_operations:167](/Users/louis/Documents/ai-guard/tests/runtime_operations.rs:167), puis les compare après refus à :186. Le diagnostic est vérifié sans secret. | Il ne qualifie pas les courses de substitution, ACL ou le filesystem complet ; ces sujets appartiennent à d'autres candidats. |
| Startup local compare avant JevClient, Store et listener | **PASS source.** [main:185](/Users/louis/Documents/ai-guard/src/main.rs:185) charge les deux rôles, :201 lit Jev, :206 compare, :208 construit Jev, :240 ouvre le premier Store, :266 bind le listener. | Le reviewer n'a pas exécuté le startup. La preuve dynamique vient des logs de développement. |
| Startup registre couvre tous les tenants, dont B | **PASS source + fourni.** [main:179](/Users/louis/Documents/ai-guard/src/main.rs:179) charge la liste complète ; :196–199 collecte les deux rôles de chaque entrée avant comparaison unique :206. [gateway_config:84](/Users/louis/Documents/ai-guard/src/gateway_config.rs:84) ne crée pas de base pendant le chargement. Les deux tests Jev=runner/viewer du second tenant sont présents à [jev_credentials:149](/Users/louis/Documents/ai-guard/tests/jev_credentials.rs:149). | Le chemin bibliothèque B1 reste distinct. Aucun changement à gateway_config n'était nécessaire pour le chemin CLI relu. |
| Comparaison de valeurs plutôt que de fichiers bruts | **PASS source sur les chemins fichiers.** [main:148](/Users/louis/Documents/ai-guard/src/main.rs:148) applique trim aux tokens Jev et contrôle ; gateway_config utilise ce même lecteur. [operations:563](/Users/louis/Documents/ai-guard/src/operations.rs:563) applique trim à chaque source avant le validateur. Les valeurs comparées ont la même normalisation à leur entrée respective. | Le helper de bibliothèque compare littéralement les strings reçues et n'impose pas de normalisation. Aucun cas de test dédié avec espaces de bord différents n'est fourni. Pas de bypass CLI identifié en source ; ajouter cette régression utile sans inventer une vulnérabilité. |
| Rejet sans base tenant créée ni service gateway annoncé | **PASS fourni.** [jev_credentials:108](/Users/louis/Documents/ai-guard/tests/jev_credentials.rs:108) exige exit non nul, aucun message `Gateway`, diagnostic attendu et aucune des bases de fixture à :132. Le contrôle source précède effectivement le bind ; ce n'est pas une simple vérification d'un message après ouverture. | Ces fixtures utilisent uniquement des bases neuves ; le non-changement d'une base existante découle de l'ordre source, sans essai dédié de son inode/contenu dans ce lot. |
| Clé distincte acceptée, pas de fuite et pas d'appel fournisseur | **PASS fourni.** Le positif [jev_credentials:159](/Users/louis/Documents/ai-guard/tests/jev_credentials.rs:159) atteint healthz. Les sorties sont comparées à tous les tokens factices (:95) et le listener Jev privé doit n'avoir reçu aucune connexion (:101). Init distinct est conservé dans [runtime_operations:100](/Users/louis/Documents/ai-guard/tests/runtime_operations.rs:100). | Cela prouve l'absence de contact de startup dans ces fixtures, pas une décision externe, ni une rotation live, ni un service fournisseur réel. |

Les trois échecs superficiels demandés ont donc été examinés : **tenant B inclus**, **trim cohérent avant égalité sur les lecteurs fichiers**, **rejet avant publication/client/Store/listener**. Aucun défaut bloquant relevé sur ces trois chemins CLI dans le snapshot relu.

## Logs et provenance

- [RED](/tmp/ai-guard-jev-credential-red.log) : les quatre startups avec collision servent avant correction ; le test init accepte la collision et échoue sur l'assertion ajoutée. Le positif distinct passe déjà. Il s'agit d'échecs comportementaux, pas d'un échec de compilation.
- [GREEN](/tmp/ai-guard-jev-credential-green.log) : 5 tests CLI Jev, 9 tests operations, 3 tests Jev de bibliothèque existants et 4 tests gateway_config réussis, zéro échec/ignoré pour ces ensembles. Le log conserve un second passage des 14 premiers ; **21 tests distincts**, pas un double comptage. Les 3 tests Jev existants portent sur parsing/URL, pas sur B1.
- Le [rapport auteur](/tmp/ai-guard-jev-credential-report.md) donne les commandes `--offline --locked`, les limites, la préservation des valeurs et les logs clippy/format. Ces résultats sont des tests de développement autorisés selon sa provenance ; cette revue n'en fait pas une confirmation sous sandbox formel.
- [Clippy ciblé](/tmp/ai-guard-jev-credential-clippy.log) et [format ciblé](/tmp/ai-guard-jev-credential-fmt.log) sont à intégrer avec le gel final ; les vérifications globales du dépôt restent à la charge de root.

Empreintes du snapshot relu, calculées sans exécuter de cible :

```text
67585510265892340cd43e44468dc8479fe91635929d620b3fb26259a1bd56ff  src/jev.rs
dfc772e81dd26a19de7c842f834c38010894068ae10373fc21bc1282212f2822  src/main.rs
a7edf62b13631e5ca17d57d595457057f046b10acb84f07076699bdd8862d42e  src/operations.rs
4bc8e51d733f372840073bf32160df0e522081434ac6bb67c22dffa69a3318e9  tests/runtime_operations.rs
feaa387d1a1267093d951560a629b7b2733e753e170144834561e717ee1b4fdc  tests/jev_credentials.rs
48eb7af0d120f583ffe976fb895c4c91a61cab7145266437952644da440b8a7a  src/api.rs
e75156a82f6e0701acb85dfeda7a1da5b92cd5a94928049a726217d4700f8e9f  src/gateway_config.rs
e0bb01e9f56b428aff7c3c814a5b43061e90aea6bf03c27dcbe8299482760118  RED log
bfc1ce00023bfaf99d59dced2ae9a60b4f2487ace0b3a1f39fe09d0b61f1d180  GREEN log
```

**Décision proposée :** accepter le comportement corrigé des entrées CLI/init sur les preuves fournies ; traiter B1 avant d'annoncer une séparation imposée par la bibliothèque ou de fermer le candidat pour tous ses callers. Ajouter un test de normalisation aux régressions sans le présenter comme défaut reproduit. Toute confirmation formelle réseau de l'audit reste **BLOCKED** faute de sandbox complet établi ; toute exécution par ce reviewer est **NOT_RUN**. Aucune qualification Docker/Podman, QA archivée, release ou fournisseur n'est transférée à ce correctif.

## Addendum final — B1 traité, seconde lecture du gel

Le propriétaire a reproduit puis traité le défaut bibliothèque signalé, sans intervention du reviewer dans les sources. L'état initial de B1 ci-dessus est historique ; il ne décrit plus le gel courant.

- [src/jev.rs:45](/Users/louis/Documents/ai-guard/src/jev.rs:45) normalise la clé au constructeur. La valeur normalisée est celle stockée dans le champ privé, puis utilisée par `bearer_auth`, pas uniquement une copie destinée à la comparaison. [src/jev.rs:84](/Users/louis/Documents/ai-guard/src/jev.rs:84) expose la validation de séparation sans exposer la valeur privée.
- [src/api.rs:175](/Users/louis/Documents/ai-guard/src/api.rs:175) ajoute cette validation aux conditions d'identité utilisable de `GatewayState::new`. En collision, `valid_credentials=false` : [authorize](/Users/louis/Documents/ai-guard/src/api.rs:247) refuse toutes les routes authentifiées avant sélection du tenant, politique ou provider. Jev et policy restent présents ; aucun fallback `Jev=None` n'est introduit. Le constructeur historique garde son retour `Self` et le diagnostic de configuration invalide reste générique. Healthz reste une route publique, ce qui ne confère aucun droit de décision.
- [src/api.rs:206](/Users/louis/Documents/ai-guard/src/api.rs:206) vérifie Jev contre les runner/viewer de toutes les entrées avant d'assembler `for_tenants`, avec propagation d'erreur. Le tenant B est inclus.
- [tests/jev_credentials.rs:229](/Users/louis/Documents/ai-guard/tests/jev_credentials.rs:229) vérifie localement runner/viewer × clé exacte/paddée : identité 503, décision 503, head d'audit inchangé et aucune connexion fournisseur. [tests/jev_credentials.rs:348](/Users/louis/Documents/ai-guard/tests/jev_credentials.rs:348) exige l'erreur d'assemblage pour les deux rôles du tenant B, exacte/paddée. Le positif distinct à :312 conserve l'autorisation viewer. Ces tests exercent l'assemblage public, pas main à sa place.

**PASS fourni :** `/tmp/ai-guard-jev-library-red.log` montre 8 échecs comportementaux avant correction : identité 200 au lieu de 503 ou assemblage accepté. `/tmp/ai-guard-jev-library-green.log` contient le passage final de 31 tests d'intégration (api_security4, gateway_tenancy1, jev_credentials14, operations9, tenant_api3) plus les 3 unitaires Jev et les 4 gateway_config, soit **38 distincts**, tous réussis ; ne pas ajouter à ce total les passages intermédiaires également conservés dans le log. Le clippy ciblé termine sans alerte ; le log fmt est vide et l'auteur rapporte exit0. Aucune exécution indépendante par le reviewer.

Empreintes finales revues :

```text
157b05a5a6c2a2ba7a9dda43b261d7f9044b062a9e4c2f11b7d3de3290f72344  src/jev.rs
823451d1a68c91e4985237d24934bd896c10a808b40df25844bff3b8a61c0b79  src/api.rs
2bc67301de756f77b05eafb780975b5f9f7098d4ed1879fb1c0802805e07fa24  tests/jev_credentials.rs
0c9b02391c162cd8c0c90ef945d6a067642759146ba387da3c2b03c59e317cae  library RED log
532d2eb168406286bdcdffeaf12fc4f9874c407a99e09471ca89f991df76382e  library GREEN log
```

**Décision finale : B1 traité, acceptation source + tests fournis pour ce lot.** Main/init conservent leur vérification anticipée avant client/base/listener/publication ; les constructeurs bibliothèque ajoutent la protection du dernier assemblage. Une base fournie à un constructeur bibliothèque a naturellement déjà été ouverte par son caller ; l'exigence « avant création de base » porte sur le startup CLI et y reste satisfaite. Pas de nouveau bloqueur concret identifié dans ce gel. L'audit original reste `needs_validation`, sans sévérité ; sa confirmation formelle et les effets externes restent hors de cette revue. La QA209 et son image ne contiennent pas ces correctifs et ne sont pas utilisées comme preuve.
