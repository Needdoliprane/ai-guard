# Revue indépendante ciblée — API supervision et horloge native

2026-10-30. Worktree exclusivement : `/Users/louis/Documents/ai-guard/.worktrees/broker-supervision`, branche `codex/broker-supervision`, base annoncée `e2cda5e`. Lecture de code/contrats/docs et métadonnées Git ; aucun test, build, moteur, modèle, API externe ou service lancé. Aucun fichier du dépôt ou worktree modifié. Skill security-audit utilisé en guidance seulement ; skills de projet `.agents`/`.claude` absents du worktree. Le Store déjà revu n'est pas réaudité ici.

## Gel relu

| Fichier | SHA-256 |
|---|---|
| `src/api.rs` | `854e654f153f70eee1c9ee8532dab85bd4012aae8bae541f40a5d3a98e40b893` |
| `src/api/supervision.rs` | `295cc728975d18c55e5f053908bc05b50471b4abfafc6f9cb8db25722d4d81b2` |
| `src/lib.rs` | `788835958faaea3f1cda206bde430dd8e758ea448e1105c32dcb8694cfab0e67` |
| `src/broker/mod.rs` | `8dff47d0124b1612bd576da30518ba5a9a7c87132ea62f8a9ae2b47b40b1d8ff` |
| `src/broker/clock.rs` | `ec4b279749e502692dfd58b958efa53b2046ee58f651d5e26f7745e3190edd33` |
| `docs/api.md` | `6a30f805da1c017de78471be349b50413f4c64590904d77d7fbc8f5a15a76509` |
| `schemas/supervision-protocol.schema.json` | `2a560d62f1f9fb3f61d58cad4b1df36b98efd2529de76c87e13b8a7db1ad20fd` |

## Verdict source

**Aucun défaut P0/P1/P2 concret identifié dans le delta relu.** Pas de correction applicative demandée à ce stade. Verdict comportemental en attente des rapports/logs finaux de Luna ; les observations ci-dessous sont des vérifications de code, pas des PASS de tests déduits du code.

### Frontière API

- [Routes](/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/src/api/supervision.rs:10) : exactement deux GET et sept POST. Le merge est limité à `/api/v1` dans [gateway_router](/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/src/api.rs:346). Aucun fresh Bind, activate, moteur HTTP ou alias nouveau `/api` ajouté ; liste dashboard inchangée.
- Chaque handler commence par ReadIdentity ou RunnerIdentity avant Path/Json. Tenant/rôle viennent exclusivement de l'index authentifié existant. La résolution d'identité réutilise les guards déjà relus ; elle ne fait pas confiance à un tenant/role du corps.
- [owned_arm](/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/src/api/supervision.rs:35) charge l'arm durable dans le Store de cette identité puis compare son parent réel au chemin. Les routes helper l'utilisent avant mutation. Changer `worker_id` dans un reçu ne remplace pas ce lookup ; le Store vérifie ensuite le reçu intégral. Une arm étrangère/absente ne révèle pas son contenu. Les endpoints arm/export comparent également le worker du corps au chemin, puis la validation Store s'applique.
- Les erreurs JSON sont normalisées en réponse contrôlée sans erreur détaillée du parseur ni écho du corps. Les contrats reçus n'ajoutent ni secret de contrôle, ni chemin privé de binding, ni environnement d'exécution. Les réponses sont DTO/résultats du Store ; les erreurs internes passent par `control_error`.
- [Admission](/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/src/api/supervision.rs:58) : validation typed, enveloppe bornée à 65536 octets et égalité après redaction avant la branche de politique. Lookup d'opération durable avant tout appel externe ; une opération existante passe au contrôle de replay exact du Store sans Jev, y compris si le Store doit finalement refuser une divergence. Demande fraîche : schéma3/active vérifiés avant `fresh_policy`, puis autorité/quotas/réservation revérifiés transactionnellement par Store.
- La prélecture n'est pas une réservation : une invalidation ou concurrence après celle-ci peut encore provoquer un refus Store après l'évaluation de politique. Le code et les docs ne doivent pas être interprétés comme une garantie d'un appel externe unique pour plusieurs soumissions simultanées encore non durables.
- Réouverture inactive : lectures autorisées et replay exact du claim durable passent par les mêmes reçus ; le Store demeure l'unique source de `AlreadyConsumed`/`EmitOnce`. Aucune conversion d'un refus en autorisation, aucun fallback legacy ajouté.

