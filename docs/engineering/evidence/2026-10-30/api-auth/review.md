# API/auth — revue indépendante ciblée

30 octobre 2026. Revue en lecture du delta, des assertions et des logs fournis ; aucun test, serveur, moteur, API ou modèle exécuté par le relecteur. Aucun secret réel consulté. Ce rapport ne constitue pas l'audit global Daybreak ni une qualification générale HTTP/DoS.

## Identité examinée

- Baseline exacte `/tmp/ai-guard-api-auth-original/api.rs` : `823451d1a68c91e4985237d24934bd896c10a808b40df25844bff3b8a61c0b79`.
- `src/api.rs` : `0d61502c45d2b25dec3c135b71495d11e024ec47b414525c48dea637ceaeceb8`.
- `src/api/auth.rs` : `f1b1b30222ecbfb1220be5dc82191e03bf560cc4bf5e528e30033641a747ceee`.
- Test final `tests/api_auth_boundary.rs` : `1817710533dbf0f36e71b2be19022053a717e26fc70426146b4d64464d0bdd80`. La version RED pré-rustfmt est identifiée séparément par `6c07398cb6d68fe9430d0c90194374c38eb25cf389699963c97578407e47471c` ; pas de substitution de preuve.

Hashes vérifiés directement. AGENTS et contrats d'architecture/règles appliqués au périmètre.

## Verdict

**Pas de défaut P0/P1/P2 reproductible identifié dans ce delta. Correction de la frontière d'authentification acceptée dans son périmètre**, avec les limites HTTP ci-dessous. Les preuves RED/GREEN ferment l'ordre erroné authentification/extraction, l'acceptation de champs Authorization multiples et l'absence de borne locale du bearer. Elles ne ferment pas la gestion complète des corps et connexions hostiles.

## Contrôles confirmés par lecture

1. `auth.rs:90–114` : extracteurs `FromRequestParts` ne consomment pas le corps. Dans toutes les routes protégées enregistrées par `gateway_router`, `ReadIdentity`/`RunnerIdentity` précèdent `Path`, `Query` et `Json`, y compris les corps `Result<Json<…>, JsonRejection>`. Les deux alias utilisent les mêmes handlers. Le dashboard utilise aussi le nouveau parseur bearer. `/healthz` reste volontairement public.
2. `auth.rs:13–30` : un seul champ Authorization, préfixe exact historique `Bearer `, jeton non vide, au plus 4096 octets, sans espace blanc. Deux champs identiques sont également refusés par le code. L'oversize est rejeté avant calcul du digest. Aucun token n'est copié dans une erreur. Les limites HTTP de réception des headers sont une autre couche.
3. `auth.rs:59–87`, `api.rs:128–257` : index SHA-256 vers tenant/role construit une seule fois depuis la configuration de confiance, doublon inter-rôles/inter-tenants refusé. Les positions de tenant restent stables ; `with_worker_quotas` ne réordonne pas le vecteur. Header/query/body ne choisissent pas le tenant. Un index incohérent échoue en 503. Le lookup HashMap n'est pas constant-time ; le commentaire le dit correctement. Aucune promesse cryptographique de temps constant n'est ajoutée.
4. `GatewayState` partage index, tenants, policy, Jev et redactor via Arc. Cloner l'état par requête ne recopie donc plus les listes de politique ni la clé Jev/dictionnaire de secrets. `TenantState` ne conserve plus runner/viewer tokens. Cela ne signifie pas effacement sécurisé des allocations antérieures ni absence de tout secret en mémoire.
5. Configuration locale invalide : `new` garde la compatibilité de signature mais verrouille les routes authentifiées en 503 ; `for_tenants` retourne une erreur. Validation identité/base/intégrité et séparation Jev sont conservées. En configuration valide, bearer absent/invalide = 401 ; viewer sur écriture = 403 avant extraction. Les refus généraux Origin/URI peuvent encore précéder ces statuts, comme avant.
6. Les corps métier et opérations durables ne sont pas réécrits : décision/replay, `store.decision`, `admit_worker`, `mutate_worker` et traitement des erreurs restent en place après le guard. Aucune voie d'autorisation publique ni nouvel accès inter-client introduit. Cette lecture du delta ne requalifie pas intégralement la couche de stockage existante.

## Preuves recoupées

Rapport `/tmp/ai-guard-api-auth-boundary-tests.md` ; répertoire `/tmp/ai-guard-api-auth-boundary.4lP1m9/`.

- `red-final.log`, SHA `0b5ab8c5ae4687a6f8736c2267cad6c48bfee524f7b2e3ee64f0abd6f0fa898e` : **4 FAIL / 2 PASS**, dont vraie attente TCP avant envoi du corps, statuts/query erronés, polling non autorisé et doublon accepté. Ce RED est conservé.
- `green-new-target.log`, SHA `e3541ad7da60f3070525c89773a1b39a4451a8f238650d50ff6d2bb1a44b6ece` : **6 PASS / 0 ignored**. Les assertions couvrent les quatre familles POST et deux alias, absence/invalidité/viewers A/B sans polling, audit inchangé, parsing runner conservé, sélection A/B malgré indices tenant adverses, doublon, oversize et premier statut TCP 401 sans corps envoyé.
- `green-regressions.log`, SHA `80efd8a4d262bb5a97f800d2b4eb5ab0af34b1fe7bca41135e4d59b63a17db9e` : **45 PASS / 0 ignored**, huit cibles annoncées. Lecture ciblée de `tenant_api` : mauvais binding/doublons refusés, audit A altéré → 503 tandis que B fonctionne ; `jev_credentials` vérifie aussi la configuration locale invalide, absence de contact fournisseur et absence de mutation durable.
- Logs fmt/Clippy cohérents avec le rapport ; Clippy all-targets termine sans warning. Manifests avant/après identiques par `cmp`, hashes des deux sources et du test concordants. Aucun build release annoncé ici.

## Limites à conserver

- Le test TCP envoie `Connection: close`, n'envoie aucun corps et vérifie uniquement la première ligne 401. Il ne prouve ni drainage sûr, ni fermeture complète bornée, ni réutilisation HTTP/keep-alive, ni résistance à une charge de connexions lentes. Le chantier drainage HTTP reste distinct et ouvert ; pas de faux PASS transféré.
- Le test de doublon utilise GET identity avec corps vide : il prouve le refus du doublon, mais pas directement l'absence de polling pour ce cas. Celle-ci est démontrée par le même parseur et guard placés avant les corps POST ; le compteur direct couvre absent/invalide/viewer/oversize. Aucun besoin de relancer une campagne pour constater cette distinction.
- Les nouveaux tests ne couvrent pas exhaustivement les frontières 4096 exactement, tous les espaces blancs, ni 503 avec un corps jamais fourni. Leur traitement est lisible dans les branches communes ; ce sont des limites de couverture, pas des contournements établis.
- Aucune qualification des autres services, du cloud, de la charge100, des fournisseurs ou des garanties transitoires du broker n'est issue de ce lot. Les réserves précédentes de supervision restent indépendantes.

Statut : revue du delta **ACCEPTÉE** ; preuves ciblées **PASS** relues ; exécution par le relecteur **NOT_RUN** ; qualification HTTP générale **NON ÉTABLIE**.
