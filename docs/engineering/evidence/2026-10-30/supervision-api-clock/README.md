# Supervision : API v1 et horloge native

30 octobre 2026. Lot logiciel du worktree `codex/broker-supervision`, base `e2cda5eb889b9fc8f8053085ccbaf6942792be67`. Aucun moteur, modèle, service OS ou navigateur lancé pour ce lot. Tests confiés à Luna ; [revue indépendante](review.md) du gel puis recoupe des preuves, sans test exécuté par le reviewer.

## Comportement livré

Neuf routes v1 uniquement, décrites dans [l'API](../../../../api.md) : protocole authentifié, admission supervisée, armements/lecture, claims worker/helper, observations/cleanup helper et export. Deux GET viewer/runner, sept POST runner. Identité avant Path/Json ; le tenant provient du credential, les helpers retrouvent leur parent réel dans le Store de cette identité. Schéma3 du Store existant, sans activation/migration CLI ni route de bind legacy. Validation et état frais avant politique externe ; les transactions conservent le dernier contrôle. Un rejeu durable ne rappelle pas Jev ; plusieurs requêtes concurrentes encore non engagées ne bénéficient pas d'une garantie d'évaluation externe unique. [Compte rendu d'implémentation](implementation.md).

`NativeHostClock` produit un `HostClockSampleV1` avec méthode, UUID de boot non nil et compteur continu décimal u128. Initialisation native unique ; échantillon sans lecture de fichier ni UTC de secours. Calculs vérifiés et erreurs typées. Linux : `CLOCK_BOOTTIME` et boot_id ; Darwin : sysctl bootsessionuuid et mach_continuous_time/timebase. Références primaires : [Linux timekeeping](https://www.kernel.org/doc/html/latest/core-api/timekeeping.html), [header Apple XNU](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/osfmk/mach/mach_time.h). ABI Darwin également recoupée au SDK installé. Un boot ID n'est pas une identité tenant ou un permis d'exécution.

## Exécutions et provenance

Hôte : macOS/Darwin ARM64, toolchain Rust/Cargo1.99.1 fixée par le dépôt. Les observations runtime de l'API sont des fixtures déclarées ; seuls routeur, stockage SQLite et horloge native sont réellement exercés.

| Contrôle | Statut et preuve |
|---|---|
| API avant implémentation | RED attendu : trois groupes échouent sur routes absentes, [log](logs/red-api.log). Aucune vulnérabilité déduite des statuts d'une route absente. |
| Clock avant intégration | RED compilation E0433, module absent, [log](logs/red-clock.log). |
| Erreurs du harnais | [Premier log](logs/harness-first-failure.log) : parsing stdout enfant pollué par libtest. [Second log](logs/harness-second-failure.log) : baseline d'audit capturée avant une écriture de fixture. Corrections tests seulement. |
| `cargo test --locked --test broker_clock --test supervision_api` | PASS : clock5, API3, [log initial](logs/green-clock-and-api-initial.log). Un helper clock ignoré est explicitement invoqué par le parent et ne compte pas comme sixième PASS. [Manifestes avant](initial-manifest-before.txt)/[après](initial-manifest-after.txt) identiques. |
| API avec Jev configuré | PASS4, [premier ajout](logs/green-jev-before-oracle.log), puis [gel final](logs/green-api-final.log) après correction de l'oracle namespace. Trois réponses schema3 exigées, A viewer=runner, A≠B, UUID non nil. [Manifestes finaux avant](api-manifest-before.txt)/[après](api-manifest-after.txt) identiques. |
| Vrai transport Jev loopback | Le vrai client observe une connexion volontaire au listener HTTPS local qui ferme sans réponse TLS/modèle, puis zéro connexion pour rejeu exact et admission fraîche inactive503. Clé QA factice, TLS non contourné, aucun fournisseur appelé. Limite : ces deux cas seulement, pas la concurrence. |
| Régressions API | PASS20 : `api_auth_boundary`6, `tenant_api`3, `workers_api`6, `gateway_tenancy`1, `api_security`4, [log](logs/green-api-canaries-20.log). Le vrai HTTP enfant des cibles existantes fait partie de ces20. |
| Formatage | PASS `cargo fmt --all -- --check`, [contexte](fmt-context.txt), [sortie vide](logs/fmt.log). |
| Analyse statique | PASS `cargo clippy --locked --all-targets -- -D warnings`, [contexte](clippy-context.txt), [log](logs/clippy.log). |
| Release | PASS unique `cargo build --release --locked`, 53,57s, [contexte](release-context.txt), [log](logs/release.log). |
| Revue indépendante | ACCEPTED au périmètre du delta, aucun finding produit ouvert. Recoupe des sources, des logs et de la sonde Jev dans [le rapport](review.md). |

Soit29fonctions uniques dans les cibles ciblées, sans additionner les exécutions successives. [Rapport API](api-tests.md) et [rapport horloge](clock-tests.md) conservent l'historique et les limites de leurs gels respectifs. L'oracle namespace initial était inopérant pour trois réponses ; le log final correspond bien au test corrigé, SHA `41ed177f0c7d851d80a51767e62cb494659f3ec9bb6d4628fe8438627db44f58`. Clock inchangé : `96828d05b2ee6ee299a4b96f87c0cc644dd8234d708edeb935b58fd6a20be44a`.

## Artefacts et limites

Les15empreintes ciblées Cargo/sources/tests des contrôles finaux sont identiques [avant](checks-manifest-before.txt)/[après](checks-manifest-after.txt) et revérifiées sur le worktree après build. Aucun code applicatif n'a changé pendant les corrections de harnais. Les rapports API/clock et les logs sont copiés sans modification d'octets : retours Markdown à deux espaces et lignes vides finales Cargo conservés. Ces archives sont exclues du contrôle Git des espaces ; sources et documentation rédigée pour le lot restent contrôlées.

- Binaire : `d707f22344c1fea43bcbb90eb64cd3a4cc516903f10efecee35f500b3a7faf31`.
- Bibliothèque : `b4f7c6a28d0a7ce904995e0512749b061fb0ffca63d738b523eb0c5c8c708e21`.
- Copies privées mode600 dans `/tmp/ai-guard-api-clock-qualified.wcFxCz` ; archive Cargo/toolchain/src/tests `3e1451dfb5ce997bb169b8fafef0529c5fd00266098c6c8c7677eff112bc1307`. Les artefacts précédents restent préservés.

**NOT_RUN** : Linux natif, sommeil/reprise, reboot, erreurs syscall injectées, Docker/Podman/Kubernetes, fournisseurs/modèles et navigateur. Aucun nouveau déploiement QA, aucune preuve des anciennes images transférée au binaire de ce lot. Le test deux processus macOS ne prouve pas un reboot ou la résistance à un administrateur/snapshot mémoire.

**Écarts ouverts** : journal immuable, IPC authentifié, service/ordonnanceur, consommateurs de l'horloge, gardien TTY, reprise/réactivation et fencing D5. Le producteur d'horloge ne déclenche aucun arrêt à échéance. Le FAIL réel du crash CLI reste ouvert ; schéma2 par défaut, facturation désactivée, aucun gate production fermé.