### Producteur d'horloge

- [Initialisation](/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/src/broker/clock.rs:64) : Linux lit le chemin noyau fixe, au plus38 octets ; Darwin utilise un buffer de37 octets, exige longueur exacte et terminaison NUL du sysctl. UUID hyphéné validé, non nil et normalisé une fois. Aucun chemin fourni par caller ni cache credential consulté.
- [sample](/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/src/broker/clock.rs:159) n'effectue aucune lecture filesystem : lecture native du compteur, conversion entière contrôlée et construction du DTO. Pas de fallback UTC ; `observed_utc=None`. Les erreurs ne contiennent que nom d'opération fixe et code système, pas de chemin privé ni valeur de boot.
- `timespec_to_ns` refuse secondes négatives et nanosecondes hors intervalle ; multiplication/addition u128 checked. `ticks_to_ns` refuse ratio nul et dépassement, arrondit vers le bas sans flottants. `checked_deadline` refuse durée0 et addition débordante. Les paramètres de timeout métier restent validés par le contrat d'arm, pas inventés par ce producteur.
- [ABI Darwin](/Users/louis/Documents/ai-guard/.worktrees/broker-supervision/src/broker/clock.rs:217) recoupée avec le header installé `/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/usr/include/mach/mach_time.h` : struct deux uint32_t, pointeur timebase, retour kern_return_t, compteur uint64_t et disponibilité macOS10.12. `repr(C)` et signatures Rust concordent. Le header indique l'avancement pendant le sommeil ; cela ne constitue pas une observation de suspend/reprise réalisée dans ce lot.
- Boot UUID identifie une session de boot, pas un hôte/tenant ni une autorisation. Il faut conserver namespace de contrôle et bindings d'autorité à l'intégration. Le module ne fournit ni ordonnanceur, ni watchdog, ni stop target, ni permis de start, ni détection prouvée de reboot/snapshot de VM.

### Documentation, schéma et valeur des preuves

Les neuf routes, rôles, statuts, lecture de protocole, différence schéma/capacité et absence de Bind/activation concordent avec le code. Le schéma JSON décrit le DTO fermé et la contrainte active⇒schema3+namespace+protocole ; ses propriétés tenant/rôle ne sont pas un moyen de choisir une identité. Les liens ajoutés vers les fichiers de contrats/schéma existent par lecture. `git diff --check` ne présente pas d'erreur de whitespace ; ce n'est pas un test applicatif.

Lecture des nouveaux tests, sans les exécuter : les POST refusés utilisent un body dont les polls sont comptés ; le scénario valide utilise les vrais Store SQLite, les arms/reçus, refus A/B, claims et réouverture inactive. Les tests d'horloge incluent conversions, UUID, échantillons natifs et un enfant du même exécutable dont le boot/counter sont comparés avant/après. L'entrée enfant ignorée n'est pas un PASS autonome et ne doit pas être additionnée au test parent.

**Limite de preuve précise :** le harnais API lu configure Jev à `None`. Il ne mesure donc pas un nombre d'appels externes sur replay/refus/preflight. Le branchement source confirme leur évitement dans les cas annoncés, mais aucun PASS dynamique « zéro appel Jev » n'est attribué à cette fixture. Un éventuel contrôle supplémentaire peut employer une sonde locale comptée ; aucun fournisseur réel n'est nécessaire.

Les bornes du harnais enfant ne qualifient pas une borne système universelle, ni reboot/suspend, ni effet moteur. La branche Linux n'est pas qualifiée dynamiquement par un run macOS. Le modèle de confiance reste celui d'un contrôle hôte intègre ; lecture d'un boot ID ne prouve pas une résistance à un administrateur ou à une restauration matérielle.

## Résultats exécutés

Au gel de cette première rédaction : nouveaux targets API/clock exécutés par Luna, rapports finaux attendus pour recoupe. Aucun résultat anticipé. Le RED de routes/module absents est une preuve de préparation de tests, pas une vulnérabilité d'un ancien runtime.

## Addendum — preuves finales API/Clock recoupées

Lecture du log `/tmp/ai-guard-supervision-clock-final2.94temq/test.log`, SHA-256 **`db7fb8867ed86cdf142a27777a508a02d35155fd7c83ebe3826e06afe9ca1822`**, contexte d'invocation et manifestes avant/après. `cmp` confirme les manifestes identiques. Les cinq fichiers applicatifs du tableau ci-dessus ainsi que docs/schéma concordent toujours avec le worktree. Branche vérifiée `codex/broker-supervision`, HEAD/base **`e2cda5eb889b9fc8f8053085ccbaf6942792be67`** ; la mention `e2cda5a` du rapport API reçu est une coquille signalée à son auteur, sans divergence des hashes de source.

Commande exécutée par Luna, non par le reviewer : `cargo test --locked --test broker_clock --test supervision_api`, exit0. Résultats directs :

- **API : 3 PASS, 0 FAIL, 0 ignoré.** Guards avant consommation des bodies, protocole tenant/rôle, alias historique, flux réel Axum/SQLite d'admission/helpers/claims et maintenance après réouverture.
- **Clock : 5 PASS, 0 FAIL, 1 helper ignoré.** Le test parent lance explicitement ce helper dans un processus enfant, lit son échantillon via pipe et vérifie le compteur entre les deux lectures parentes. Le helper ignoré n'est pas un sixième PASS autonome.

Tests exacts de cette preuve : `tests/broker_clock.rs` **`96828d05b2ee6ee299a4b96f87c0cc644dd8234d708edeb935b58fd6a20be44a`** ; `tests/supervision_api.rs` **`3344d9b43be1af08abaa5164cec709ba9ef8bcee8abf972b69a6afd094fc6116`**. Rapports lus/hachés : `/tmp/ai-guard-broker-clock-tests.md` **`bc5654896a7cc1b17aa825c1006c630fc315061580ce2aa1637ffc2e8cf5f777`**, `/tmp/ai-guard-supervision-api-tests.md` **`04b0aadd78f172f69120d12e1a802792ec75de9b1b55c54308836b82dfc2159d`** à réception. Leurs corrections de harnais (framing stdout enfant et baseline d'audit après écriture par la fixture) sont distinguées des RED de fonctionnalités absentes ; aucun de ces RED n'est requalifié en vulnérabilité runtime.

**Verdict du delta : revue ciblée acceptée, sans finding P0/P1/P2 ouvert, avec les huit tests produits ci-dessus PASS sur les sources relues.** Cela ne qualifie pas le broker permanent, l'intégration start/stop, le fencing/reboot, le suspend, un watchdog ou Linux. Aucun test/build lancé par le reviewer ; aucune qualification finale fmt/Clippy/release déduite de ces seuls logs.

La limite **Jev=None** reste explicitement ouverte comme limite de preuve comportementale, sans défaut applicatif établi : une preuve additionnelle avec Jev configuré et compteur local a été confiée à Luna, sans appel modèle. Aucun résultat de ce futur contrôle n'est anticipé ici.

## Addendum final — sonde Jev configurée et oracle namespace corrigé

Le code applicatif demeure exactement au gel du tableau initial. Le delta porte sur `tests/supervision_api.rs`. La sonde utilise un vrai `JevClient` configuré avec une URL HTTPS loopback, une clé QA fictive et `allow_local_http=false`. Un listener local compte les connexions puis ferme le stream ; il ne fournit ni réponse TLS ni modèle. Le contrôle positif appelle volontairement `decide`, attend l'erreur et exige **une connexion observée** avant remise du compteur à zéro. Le router avec `Some(jev)` traite ensuite le replay exact et une admission fraîche sur la base rouverte inactive ; les réponses attendues et l'absence d'append sont vérifiées, puis le compteur reste à zéro. Aucun bypass TLS, secret réel ou serveur externe n'est utilisé.

**Réserve Jev fermée pour ces deux cas précis : replay exact d'une admission durable et refus frais `supervision_inactive`.** Ce test ne démontre ni une évaluation unique sous concurrence, ni tous les refus possibles de validation, ni l'absence générale d'effets réseau. Le branchement source reste distinct de cette preuve comptée.

Provenance recoupée, sans exécution reviewer :

- Première version Jev, test SHA **`d5fe386f3f41be0b3f1cda5c6a4f0b5663ccf732721df9109fa826d795b5419a`** : `/tmp/ai-guard-supervision-jev-test.JcKWiz/test.log`, SHA **`8b4b0ebc1f5daf8a65f1230122afe4182e7f8be10d906efe403e9ad260dbb78d`**, API **4 PASS**. Manifestes avant/après identiques.
- L'ancien oracle namespace testait `len==2`, alors que trois réponses schema3 étaient collectées ; cette comparaison ne s'exécutait donc pas. Ce défaut d'assertion n'était pas un défaut produit. L'oracle final exige trois UUID non nil, namespace A-viewer égal à A-runner et différent de B-viewer. Le SHA final après correction/formatage est **`41ed177f0c7d851d80a51767e62cb494659f3ec9bb6d4628fe8438627db44f58`**.
- La preuve de cet oracle final est un **rejeu ciblé distinct**, `/tmp/ai-guard-supervision-api-jev-oracle.m5ba3n/test.log`, SHA **`e98df517f512d94beff5682ec43cd208f26b1293e32a6f27900e648a4da44842`** : API **4 PASS/0 FAIL/0 ignoré**. Ses manifestes avant/après sont identiques et portent bien le SHA final41ed. Le PASS du log d5fe n'est pas rétroactivement attribué à cette assertion renforcée.
- Canaris historiques séparés : `/tmp/ai-guard-supervision-api-canaries.UrDIs8/test.log`, SHA **`b7afbe8dfe6daf619e43c842322952e39a6c253da71fb6c313b7065cac368ce4`**, **20 PASS** : auth_boundary6, api_security4, gateway_tenancy1, tenant_api3, workers_api6. Manifestes avant/après identiques. Aucune de ces suites ne qualifie un moteur.

Logs et contextes de `/tmp/ai-guard-supervision-api-checks-final.gqrSOo/` également recoupés : fmt global, Clippy `--locked --all-targets -- -D warnings` et build release `--locked` **exit0**. Manifestes avant contrôle et après release identiques ; l'identité d'artefact enregistrée dans `release-binary-hash.txt` est **`d707f22344c1fea43bcbb90eb64cd3a4cc516903f10efecee35f500b3a7faf31`**. Le reviewer n'a ni lancé ni qualifié cet exécutable sur un runtime.

**Verdict final inchangé : delta accepté en revue ciblée, aucun finding produit ouvert.** Les résultats applicables sont API4 + Clock5, helper ignoré distinct, et20 canaris historiques ; les répétitions API3/API4 ne sont pas additionnées comme nouveaux tests uniques. La preuve Jev est désormais acquise dans les deux cas ci-dessus. Reboot/suspend/Linux, concurrence d'évaluation, broker/IPC/guardian, échéances imposées et D5 demeurent hors qualification de ce lot.
